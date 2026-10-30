use aura_historia_jobs::{DomainJob, DomainJobPayload, WorkerScope, decode};
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use lambda_runtime::{Error, LambdaEvent};
use platform_lambda_bootstrap::{
    Dependency, FailureDiagnostic, LambdaInvocationBudget, UNCLASSIFIED_FAILURE,
};
use platform_lambda_sqs::{
    FailureCustody, FifoDisposition, RecordOutcome, process_fifo_batch, safe_message_id,
};
use std::{sync::Arc, time::Duration};
use tracing::{info, warn};
use user_loops::{LoopsMarketingEmailConsentWriter, LoopsNewsletterConfig};
use user_postgres::{SqlxMarketingConsentIntentRepository, SqlxMarketingConsentIntentWorker};
use user_service::use_cases::{
    SyncMarketingConsentIntentError, SyncMarketingConsentIntentHandler,
    SyncMarketingConsentIntentUseCase,
};

pub const LAMBDA_INVOCATION_CAP: Duration = Duration::from_secs(45);
const RESPONSE_HEADROOM: Duration = Duration::from_secs(5);
const MAX_RECORD_PROCESSING_BUDGET: Duration = Duration::from_secs(35);

pub fn compose_marketing_consent_sync_use_case(
    pool: sqlx::PgPool,
    loops: LoopsNewsletterConfig,
) -> Result<Arc<dyn SyncMarketingConsentIntentUseCase>, Error> {
    let provider = LoopsMarketingEmailConsentWriter::new(loops)
        .map_err(|_| Error::from("Loops consent writer configuration is invalid"))?;
    let repository = SqlxMarketingConsentIntentRepository::new();
    Ok(Arc::new(SyncMarketingConsentIntentHandler::new(
        platform_postgres::SqlxUnitOfWork::new(pool),
        SqlxMarketingConsentIntentWorker::new(),
        repository,
        provider,
    )))
}

pub fn invocation_budget(context: &lambda_runtime::Context) -> LambdaInvocationBudget {
    LambdaInvocationBudget::from_context(context, LAMBDA_INVOCATION_CAP, RESPONSE_HEADROOM)
}

/// `custody` holds jobs that can never complete, normally the queue's own dead-letter queue.
pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn SyncMarketingConsentIntentUseCase + Send + Sync),
    custody: &impl FailureCustody,
) -> Result<SqsBatchResponse, Error> {
    let budget = invocation_budget(&event.context);
    handler_with_invocation_budget(event, use_case, custody, &budget).await
}

pub async fn handler_with_invocation_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn SyncMarketingConsentIntentUseCase + Send + Sync),
    custody: &impl FailureCustody,
    budget: &LambdaInvocationBudget,
) -> Result<SqsBatchResponse, Error> {
    handler_with_budget(event, use_case, custody, budget.remaining()).await
}

pub fn retain_all_records(event: &LambdaEvent<SqsEvent>) -> Result<SqsBatchResponse, Error> {
    platform_lambda_sqs::retain_all_records(event)
}

async fn handler_with_budget(
    event: LambdaEvent<SqsEvent>,
    use_case: &(dyn SyncMarketingConsentIntentUseCase + Send + Sync),
    custody: &impl FailureCustody,
    remaining: Duration,
) -> Result<SqsBatchResponse, Error> {
    let results = process_fifo_batch(
        event,
        remaining,
        MAX_RECORD_PROCESSING_BUDGET,
        custody,
        |body| async move { process_marketing_consent_sync_job(&body, use_case).await },
        |attempt| {
            let disposition = match &attempt.outcome {
                RecordOutcome::Completed(disposition) => disposition.fifo_disposition(),
                RecordOutcome::MissingBody
                | RecordOutcome::InsufficientBudget
                | RecordOutcome::TimedOut
                | RecordOutcome::Panicked => FifoDisposition::Retain,
            };
            match disposition {
                FifoDisposition::Complete => {}
                FifoDisposition::Park => warn!(
                    message_id = safe_message_id(&attempt.message_id).unwrap_or("invalid"),
                    outcome = outcome_category(&attempt.outcome),
                    "Marketing consent sync job cannot complete as delivered"
                ),
                FifoDisposition::Retain => warn!(
                    message_id = safe_message_id(&attempt.message_id).unwrap_or("invalid"),
                    outcome = outcome_category(&attempt.outcome),
                    "Marketing consent sync record retained for SQS retry or redrive"
                ),
            }
            disposition
        },
    )
    .await?
    .response;
    info!(
        failed_sqs_message_count = results.batch_item_failures.len(),
        "Finished marketing consent sync FIFO batch"
    );
    Ok(results)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarketingConsentSyncJobDisposition {
    Applied,
    Blocked,
    Superseded,
    AlreadyTerminal,
    Deferred,
    Missing,
    Retry,
    Poison,
}

impl MarketingConsentSyncJobDisposition {
    /// An undecodable job and a missing intent can never complete: a job never recreates its
    /// intent, so both are parked on redelivery and later intents of the recipient continue.
    pub const fn fifo_disposition(self) -> FifoDisposition {
        match self {
            Self::Applied | Self::Blocked | Self::Superseded | Self::AlreadyTerminal => {
                FifoDisposition::Complete
            }
            Self::Missing | Self::Poison => FifoDisposition::Park,
            Self::Deferred | Self::Retry => FifoDisposition::Retain,
        }
    }

    const fn category(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Blocked => "blocked",
            Self::Superseded => "superseded",
            Self::AlreadyTerminal => "already_terminal",
            Self::Deferred => "active_lease_deferred",
            Self::Missing => "intent_missing",
            Self::Retry => "sync_unconfirmed",
            Self::Poison => "invalid_wire_job",
        }
    }
}

pub async fn process_marketing_consent_sync_job(
    body: &str,
    use_case: &(dyn SyncMarketingConsentIntentUseCase + Send + Sync),
) -> MarketingConsentSyncJobDisposition {
    let job = match decode(body, WorkerScope::MarketingConsentSync) {
        Ok(job) => job,
        Err(_) => return MarketingConsentSyncJobDisposition::Poison,
    };
    let intent_id = match intent_id_from_job(job) {
        Ok(intent_id) => intent_id,
        Err(_) => return MarketingConsentSyncJobDisposition::Poison,
    };
    match use_case.execute(intent_id).await {
        Ok(result) => {
            let disposition = match result {
                user_service::use_cases::SyncMarketingConsentIntentResult::Applied => {
                    MarketingConsentSyncJobDisposition::Applied
                }
                user_service::use_cases::SyncMarketingConsentIntentResult::Blocked => {
                    MarketingConsentSyncJobDisposition::Blocked
                }
                user_service::use_cases::SyncMarketingConsentIntentResult::Superseded => {
                    MarketingConsentSyncJobDisposition::Superseded
                }
                user_service::use_cases::SyncMarketingConsentIntentResult::AlreadyTerminal => {
                    MarketingConsentSyncJobDisposition::AlreadyTerminal
                }
                user_service::use_cases::SyncMarketingConsentIntentResult::Deferred => {
                    MarketingConsentSyncJobDisposition::Deferred
                }
                user_service::use_cases::SyncMarketingConsentIntentResult::Retryable => {
                    MarketingConsentSyncJobDisposition::Retry
                }
                user_service::use_cases::SyncMarketingConsentIntentResult::Missing => {
                    MarketingConsentSyncJobDisposition::Missing
                }
            };
            info!(
                intent_id = %intent_id,
                outcome = disposition.category(),
                "Marketing consent sync intent processed"
            );
            disposition
        }
        Err(error) => {
            let failure = sync_failure(&error);
            warn!(
                intent_id = %intent_id,
                outcome = "sync_unconfirmed",
                stage = failure.stage(),
                dependency = failure.dependency(),
                error_category = failure.category(),
                sqlstate = failure.sqlstate(),
                "Marketing consent sync intent remains unfinished"
            );
            MarketingConsentSyncJobDisposition::Retry
        }
    }
}

/// Keeps the failed stage and safe cause facts; the error chain itself is never logged.
fn sync_failure(error: &SyncMarketingConsentIntentError) -> FailureDiagnostic {
    use SyncMarketingConsentIntentError as E;
    match error {
        // The service returns this for both PostgreSQL and Loops state reads and drops the cause,
        // so the failed system cannot be named here.
        E::Persistence => FailureDiagnostic::new(
            "intent_sync",
            Dependency::Unknown,
            UNCLASSIFIED_FAILURE,
            None,
        ),
        E::Transaction(_) => FailureDiagnostic::postgres("intent_transaction", error),
        E::ReconciliationExhausted { .. } => FailureDiagnostic::postgres("reconciliation", error),
        E::InvalidPersistedState => FailureDiagnostic::new(
            "intent_read",
            Dependency::Postgres,
            "invalid_persisted_state",
            None,
        ),
    }
}

pub fn intent_id_from_job(
    job: DomainJob,
) -> Result<
    user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId,
    aura_historia_jobs::InvalidJob,
> {
    let DomainJobPayload::MarketingConsentSyncIntentCreated(job) = job.payload else {
        return Err(aura_historia_jobs::InvalidJob);
    };
    Ok(job.marketing_consent_sync_intent_id)
}

fn outcome_category(outcome: &RecordOutcome<MarketingConsentSyncJobDisposition>) -> &'static str {
    match outcome {
        RecordOutcome::Completed(disposition) => disposition.category(),
        RecordOutcome::MissingBody => "missing_message_body",
        RecordOutcome::InsufficientBudget => "insufficient_invocation_budget",
        RecordOutcome::TimedOut => "execution_timeout",
        RecordOutcome::Panicked => "handler_panicked",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use aws_lambda_events::sqs::SqsMessage;
    use lambda_runtime::Context;
    use platform_lambda_sqs::{FailureCustodyError, ParkedRecord};
    use std::{
        collections::VecDeque,
        sync::Mutex,
        time::{SystemTime, UNIX_EPOCH},
    };
    use test_api::CapturedLogs;
    use user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId;
    use user_service::use_cases::{
        SyncMarketingConsentIntentError, SyncMarketingConsentIntentResult,
    };

    #[tokio::test]
    async fn should_record_the_failed_stage_without_logging_its_cause() {
        let use_case = FakeUseCase::new([Err(SyncMarketingConsentIntentError::Transaction(
            Box::new(sqlx::Error::Io(std::io::Error::other(
                "SENTINEL_PAYLOAD_TOKEN_person@example.com",
            ))),
        ))]);
        let (logs, guard) = CapturedLogs::start();

        let response = handler(
            events([(Some("retry"), valid_body())]),
            &use_case,
            &RecordingCustody::default(),
        )
        .await
        .expect("batch response");
        drop(guard);

        assert_eq!(failure_ids(response), ["retry"]);
        let failures = logs.events_where(
            "message",
            "Marketing consent sync intent remains unfinished",
        );
        assert_eq!(1, failures.len());
        assert_eq!(Some("intent_transaction"), failures[0]["stage"].as_str());
        assert_eq!(Some("postgres"), failures[0]["dependency"].as_str());
        assert_eq!(Some("io"), failures[0]["error_category"].as_str());
        assert!(!logs.text().contains("SENTINEL"));
    }

    #[tokio::test]
    async fn acknowledges_only_confirmed_terminal_results() {
        let use_case = FakeUseCase::new([
            Ok(SyncMarketingConsentIntentResult::Applied),
            Ok(SyncMarketingConsentIntentResult::Blocked),
            Ok(SyncMarketingConsentIntentResult::Superseded),
            Ok(SyncMarketingConsentIntentResult::AlreadyTerminal),
            Ok(SyncMarketingConsentIntentResult::Retryable),
            Ok(SyncMarketingConsentIntentResult::Missing),
            Err(SyncMarketingConsentIntentError::Transaction(
                application::error::static_error("test transaction failure"),
            )),
        ]);
        let response = handler(
            events([
                (Some("applied"), valid_body()),
                (Some("blocked"), valid_body()),
                (Some("superseded"), valid_body()),
                (Some("terminal"), valid_body()),
                (Some("retryable"), valid_body()),
                (Some("missing"), valid_body()),
                (Some("retry"), valid_body()),
            ]),
            &use_case,
            &RecordingCustody::default(),
        )
        .await
        .expect("batch response");
        assert_eq!(failure_ids(response), ["retryable", "missing", "retry"]);
        // FIFO partial failure reports successors without starting them.
        assert_eq!(5, use_case.call_count());
    }

    #[tokio::test]
    async fn deferred_missing_and_retryable_intents_are_each_reported_as_failures() {
        for result in [
            SyncMarketingConsentIntentResult::Deferred,
            SyncMarketingConsentIntentResult::Missing,
            SyncMarketingConsentIntentResult::Retryable,
        ] {
            let use_case = FakeUseCase::new([Ok(result)]);
            let response = handler(
                events([(Some("unfinished"), valid_body())]),
                &use_case,
                &RecordingCustody::default(),
            )
            .await
            .expect("batch response");
            assert_eq!(failure_ids(response), ["unfinished"]);
        }
    }

    #[tokio::test]
    async fn fifo_stops_at_first_incomplete_record_and_retains_successors() {
        let use_case = FakeUseCase::new([
            Ok(SyncMarketingConsentIntentResult::Applied),
            Ok(SyncMarketingConsentIntentResult::Deferred),
            Ok(SyncMarketingConsentIntentResult::Applied),
        ]);
        let response = handler(
            events([
                (Some("first"), valid_body()),
                (Some("second"), valid_body()),
                (Some("third"), valid_body()),
            ]),
            &use_case,
            &RecordingCustody::default(),
        )
        .await
        .expect("batch response");
        assert_eq!(failure_ids(response), ["second", "third"]);
        assert_eq!(2, use_case.call_count());
    }

    #[tokio::test]
    async fn malformed_jobs_and_missing_bodies_remain_unfinished_without_logging_bodies() {
        let use_case = FakeUseCase::new([]);
        let response = handler(
            events([
                (
                    Some("invalid"),
                    "sensitive body that is not JSON".to_owned(),
                ),
                (Some("missing-body"), "".to_owned()),
            ]),
            &use_case,
            &RecordingCustody::default(),
        )
        .await
        .expect("batch response");
        assert_eq!(failure_ids(response), ["invalid", "missing-body"]);
        assert_eq!(0, use_case.call_count());
    }

    #[tokio::test]
    async fn timeout_and_panic_retain_the_record_and_fifo_successors() {
        let timeout = handler_with_budget(
            events([
                (Some("timeout"), valid_body()),
                (Some("successor"), valid_body()),
            ]),
            &PendingUseCase,
            &RecordingCustody::default(),
            Duration::from_millis(1),
        )
        .await
        .expect("batch response");
        assert_eq!(failure_ids(timeout), ["timeout", "successor"]);

        let panic = handler(
            events([
                (Some("panic"), valid_body()),
                (Some("successor"), valid_body()),
            ]),
            &PanickingUseCase,
            &RecordingCustody::default(),
        )
        .await
        .expect("batch response");
        assert_eq!(failure_ids(panic), ["panic", "successor"]);
    }

    #[tokio::test]
    async fn should_park_redelivered_invalid_jobs_and_missing_intents_so_later_intents_run() {
        let records = || {
            [
                ("invalid", "not JSON".to_owned()),
                ("missing", valid_body()),
                ("later", valid_body()),
            ]
        };
        let custody = RecordingCustody::default();
        let use_case = FakeUseCase::new([]);
        let response = handler(delivered_events(records(), 1), &use_case, &custody)
            .await
            .expect("batch response");
        assert_eq!(failure_ids(response), ["invalid", "missing", "later"]);
        assert_eq!(0, use_case.call_count());
        assert!(custody.parked_ids().is_empty());

        let use_case = FakeUseCase::new([
            Ok(SyncMarketingConsentIntentResult::Missing),
            Ok(SyncMarketingConsentIntentResult::Applied),
        ]);
        let response = handler(delivered_events(records(), 2), &use_case, &custody)
            .await
            .expect("batch response");
        assert!(failure_ids(response).is_empty());
        assert_eq!(2, use_case.call_count());
        assert_eq!(custody.parked_ids(), ["invalid", "missing"]);
    }

    #[tokio::test]
    async fn should_keep_active_leases_and_unconfirmed_syncs_retained_on_redelivery() {
        for result in [
            SyncMarketingConsentIntentResult::Deferred,
            SyncMarketingConsentIntentResult::Retryable,
        ] {
            let custody = RecordingCustody::default();
            let use_case = FakeUseCase::new([Ok(result)]);
            let response = handler(
                delivered_events([("unfinished", valid_body()), ("later", valid_body())], 5),
                &use_case,
                &custody,
            )
            .await
            .expect("batch response");
            assert_eq!(failure_ids(response), ["unfinished", "later"]);
            assert!(custody.parked_ids().is_empty());
        }
    }

    #[test]
    fn setup_retain_keeps_all_records() {
        let response = retain_all_records(&events([
            (Some("first"), valid_body()),
            (Some("second"), valid_body()),
        ]))
        .expect("partial response");
        assert_eq!(failure_ids(response), ["first", "second"]);
    }

    fn valid_body() -> String {
        r#"{"schema_version":1,"scope":"marketing-consent-sync","idempotency_key":"marketing-consent:mci_01h455vb4pex5vy7enb1p677vn","ordering_key":"marketing-email:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","job_type":"MARKETING_CONSENT_SYNC_INTENT_CREATED","payload":{"marketing_consent_sync_intent_id":"mci_01h455vb4pex5vy7enb1p677vn"}}"#.to_owned()
    }

    fn events<const N: usize>(records: [(Option<&str>, String); N]) -> LambdaEvent<SqsEvent> {
        let mut event = SqsEvent::default();
        event.records = records
            .into_iter()
            .map(|(message_id, body)| {
                let mut message = SqsMessage::default();
                message.message_id = message_id.map(ToOwned::to_owned);
                message.body = Some(body);
                message
            })
            .collect();
        let mut context = Context::default();
        context.deadline = epoch_millis().saturating_add(60_000);
        LambdaEvent::new(event, context)
    }

    /// One recipient's records, each delivered `receive_count` times.
    fn delivered_events<const N: usize>(
        records: [(&str, String); N],
        receive_count: u32,
    ) -> LambdaEvent<SqsEvent> {
        let mut event = events(records.map(|(message_id, body)| (Some(message_id), body)));
        for record in &mut event.payload.records {
            record
                .attributes
                .insert("MessageGroupId".to_owned(), "recipient".to_owned());
            record.attributes.insert(
                "ApproximateReceiveCount".to_owned(),
                receive_count.to_string(),
            );
        }
        event
    }

    #[derive(Default)]
    struct RecordingCustody(Mutex<Vec<ParkedRecord>>);

    impl RecordingCustody {
        fn parked_ids(&self) -> Vec<String> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .map(|record| record.message_id.clone())
                .collect()
        }
    }

    impl FailureCustody for RecordingCustody {
        async fn park(&self, record: &ParkedRecord) -> Result<String, FailureCustodyError> {
            self.0.lock().unwrap().push(record.clone());
            Ok("custody-message".to_owned())
        }
    }

    fn epoch_millis() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    fn failure_ids(response: SqsBatchResponse) -> Vec<String> {
        response
            .batch_item_failures
            .into_iter()
            .map(|failure| failure.item_identifier)
            .collect()
    }

    struct FakeUseCase {
        outcomes: Mutex<
            VecDeque<Result<SyncMarketingConsentIntentResult, SyncMarketingConsentIntentError>>,
        >,
        calls: Mutex<Vec<MarketingConsentSyncIntentId>>,
    }

    impl FakeUseCase {
        fn new<const N: usize>(
            outcomes: [Result<SyncMarketingConsentIntentResult, SyncMarketingConsentIntentError>;
                N],
        ) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into()),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl SyncMarketingConsentIntentUseCase for FakeUseCase {
        async fn execute(
            &self,
            intent_id: MarketingConsentSyncIntentId,
        ) -> Result<SyncMarketingConsentIntentResult, SyncMarketingConsentIntentError> {
            self.calls.lock().unwrap().push(intent_id);
            self.outcomes.lock().unwrap().pop_front().unwrap()
        }
    }

    struct PendingUseCase;

    #[async_trait]
    impl SyncMarketingConsentIntentUseCase for PendingUseCase {
        async fn execute(
            &self,
            _: MarketingConsentSyncIntentId,
        ) -> Result<SyncMarketingConsentIntentResult, SyncMarketingConsentIntentError> {
            std::future::pending().await
        }
    }

    struct PanickingUseCase;

    #[async_trait]
    impl SyncMarketingConsentIntentUseCase for PanickingUseCase {
        async fn execute(
            &self,
            _: MarketingConsentSyncIntentId,
        ) -> Result<SyncMarketingConsentIntentResult, SyncMarketingConsentIntentError> {
            panic!("synthetic panic payload")
        }
    }
}
