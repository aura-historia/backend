//! Captures one immutable canonical FX rate snapshot from FXRatesAPI.

use crate::OpsError;
use application::error::find_source;
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use fxrate_fxratesapi::FxRatesApiQuoteProvider;
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use fxrate_service::{
    CaptureFxRateSnapshotCommand, CaptureFxRateSnapshotError, CaptureFxRateSnapshotHandler,
    CaptureFxRateSnapshotOutcome, CaptureFxRateSnapshotUseCase, ports::FxRateQuoteProviderError,
};
use platform_postgres::SqlxUnitOfWork;
use platform_process::{Dependency, FailureDiagnostic, UNCLASSIFIED_FAILURE, config};
use serde_json::{Value, json};
use time::OffsetDateTime;
use tracing::{info, warn};
use uuid::Uuid;

const COMMAND: &str = "fx-refresh";
const FXRATES_API_TOKEN_ENV: &str = "FXRATES_API_TOKEN";

pub(crate) async fn run() -> Result<Value, OpsError> {
    let token =
        config::required(FXRATES_API_TOKEN_ENV).map_err(|error| OpsError::Config(error.into()))?;
    let snapshots = CaptureFxRateSnapshotHandler::new(
        FxRatesApiQuoteProvider::new(reqwest::Client::new(), token),
        SqlxUnitOfWork::new(crate::pool().await?),
        SqlxFxRateSnapshotRepositoryFactory::new(),
    );
    capture_fx_rate_snapshot(&snapshots, &FxRefreshRun::at(OffsetDateTime::now_utc())).await
}

/// The identity of one refresh run.
///
/// The Lambda deduplicated captures by the EventBridge Scheduler event ID
/// `fxrate:<scheduled-time>`, which a delivery retry of the same schedule slot repeats. The host
/// timer keeps that contract with the run's start truncated to the hour: a retry of the same
/// slot (a restarted unit, or a run whose commit response was lost) is recognized as a duplicate
/// and stores no second snapshot, while every scheduled slot (06:00 and 18:00 UTC) and a manual
/// run in another hour captures its own. The ID format matches the Lambda's, so stored
/// `source_event_id` values stay comparable. A fresh UUID v7 per run identifies the run in logs
/// as the Lambda request ID did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FxRefreshRun {
    run_id: Uuid,
    source_event_id: String,
}

impl FxRefreshRun {
    pub fn at(started_at: OffsetDateTime) -> Self {
        let slot = started_at.to_offset(time::UtcOffset::UTC);
        Self {
            run_id: Uuid::now_v7(),
            source_event_id: format!(
                "fxrate:{:04}-{:02}-{:02}T{:02}:00:00Z",
                slot.year(),
                u8::from(slot.month()),
                slot.day(),
                slot.hour()
            ),
        }
    }

    pub fn source_event_id(&self) -> &str {
        &self.source_event_id
    }

    fn operation_context(&self) -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new(self.run_id.to_string()),
            correlation_id: CorrelationId::new(self.source_event_id.as_str()),
        }
    }
}

/// Captures a snapshot for `run` and reports whether it was stored or already present.
#[tracing::instrument(
    skip_all,
    fields(run_id = %run.run_id, source_event_id = %run.source_event_id)
)]
pub async fn capture_fx_rate_snapshot(
    snapshots: &(dyn CaptureFxRateSnapshotUseCase + Send + Sync),
    run: &FxRefreshRun,
) -> Result<Value, OpsError> {
    let result = snapshots
        .execute(
            &run.operation_context(),
            CaptureFxRateSnapshotCommand {
                source_event_id: run.source_event_id.clone(),
                captured_at: OffsetDateTime::now_utc(),
            },
        )
        .await;
    match result {
        Ok(result) => {
            let outcome = match result.outcome {
                CaptureFxRateSnapshotOutcome::Captured {
                    fx_rate_id,
                    generation,
                } => {
                    info!(event = "fxrate.snapshot.captured", fx_rate_id = %fx_rate_id, generation = generation.as_i64());
                    json!({
                        "outcome": "captured",
                        "fxRateId": fx_rate_id.to_string(),
                        "generation": generation.as_i64(),
                    })
                }
                CaptureFxRateSnapshotOutcome::Duplicate => {
                    info!(event = "fx_rate_snapshot.duplicate");
                    json!({"outcome": "duplicate"})
                }
            };
            let mut report = json!({
                "runId": run.run_id.to_string(),
                "sourceEventId": run.source_event_id,
            });
            if let (Value::Object(report), Value::Object(outcome)) = (&mut report, outcome) {
                report.extend(outcome);
            }
            Ok(report)
        }
        Err(error) => {
            let failure = capture_failure(&error);
            warn!(
                event = "fxrate.snapshot.capture_failed",
                stage = failure.stage(),
                dependency = failure.dependency(),
                error_category = failure.category(),
                http_status = failure.http_status(),
                sqlstate = failure.sqlstate(),
                "failed to capture FX rate snapshot"
            );
            Err(OpsError::failed(COMMAND, error))
        }
    }
}

/// Keeps the failed stage and safe cause facts; the error chain itself is never logged.
fn capture_failure(error: &CaptureFxRateSnapshotError) -> FailureDiagnostic {
    use CaptureFxRateSnapshotError as E;
    match error {
        E::ServiceOrSystemPrincipalRequired => FailureDiagnostic::rejected("capture", "forbidden"),
        E::SourceEventIdRequired => FailureDiagnostic::rejected("capture", "invalid_input"),
        E::QuoteFetchFailed { .. } => quote_fetch_failure(error),
        E::InvalidQuotes { .. } => FailureDiagnostic::new(
            "quote_validation",
            Dependency::FxRatesApi,
            "response_invalid",
            None,
        ),
        E::BeginTransactionFailed { .. } => FailureDiagnostic::postgres("snapshot_begin", error),
        E::SnapshotInsertFailed { .. } => FailureDiagnostic::postgres("snapshot_insert", error),
        E::CapturedAtNotMonotonic => {
            FailureDiagnostic::rejected("snapshot_insert", "captured_at_not_monotonic")
        }
        E::CommitTransactionFailed { .. } => FailureDiagnostic::postgres("snapshot_commit", error),
    }
}

/// Classifies the provider call by the HTTP client error in the chain. Without one, the adapter
/// rejected the provider response itself, which its port variant still names.
fn quote_fetch_failure(error: &CaptureFxRateSnapshotError) -> FailureDiagnostic {
    let http = find_source::<reqwest::Error>(error);
    let category = match http {
        Some(http) if http.is_timeout() => "timeout",
        Some(http) if http.status().is_some() => "http_status",
        Some(http) if http.is_decode() => "response_invalid",
        Some(http) if http.is_connect() => "connect",
        Some(_) => "transport",
        None => match find_source::<FxRateQuoteProviderError>(error) {
            Some(FxRateQuoteProviderError::InvalidResponse { .. }) => "response_invalid",
            Some(FxRateQuoteProviderError::RequestFailed { .. }) | None => UNCLASSIFIED_FAILURE,
        },
    };
    FailureDiagnostic::new(
        "quote_fetch",
        Dependency::FxRatesApi,
        category,
        http.and_then(reqwest::Error::status)
            .map(|status| status.as_u16()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::box_error;
    use fxrate_service::CaptureFxRateSnapshotResult;
    use std::sync::Mutex;
    use test_api::CapturedLogs;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const SENTINEL: &str = "SENTINEL_PAYLOAD_TOKEN_person@example.com";

    struct Failing(Mutex<Option<CaptureFxRateSnapshotError>>);

    #[async_trait::async_trait]
    impl CaptureFxRateSnapshotUseCase for Failing {
        async fn execute(
            &self,
            _: &OperationContext,
            _: CaptureFxRateSnapshotCommand,
        ) -> Result<CaptureFxRateSnapshotResult, CaptureFxRateSnapshotError> {
            Err(self.0.lock().unwrap().take().expect("one execution"))
        }
    }

    #[test]
    fn should_identify_a_run_by_its_scheduled_hour_like_the_scheduler_event() {
        let date = time::Date::from_calendar_date(2026, time::Month::November, 17).unwrap();
        let at = |hour, minute, second, millisecond, offset_hours| {
            FxRefreshRun::at(
                date.with_hms_milli(hour, minute, second, millisecond)
                    .unwrap()
                    .assume_offset(time::UtcOffset::from_hms(offset_hours, 0, 0).unwrap()),
            )
        };
        let scheduled = at(6, 0, 0, 0, 0);
        let late_retry = at(6, 59, 59, 999, 0);

        assert_eq!("fxrate:2026-11-17T06:00:00Z", scheduled.source_event_id());
        assert_eq!(scheduled.source_event_id(), late_retry.source_event_id());
        assert_ne!(scheduled.run_id, late_retry.run_id);
        assert_eq!(
            "fxrate:2026-11-17T18:00:00Z",
            at(19, 30, 0, 0, 1).source_event_id()
        );
        assert_eq!(
            "fxrate:2026-11-17T07:00:00Z",
            at(7, 0, 0, 0, 0).source_event_id()
        );
        let context = scheduled.operation_context();
        assert!(matches!(context.principal, Principal::System));
        assert_eq!(scheduled.source_event_id(), context.correlation_id.as_str());
        assert_eq!(
            7,
            Uuid::parse_str(context.request_id.as_str())
                .unwrap()
                .get_version_num()
        );
    }

    /// The provider client error for one HTTP response with `status` and a sentinel body.
    async fn provider_status_error(status: u16) -> reqwest::Error {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            let response = format!(
                "HTTP/1.1 {status} failed\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{SENTINEL}",
                SENTINEL.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let error = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{address}/latest"))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap_err();
        server.await.unwrap();
        error
    }

    #[tokio::test]
    async fn should_record_the_failed_capture_stage_without_its_cause() {
        let cases = [
            (
                CaptureFxRateSnapshotError::QuoteFetchFailed {
                    source: box_error(FxRateQuoteProviderError::RequestFailed {
                        source: box_error(provider_status_error(429).await),
                    }),
                },
                ("quote_fetch", "fxratesapi", "http_status", Some(429)),
            ),
            (
                CaptureFxRateSnapshotError::QuoteFetchFailed {
                    source: box_error(FxRateQuoteProviderError::InvalidResponse {
                        source: application::error::static_error(SENTINEL),
                    }),
                },
                ("quote_fetch", "fxratesapi", "response_invalid", None),
            ),
            (
                CaptureFxRateSnapshotError::SnapshotInsertFailed {
                    source: box_error(sqlx::Error::PoolTimedOut),
                },
                ("snapshot_insert", "postgres", "pool_timed_out", None),
            ),
        ];
        for (error, (stage, dependency, category, http_status)) in cases {
            let snapshots = Failing(Mutex::new(Some(error)));
            let (logs, guard) = CapturedLogs::start();

            let result =
                capture_fx_rate_snapshot(&snapshots, &FxRefreshRun::at(OffsetDateTime::now_utc()))
                    .await;
            drop(guard);

            assert!(matches!(
                result,
                Err(OpsError::Failed {
                    command: COMMAND,
                    ..
                })
            ));
            let events = logs.events_where("event", "fxrate.snapshot.capture_failed");
            assert_eq!(1, events.len(), "{stage}");
            assert_eq!(Some(stage), events[0]["stage"].as_str());
            assert_eq!(Some(dependency), events[0]["dependency"].as_str());
            assert_eq!(Some(category), events[0]["error_category"].as_str());
            assert_eq!(
                http_status,
                events[0]
                    .get("http_status")
                    .and_then(|status| status.as_u64())
            );
            assert!(!logs.text().contains("SENTINEL"), "{stage}");
        }
    }
}
