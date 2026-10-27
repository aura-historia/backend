//! Native Lambda SQS batch mechanics. Business dispositions belong to the consuming adapter.

mod failure_custody;

pub use failure_custody::{
    DEAD_LETTER_QUEUE_URL_ENV, FailureCustody, FailureCustodyError, ParkedRecord,
    SqsDeadLetterCustody,
};

use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent, SqsMessage};
use futures_util::FutureExt;
use lambda_runtime::{Context, Error, LambdaEvent};
use platform_lambda_bootstrap::LambdaInvocationBudget;
use std::{
    collections::HashSet,
    future::Future,
    panic::AssertUnwindSafe,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub enum RecordOutcome<T> {
    Completed(T),
    MissingBody,
    InsufficientBudget,
    TimedOut,
    Panicked,
}

pub struct RecordAttempt<T> {
    pub message_id: String,
    pub outcome: RecordOutcome<T>,
    pub duration: Duration,
}

pub struct BatchResults<T> {
    pub record_count: usize,
    pub started_at: Instant,
    pub attempts: Vec<RecordAttempt<T>>,
}

impl<T> BatchResults<T> {
    /// The adapter decides which business outcomes are durably complete.
    pub fn finish(
        self,
        mut is_complete: impl FnMut(&RecordAttempt<T>) -> bool,
    ) -> SqsBatchResponse {
        let failures = self
            .attempts
            .iter()
            .filter(|attempt| !is_complete(attempt))
            .map(|attempt| failure(attempt.message_id.clone()))
            .collect();
        let mut response = SqsBatchResponse::default();
        response.batch_item_failures = failures;
        response
    }
}

fn failure(message_id: String) -> BatchItemFailure {
    let mut failure = BatchItemFailure::default();
    failure.item_identifier = message_id;
    failure
}

/// SQS supplies UUID message IDs; never interpolate arbitrary event strings into diagnostics.
/// The original ID is still returned unchanged in the partial-batch response.
pub fn safe_message_id(id: &str) -> Option<&str> {
    (!id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
    .then_some(id)
}

fn require_ids(event: &SqsEvent) -> Result<(), Error> {
    if event.records.iter().any(|record| {
        record
            .message_id
            .as_ref()
            .is_none_or(|id| id.trim().is_empty())
    }) {
        return Err(Error::from(
            "SQS event record has no message ID; fail whole invocation",
        ));
    }
    Ok(())
}

/// FIFO partial failures must identify each record uniquely. A malformed batch cannot be
/// represented truthfully by a partial response, so it must fail before setup or execution.
fn require_fifo_ids(event: &SqsEvent) -> Result<(), Error> {
    require_ids(event)?;
    if event.records.len() > 10 {
        return Err(Error::from("FIFO SQS batch exceeds ten records"));
    }
    let mut seen = HashSet::with_capacity(event.records.len());
    if event
        .records
        .iter()
        .any(|record| !seen.insert(record.message_id.as_deref().expect("IDs validated")))
    {
        return Err(Error::from("FIFO SQS batch contains duplicate message IDs"));
    }
    Ok(())
}

/// Setup failure must retain *every* record, not just the record currently being processed.
/// Without an ID a partial response cannot truthfully retain the batch, so fail the invocation.
pub fn retain_all_records(event: &LambdaEvent<SqsEvent>) -> Result<SqsBatchResponse, Error> {
    require_ids(&event.payload)?;
    let mut response = SqsBatchResponse::default();
    response.batch_item_failures = event
        .payload
        .records
        .iter()
        .map(|record| failure(record.message_id.clone().expect("IDs validated")))
        .collect();
    Ok(response)
}

/// Only a completed, successful setup can proceed to record handling. The helper does not
/// include setup errors or panic payloads in its own logs.
#[derive(Debug, PartialEq, Eq)]
pub enum SetupOutcome<T> {
    Ready(T),
    TimedOut,
    Failed,
    Panicked,
}

/// Bounds credential refresh and composition against the same invocation deadline as records.
/// Constructing the setup future happens inside the unwind guard, after checking the budget.
pub async fn run_setup_with_budget<T, E, F, Fut>(
    budget: &LambdaInvocationBudget,
    setup: F,
) -> SetupOutcome<T>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let remaining = budget.remaining();
    if remaining.is_zero() {
        return SetupOutcome::TimedOut;
    }
    match AssertUnwindSafe(async { tokio::time::timeout(remaining, setup()).await })
        .catch_unwind()
        .await
    {
        Ok(Ok(Ok(value))) => SetupOutcome::Ready(value),
        Ok(Ok(Err(_))) => SetupOutcome::Failed,
        Ok(Err(_)) => SetupOutcome::TimedOut,
        Err(_) => SetupOutcome::Panicked,
    }
}

/// Creates the Lambda budget at the invocation edge, validates IDs before any setup work,
/// and retains the entire batch if setup did not finish safely. The adapter still owns business
/// completion decisions; its setup errors and panic payloads are not included in these counters.
pub async fn handle_sqs_invocation<T, E, F, Fut, H, HFut>(
    event: LambdaEvent<SqsEvent>,
    component: &'static str,
    budget_from_context: impl FnOnce(&Context) -> LambdaInvocationBudget,
    setup: F,
    handle: H,
) -> Result<SqsBatchResponse, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    H: FnOnce(LambdaEvent<SqsEvent>, T, LambdaInvocationBudget) -> HFut,
    HFut: Future<Output = Result<SqsBatchResponse, Error>>,
{
    handle_invocation_with_validation(
        event,
        component,
        budget_from_context,
        require_ids,
        setup,
        handle,
    )
    .await
}

/// FIFO-specific entry point: reject missing, blank, or duplicate IDs (and batches over ten)
/// before credential refresh or composition. The Standard SQS entry point is unchanged.
pub async fn handle_fifo_sqs_invocation<T, E, F, Fut, H, HFut>(
    event: LambdaEvent<SqsEvent>,
    component: &'static str,
    budget_from_context: impl FnOnce(&Context) -> LambdaInvocationBudget,
    setup: F,
    handle: H,
) -> Result<SqsBatchResponse, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    H: FnOnce(LambdaEvent<SqsEvent>, T, LambdaInvocationBudget) -> HFut,
    HFut: Future<Output = Result<SqsBatchResponse, Error>>,
{
    handle_invocation_with_validation(
        event,
        component,
        budget_from_context,
        require_fifo_ids,
        setup,
        handle,
    )
    .await
}

async fn handle_invocation_with_validation<T, E, F, Fut, H, HFut>(
    event: LambdaEvent<SqsEvent>,
    component: &'static str,
    budget_from_context: impl FnOnce(&Context) -> LambdaInvocationBudget,
    validate: impl FnOnce(&SqsEvent) -> Result<(), Error>,
    setup: F,
    handle: H,
) -> Result<SqsBatchResponse, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    H: FnOnce(LambdaEvent<SqsEvent>, T, LambdaInvocationBudget) -> HFut,
    HFut: Future<Output = Result<SqsBatchResponse, Error>>,
{
    let budget = budget_from_context(&event.context);
    validate(&event.payload)?;
    let setup_outcome = run_setup_with_budget(&budget, setup).await;
    let outcome = match setup_outcome {
        SetupOutcome::Ready(value) => return handle(event, value, budget).await,
        SetupOutcome::TimedOut => "invocation_setup_timeout",
        SetupOutcome::Failed => "invocation_setup_failed",
        SetupOutcome::Panicked => "invocation_setup_panicked",
    };
    let response = retain_all_records(&event)?;
    tracing::warn!(
        component,
        outcome,
        sqs_message_count = event.payload.records.len(),
        failed_sqs_message_count = response.batch_item_failures.len(),
        "SQS Lambda setup retained every record for retry or redrive"
    );
    Ok(response)
}

/// How an attempted FIFO record leaves the queue. The consumer classifies each attempt; the
/// batch keeps message-group order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FifoDisposition {
    /// Confirmed complete: Lambda deletes the record.
    Complete,
    /// Can never succeed as delivered, so retrying would only hold its message group until the
    /// queue's redrive dead-letters it. A redelivered record moves to failure custody and is
    /// deleted once custody is confirmed, letting the rest of its group continue. A first
    /// delivery gets one retry, which absorbs a consumer deploy that lags its producer; it and
    /// any unconfirmed transfer are retained like [`FifoDisposition::Retain`].
    Park,
    /// Not confirmed complete: retain the record and every later record of its message group.
    Retain,
}

/// The partial-batch response of a FIFO invocation and the records it retained unstarted.
#[derive(Default)]
pub struct FifoBatchOutcome {
    pub response: SqsBatchResponse,
    /// Records retained without an attempt because an unfinished record of their group, or an
    /// exhausted budget, came first; in batch order. Every one is also a failure.
    pub unprocessed_message_ids: Vec<String>,
}

/// Process FIFO records one at a time. Once a record is neither confirmed complete nor in
/// confirmed failure custody, retain it and every later record of its message group without
/// starting them; records of other groups still run. SQS orders FIFO delivery only within a
/// group, and Lambda deletes exactly the records a partial response omits (the AWS Powertools
/// `skip_group_on_error` rule). A record without a group ID cannot be proven independent: it
/// blocks, and is blocked by, every group. Exhausting the invocation budget retains every
/// remaining record.
pub async fn process_fifo_batch<T, F, Fut, C>(
    event: LambdaEvent<SqsEvent>,
    remaining: Duration,
    per_record_cap: Duration,
    custody: &C,
    mut process: F,
    mut dispose: impl FnMut(&RecordAttempt<T>) -> FifoDisposition,
) -> Result<FifoBatchOutcome, Error>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = T>,
    C: FailureCustody,
{
    require_fifo_ids(&event.payload)?;
    let started_at = Instant::now();
    let mut outcome = FifoBatchOutcome::default();
    let mut retained = RetainedFifoGroups::default();
    for mut record in event.payload.records {
        let message_id = record.message_id.take().expect("IDs validated");
        let group_id = record
            .attributes
            .remove(MESSAGE_GROUP_ID_ATTRIBUTE)
            .filter(|group_id| !group_id.is_empty());
        if retained.blocks(group_id.as_deref()) {
            outcome
                .response
                .batch_item_failures
                .push(failure(message_id.clone()));
            outcome.unprocessed_message_ids.push(message_id);
            continue;
        }
        // The attempt consumes the body, so keep the custody copy first.
        let custody_copy = custody_copy(&record, &message_id, group_id.as_deref());
        let budget = per_record_cap.min(remaining.saturating_sub(started_at.elapsed()));
        let attempt = attempt_record(message_id, record.body, budget, &mut process).await;
        let acknowledged = match dispose(&attempt) {
            FifoDisposition::Complete => true,
            FifoDisposition::Retain => false,
            FifoDisposition::Park => {
                let budget = per_record_cap.min(remaining.saturating_sub(started_at.elapsed()));
                move_to_custody(custody, custody_copy, &attempt.message_id, budget).await
            }
        };
        if !acknowledged {
            if matches!(attempt.outcome, RecordOutcome::InsufficientBudget) {
                retained.retain_every_group();
            } else {
                retained.retain(group_id);
            }
            outcome
                .response
                .batch_item_failures
                .push(failure(attempt.message_id));
        }
    }
    Ok(outcome)
}

const MESSAGE_GROUP_ID_ATTRIBUTE: &str = "MessageGroupId";
const APPROXIMATE_RECEIVE_COUNT_ATTRIBUTE: &str = "ApproximateReceiveCount";

/// Only a redelivered record that custody can hold unchanged may be parked: an unreadable
/// receive count counts as a first delivery, and message attributes cannot be carried over.
fn custody_copy(
    record: &SqsMessage,
    message_id: &str,
    group_id: Option<&str>,
) -> Result<ParkedRecord, &'static str> {
    let redelivered = record
        .attributes
        .get(APPROXIMATE_RECEIVE_COUNT_ATTRIBUTE)
        .and_then(|count| count.parse::<u32>().ok())
        .is_some_and(|count| count >= 2);
    if !redelivered {
        return Err("first_delivery");
    }
    let group_id = group_id.ok_or("missing_message_group_id")?;
    if !record.message_attributes.is_empty() {
        return Err("message_attributes_present");
    }
    let body = record.body.as_ref().ok_or("missing_message_body")?;
    Ok(ParkedRecord {
        message_id: message_id.to_owned(),
        message_group_id: group_id.to_owned(),
        body: body.clone(),
    })
}

/// Whether the record may be acknowledged: only once custody of it is confirmed. Anything
/// else leaves it to the queue's native retry and redrive.
async fn move_to_custody<C: FailureCustody>(
    custody: &C,
    copy: Result<ParkedRecord, &'static str>,
    message_id: &str,
    budget: Duration,
) -> bool {
    let sqs_message_id = safe_message_id(message_id).unwrap_or("invalid");
    let record = match copy {
        Ok(record) if !budget.is_zero() => record,
        Ok(_) => return stay_in_source_queue(sqs_message_id, "insufficient_invocation_budget"),
        Err(reason) => return stay_in_source_queue(sqs_message_id, reason),
    };
    match tokio::time::timeout(budget, custody.park(&record)).await {
        Ok(Ok(custody_message_id)) => {
            tracing::warn!(
                sqs_message_id,
                custody_message_id = safe_message_id(&custody_message_id).unwrap_or("invalid"),
                outcome = "parked",
                "FIFO record moved to failure custody; its message group continues"
            );
            true
        }
        Ok(Err(_)) => stay_in_source_queue(sqs_message_id, "custody_transfer_unconfirmed"),
        Err(_) => stay_in_source_queue(sqs_message_id, "custody_transfer_timeout"),
    }
}

/// Logs why a record that should be parked stays in its source queue; it is not acknowledged.
fn stay_in_source_queue(sqs_message_id: &str, outcome: &'static str) -> bool {
    tracing::warn!(
        sqs_message_id,
        outcome,
        "FIFO record retained in its source queue instead of failure custody"
    );
    false
}

/// Message groups whose later records must wait for an unfinished predecessor.
#[derive(Default)]
struct RetainedFifoGroups {
    every_group: bool,
    groups: HashSet<String>,
}

impl RetainedFifoGroups {
    fn blocks(&self, group_id: Option<&str>) -> bool {
        self.every_group
            || match group_id {
                Some(group_id) => self.groups.contains(group_id),
                None => !self.groups.is_empty(),
            }
    }

    fn retain(&mut self, group_id: Option<String>) {
        match group_id {
            Some(group_id) => {
                self.groups.insert(group_id);
            }
            None => self.every_group = true,
        }
    }

    fn retain_every_group(&mut self) {
        self.every_group = true;
    }
}

/// Processes in event order, without prefetch. `remaining` is sampled after setup, and
/// decreases across the entire batch. Work not started before headroom expires is retained.
/// All IDs are checked *before* any work, so a later missing ID cannot partially commit work.
pub async fn process_batch<T, F, Fut>(
    event: LambdaEvent<SqsEvent>,
    remaining: Duration,
    per_record_cap: Duration,
    mut process: F,
) -> Result<BatchResults<T>, Error>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = T>,
{
    require_ids(&event.payload)?;
    let record_count = event.payload.records.len();
    let started_at = Instant::now();
    let mut attempts = Vec::with_capacity(record_count);
    for record in event.payload.records {
        let message_id = record.message_id.expect("IDs validated");
        let budget = per_record_cap.min(remaining.saturating_sub(started_at.elapsed()));
        attempts.push(attempt_record(message_id, record.body, budget, &mut process).await);
    }
    Ok(BatchResults {
        record_count,
        started_at,
        attempts,
    })
}

/// Runs one record within its budget; a zero budget never starts the work.
async fn attempt_record<T, F, Fut>(
    message_id: String,
    body: Option<String>,
    budget: Duration,
    process: &mut F,
) -> RecordAttempt<T>
where
    F: FnMut(String) -> Fut,
    Fut: Future<Output = T>,
{
    let record_started_at = Instant::now();
    let outcome = if budget.is_zero() {
        RecordOutcome::InsufficientBudget
    } else if let Some(body) = body {
        match AssertUnwindSafe(async { tokio::time::timeout(budget, process(body)).await })
            .catch_unwind()
            .await
        {
            Ok(Ok(value)) => RecordOutcome::Completed(value),
            Ok(Err(_)) => RecordOutcome::TimedOut,
            Err(_) => RecordOutcome::Panicked,
        }
    } else {
        RecordOutcome::MissingBody
    };
    RecordAttempt {
        message_id,
        outcome,
        duration: record_started_at.elapsed(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lambda_runtime::Context;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };

    fn epoch_millis() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    fn budget(usable: Duration) -> LambdaInvocationBudget {
        let mut context = Context::default();
        context.deadline = epoch_millis().saturating_add(5_000 + usable.as_millis() as u64);
        LambdaInvocationBudget::from_context(
            &context,
            Duration::from_secs(45),
            Duration::from_secs(5),
        )
    }

    fn event(records: &[(Option<&str>, Option<&str>)]) -> LambdaEvent<SqsEvent> {
        let mut payload = SqsEvent::default();
        payload.records = records
            .iter()
            .map(|(id, body)| {
                let mut message = SqsMessage::default();
                message.message_id = id.map(str::to_owned);
                message.body = body.map(str::to_owned);
                message
            })
            .collect();
        LambdaEvent::new(payload, Context::default())
    }

    fn grouped_event(records: &[(&str, &str, &str)]) -> LambdaEvent<SqsEvent> {
        let mut payload = SqsEvent::default();
        payload.records = records
            .iter()
            .map(|(id, group_id, body)| {
                let mut message = SqsMessage::default();
                message.message_id = Some((*id).to_owned());
                message.body = Some((*body).to_owned());
                message
                    .attributes
                    .insert("MessageGroupId".to_owned(), (*group_id).to_owned());
                message
            })
            .collect();
        LambdaEvent::new(payload, Context::default())
    }

    fn failed(response: SqsBatchResponse) -> Vec<String> {
        response
            .batch_item_failures
            .into_iter()
            .map(|f| f.item_identifier)
            .collect()
    }

    #[test]
    fn only_bounded_sqs_identifiers_are_written_to_diagnostics() {
        for id in ["sqs-1", "123e4567-e89b-12d3-a456-426614174000"] {
            assert_eq!(safe_message_id(id), Some(id));
        }
        for id in ["", "a\nsecret", "arn:secret", "with space"] {
            assert_eq!(safe_message_id(id), None);
        }
        assert_eq!(safe_message_id(&"x".repeat(129)), None);
    }

    fn complete_when_true(attempt: &RecordAttempt<bool>) -> FifoDisposition {
        if matches!(attempt.outcome, RecordOutcome::Completed(true)) {
            FifoDisposition::Complete
        } else {
            FifoDisposition::Retain
        }
    }

    #[tokio::test]
    async fn fifo_stops_at_first_unfinished_record_and_returns_the_whole_suffix() {
        for (bad_body, expected_calls) in [(None, 1), (Some("retry"), 2)] {
            let calls = Arc::new(AtomicUsize::new(0));
            let observed = calls.clone();
            let response = process_fifo_batch(
                event(&[
                    (Some("first"), Some("ok")),
                    (Some("second"), bad_body),
                    (Some("third"), Some("ok")),
                ]),
                Duration::from_secs(1),
                Duration::from_secs(1),
                &RecordingCustody::default(),
                move |body| {
                    let calls = observed.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        body == "ok"
                    }
                },
                complete_when_true,
            )
            .await
            .unwrap()
            .response;
            assert_eq!(failed(response), ["second", "third"]);
            assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
        }
    }

    #[tokio::test]
    async fn should_retain_only_the_unfinished_group_when_records_carry_group_ids() {
        let started = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = started.clone();
        let outcome = process_fifo_batch(
            grouped_event(&[
                ("a1", "a", "ok"),
                ("a2", "a", "retry"),
                ("b1", "b", "ok"),
                ("a3", "a", "ok"),
                ("b2", "b", "ok"),
            ]),
            Duration::from_secs(1),
            Duration::from_secs(1),
            &RecordingCustody::default(),
            move |body| {
                let started = observed.clone();
                async move {
                    started.lock().unwrap().push(body.clone());
                    body == "ok"
                }
            },
            complete_when_true,
        )
        .await
        .unwrap();

        assert_eq!(failed(outcome.response), ["a2", "a3"]);
        assert_eq!(outcome.unprocessed_message_ids, ["a3"]);
        assert_eq!(*started.lock().unwrap(), ["ok", "retry", "ok", "ok"]);
    }

    #[tokio::test]
    async fn should_retain_only_the_timed_out_group_while_budget_remains() {
        let outcome = process_fifo_batch(
            grouped_event(&[("a1", "a", "wait"), ("b1", "b", "ok"), ("a2", "a", "ok")]),
            Duration::from_secs(1),
            Duration::from_millis(5),
            &RecordingCustody::default(),
            |body| async move {
                if body == "wait" {
                    std::future::pending::<bool>().await
                } else {
                    true
                }
            },
            complete_when_true,
        )
        .await
        .unwrap();

        assert_eq!(failed(outcome.response), ["a1", "a2"]);
        assert_eq!(outcome.unprocessed_message_ids, ["a2"]);
    }

    #[tokio::test]
    async fn should_treat_records_without_group_ids_as_dependent_on_every_group() {
        let mut after_grouped_failure = grouped_event(&[("a1", "a", "retry"), ("b1", "b", "ok")]);
        after_grouped_failure.payload.records[1].attributes.clear();
        let mut after_ungrouped_failure = grouped_event(&[("x1", "x", "retry"), ("b1", "b", "ok")]);
        after_ungrouped_failure.payload.records[0]
            .attributes
            .clear();

        for event in [after_grouped_failure, after_ungrouped_failure] {
            let outcome = process_fifo_batch(
                event,
                Duration::from_secs(1),
                Duration::from_secs(1),
                &RecordingCustody::default(),
                |body| async move { body == "ok" },
                complete_when_true,
            )
            .await
            .unwrap();

            assert_eq!(outcome.unprocessed_message_ids, ["b1"]);
            assert_eq!(failed(outcome.response).len(), 2);
        }
    }

    #[tokio::test]
    async fn should_retain_every_group_once_the_invocation_budget_is_exhausted() {
        let outcome = process_fifo_batch(
            grouped_event(&[("a1", "a", "wait"), ("b1", "b", "ok"), ("c1", "c", "ok")]),
            Duration::from_millis(5),
            Duration::from_secs(1),
            &RecordingCustody::default(),
            |body| async move {
                if body == "wait" {
                    std::future::pending::<bool>().await
                } else {
                    true
                }
            },
            complete_when_true,
        )
        .await
        .unwrap();

        assert_eq!(failed(outcome.response), ["a1", "b1", "c1"]);
        assert_eq!(outcome.unprocessed_message_ids, ["c1"]);
    }

    #[tokio::test]
    async fn fifo_all_complete_and_empty_batches_acknowledge() {
        for records in [
            vec![],
            vec![(Some("a"), Some("ok")), (Some("b"), Some("ok"))],
        ] {
            let response = process_fifo_batch(
                event(&records),
                Duration::from_secs(1),
                Duration::from_secs(1),
                &RecordingCustody::default(),
                |_| async { true },
                complete_when_true,
            )
            .await
            .unwrap()
            .response;
            assert!(failed(response).is_empty());
        }
    }

    #[tokio::test]
    async fn fifo_timeout_panic_and_exhaustion_block_successors() {
        for mode in ["wait", "panic", "expired"] {
            let calls = Arc::new(AtomicUsize::new(0));
            let observed = calls.clone();
            let response = process_fifo_batch(
                event(&[(Some("bad"), Some(mode)), (Some("next"), Some("ok"))]),
                if mode == "expired" {
                    Duration::ZERO
                } else {
                    Duration::from_secs(1)
                },
                Duration::from_millis(5),
                &RecordingCustody::default(),
                move |body| {
                    let calls = observed.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        match body.as_str() {
                            "wait" => std::future::pending::<bool>().await,
                            "panic" => panic!("private record detail"),
                            _ => true,
                        }
                    }
                },
                complete_when_true,
            )
            .await
            .unwrap()
            .response;
            assert_eq!(failed(response), ["bad", "next"], "{mode}");
            assert_eq!(calls.load(Ordering::SeqCst), usize::from(mode != "expired"));
        }
    }

    #[tokio::test]
    async fn fifo_invalid_ids_and_oversized_batch_fail_before_setup_or_execution() {
        let duplicate = [(Some("a"), Some("ok")), (Some("a"), Some("ok"))];
        let missing = [(Some("a"), Some("ok")), (None, Some("ok"))];
        let blank = [(Some("a"), Some("ok")), (Some("  "), Some("ok"))];
        let oversized = vec![(Some("a"), Some("ok")); 11];
        for records in [&duplicate[..], &missing[..], &blank[..], &oversized[..]] {
            let calls = Arc::new(AtomicUsize::new(0));
            let observed = calls.clone();
            assert!(
                handle_fifo_sqs_invocation(
                    event(records),
                    "test",
                    |_| budget(Duration::from_secs(1)),
                    move || {
                        observed.fetch_add(1, Ordering::SeqCst);
                        async { Ok::<_, ()>(()) }
                    },
                    |_, _, _| async { panic!("invalid batch must not reach records") },
                )
                .await
                .is_err()
            );
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert!(
                process_fifo_batch(
                    event(records),
                    Duration::from_secs(1),
                    Duration::from_secs(1),
                    &RecordingCustody::default(),
                    |_| async { true },
                    complete_when_true,
                )
                .await
                .is_err()
            );
        }
    }

    #[tokio::test]
    async fn fifo_setup_failure_retries_every_record() {
        let response = handle_fifo_sqs_invocation(
            event(&[(Some("a"), Some("ok")), (Some("b"), Some("ok"))]),
            "test",
            |_| budget(Duration::from_millis(10)),
            || async { std::future::pending::<Result<(), ()>>().await },
            |_, _, _| async { panic!("setup never completed") },
        )
        .await
        .unwrap();
        assert_eq!(failed(response), ["a", "b"]);
    }

    #[tokio::test]
    async fn sequential_partial_response_preserves_order_and_retry_decisions() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let results = process_batch(
            event(&[
                (Some("one"), Some("ok")),
                (Some("two"), None),
                (Some("three"), Some("retry")),
                (Some("four"), Some("ok")),
            ]),
            Duration::from_secs(1),
            Duration::from_secs(1),
            move |body| {
                let calls = observed.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    body == "ok"
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(results.record_count, 4);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(
            failed(
                results.finish(|attempt| matches!(attempt.outcome, RecordOutcome::Completed(true)))
            ),
            ["two", "three"]
        );
    }

    #[tokio::test]
    async fn missing_late_id_fails_before_any_processing_and_setup_failure_fails_all() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let bad = event(&[(Some("first"), Some("work")), (None, Some("work"))]);
        assert!(
            process_batch(
                bad,
                Duration::from_secs(1),
                Duration::from_secs(1),
                move |_| {
                    let calls = observed.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                    }
                }
            )
            .await
            .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(retain_all_records(&event(&[(Some("first"), None), (None, None)])).is_err());
        assert_eq!(
            failed(retain_all_records(&event(&[(Some("a"), None), (Some("b"), None)])).unwrap()),
            ["a", "b"]
        );
    }

    #[tokio::test]
    async fn rejects_blank_and_whitespace_ids_before_work_or_setup_retention() {
        for invalid in ["", "  \t "] {
            let calls = Arc::new(AtomicUsize::new(0));
            let observed = calls.clone();
            let records = [(Some("first"), Some("work")), (Some(invalid), Some("work"))];
            assert!(
                process_batch(
                    event(&records),
                    Duration::from_secs(1),
                    Duration::from_secs(1),
                    move |_| {
                        let calls = observed.clone();
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                )
                .await
                .is_err()
            );
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert!(retain_all_records(&event(&records)).is_err());
            let setup_calls = calls.clone();
            assert!(
                handle_sqs_invocation(
                    event(&records),
                    "test",
                    |_| budget(Duration::from_secs(1)),
                    move || {
                        let calls = setup_calls.clone();
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            Ok::<_, ()>(())
                        }
                    },
                    |_, _, _| async { Ok(SqsBatchResponse::default()) }
                )
                .await
                .is_err()
            );
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn setup_success_passes_remaining_shared_budget_to_records() {
        let response = handle_sqs_invocation(
            event(&[(Some("first"), Some("work"))]),
            "test",
            |_| budget(Duration::from_millis(200)),
            || async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                Ok::<_, ()>(7)
            },
            |event, value, budget| async move {
                assert_eq!(value, 7);
                assert!(budget.remaining() < Duration::from_millis(200));
                let results = process_batch(
                    event,
                    budget.remaining(),
                    Duration::from_millis(100),
                    |_| async { true },
                )
                .await?;
                Ok(results
                    .finish(|attempt| matches!(attempt.outcome, RecordOutcome::Completed(true))))
            },
        )
        .await
        .unwrap();
        assert!(failed(response).is_empty());
    }

    #[tokio::test]
    async fn setup_wait_timeout_error_and_panic_retain_every_id() {
        let records = [
            (Some("first"), Some("work")),
            (Some("second"), Some("work")),
        ];
        for mode in ["wait", "error", "panic", "construction_panic"] {
            let response = handle_sqs_invocation(
                event(&records),
                "test",
                |_| budget(Duration::from_millis(20)),
                move || {
                    if mode == "construction_panic" {
                        panic!("sensitive setup detail");
                    }
                    async move {
                        match mode {
                            "wait" => std::future::pending::<Result<(), ()>>().await,
                            "error" => Err(()),
                            "panic" => panic!("sensitive setup detail"),
                            _ => Ok(()),
                        }
                    }
                },
                |_, _, _| async { panic!("unfinished setup must not reach handler") },
            )
            .await
            .unwrap();
            assert_eq!(failed(response), ["first", "second"], "{mode}");
        }
    }

    #[tokio::test]
    async fn setup_failure_categories_distinguish_error_timeout_and_panic() {
        assert_eq!(
            run_setup_with_budget(&budget(Duration::from_secs(1)), || async {
                Err::<(), _>("private setup error")
            })
            .await,
            SetupOutcome::Failed
        );
        assert_eq!(
            run_setup_with_budget(&budget(Duration::from_secs(1)), || async {
                panic!("private setup panic");
                #[allow(unreachable_code)]
                Ok::<(), ()>(())
            })
            .await,
            SetupOutcome::Panicked
        );
        assert_eq!(
            run_setup_with_budget(&budget(Duration::from_secs(1)), || {
                panic!("private construction panic");
                #[allow(unreachable_code)]
                async {
                    Ok::<(), ()>(())
                }
            })
            .await,
            SetupOutcome::Panicked
        );
        assert_eq!(
            run_setup_with_budget(&budget(Duration::from_millis(10)), || async {
                std::future::pending::<Result<(), ()>>().await
            })
            .await,
            SetupOutcome::TimedOut
        );
    }

    #[tokio::test]
    async fn expired_budget_does_not_construct_or_poll_setup() {
        let mut context = Context::default();
        context.deadline = epoch_millis();
        let mut payload = event(&[(Some("first"), Some("work"))]);
        payload.context = context;
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let response = handle_sqs_invocation(
            payload,
            "test",
            |context| {
                LambdaInvocationBudget::from_context(
                    context,
                    Duration::from_secs(45),
                    Duration::from_secs(5),
                )
            },
            move || {
                observed.fetch_add(1, Ordering::SeqCst);
                async { Ok::<_, ()>(()) }
            },
            |_, _, _| async { panic!("expired budget must not run records") },
        )
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(failed(response), ["first"]);
    }

    #[tokio::test]
    async fn nearly_expired_budget_never_starts_late_setup() {
        let near_deadline = budget(Duration::from_millis(2));
        tokio::time::sleep(Duration::from_millis(5)).await;
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let outcome = run_setup_with_budget(&near_deadline, move || {
            observed.fetch_add(1, Ordering::SeqCst);
            async { Ok::<_, ()>(()) }
        })
        .await;
        assert_eq!(outcome, SetupOutcome::TimedOut);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn timeout_panic_and_exhausted_budget_are_independent_failures() {
        let results = process_batch(
            event(&[
                (Some("timeout"), Some("wait")),
                (Some("panic"), Some("panic")),
                (Some("good"), Some("good")),
            ]),
            Duration::from_millis(100),
            Duration::from_millis(5),
            |body| async move {
                if body == "wait" {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                if body == "panic" {
                    panic!("test panic");
                }
                body
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            results.attempts[0].outcome,
            RecordOutcome::TimedOut
        ));
        assert!(matches!(
            results.attempts[1].outcome,
            RecordOutcome::Panicked
        ));
        assert_eq!(
            failed(
                results
                    .finish(|a| matches!(&a.outcome, RecordOutcome::Completed(s) if s == "good"))
            ),
            ["timeout", "panic"]
        );

        let results = process_batch(
            event(&[(Some("a"), Some("x")), (Some("b"), None)]),
            Duration::ZERO,
            Duration::from_secs(1),
            |_| async { panic!("must not run") },
        )
        .await
        .unwrap();
        assert!(
            results
                .attempts
                .iter()
                .all(|a| matches!(a.outcome, RecordOutcome::InsufficientBudget))
        );
        assert_eq!(failed(results.finish(|_| false)), ["a", "b"]);
    }

    #[tokio::test]
    async fn elapsed_batch_budget_does_not_reset_for_later_records() {
        let results = process_batch(
            event(&[(Some("first"), Some("x")), (Some("second"), Some("x"))]),
            Duration::from_millis(10),
            Duration::from_millis(20),
            |_| async {
                tokio::time::sleep(Duration::from_millis(50)).await;
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            results.attempts[0].outcome,
            RecordOutcome::TimedOut
        ));
        assert!(matches!(
            results.attempts[1].outcome,
            RecordOutcome::InsufficientBudget
        ));
    }

    #[derive(Default)]
    struct RecordingCustody {
        parked: std::sync::Mutex<Vec<ParkedRecord>>,
        unconfirmed: bool,
    }

    impl RecordingCustody {
        fn parked_ids(&self) -> Vec<String> {
            self.parked
                .lock()
                .unwrap()
                .iter()
                .map(|record| record.message_id.clone())
                .collect()
        }
    }

    impl FailureCustody for RecordingCustody {
        async fn park(&self, record: &ParkedRecord) -> Result<String, FailureCustodyError> {
            if self.unconfirmed {
                return Err(FailureCustodyError::MissingMessageId);
            }
            self.parked.lock().unwrap().push(record.clone());
            Ok(format!("custody-{}", record.message_id))
        }
    }

    struct PendingCustody;

    impl FailureCustody for PendingCustody {
        async fn park(&self, _: &ParkedRecord) -> Result<String, FailureCustodyError> {
            std::future::pending().await
        }
    }

    /// Records as `(message ID, group ID, body, ApproximateReceiveCount)`.
    fn delivered_event(records: &[(&str, &str, &str, u32)]) -> LambdaEvent<SqsEvent> {
        let mut event = grouped_event(
            &records
                .iter()
                .map(|(id, group_id, body, _)| (*id, *group_id, *body))
                .collect::<Vec<_>>(),
        );
        for (record, (_, _, _, receive_count)) in event.payload.records.iter_mut().zip(records) {
            record.attributes.insert(
                "ApproximateReceiveCount".to_owned(),
                receive_count.to_string(),
            );
        }
        event
    }

    /// `poison` can never succeed, `ok` completes and anything else stays unfinished.
    fn park_poison(attempt: &RecordAttempt<String>) -> FifoDisposition {
        match &attempt.outcome {
            RecordOutcome::Completed(body) if body == "ok" => FifoDisposition::Complete,
            RecordOutcome::Completed(body) if body == "poison" => FifoDisposition::Park,
            _ => FifoDisposition::Retain,
        }
    }

    async fn run_parking_batch(
        event: LambdaEvent<SqsEvent>,
        custody: &impl FailureCustody,
        per_record_cap: Duration,
    ) -> (FifoBatchOutcome, Vec<String>) {
        let started = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = started.clone();
        let outcome = process_fifo_batch(
            event,
            Duration::from_secs(1),
            per_record_cap,
            custody,
            move |body| {
                let started = observed.clone();
                async move {
                    started.lock().unwrap().push(body.clone());
                    body
                }
            },
            park_poison,
        )
        .await
        .unwrap();
        let started = started.lock().unwrap().clone();
        (outcome, started)
    }

    #[tokio::test]
    async fn should_run_co_batched_successors_on_the_redelivery_that_parks_their_poisoned_head() {
        let custody = RecordingCustody::default();
        let first = [
            ("a1", "a", "poison", 1),
            ("a2", "a", "ok", 1),
            ("b1", "b", "ok", 1),
        ];
        let (outcome, started) =
            run_parking_batch(delivered_event(&first), &custody, Duration::from_secs(1)).await;
        assert_eq!(failed(outcome.response), ["a1", "a2"]);
        assert_eq!(outcome.unprocessed_message_ids, ["a2"]);
        assert_eq!(started, ["poison", "ok"]);
        assert!(custody.parked_ids().is_empty());

        let redelivered = [("a1", "a", "poison", 2), ("a2", "a", "ok", 2)];
        let (outcome, started) = run_parking_batch(
            delivered_event(&redelivered),
            &custody,
            Duration::from_secs(1),
        )
        .await;
        assert!(failed(outcome.response).is_empty());
        assert_eq!(started, ["poison", "ok"]);
        assert_eq!(
            *custody.parked.lock().unwrap(),
            [ParkedRecord {
                message_id: "a1".to_owned(),
                message_group_id: "a".to_owned(),
                body: "poison".to_owned(),
            }]
        );
    }

    #[tokio::test]
    async fn should_retain_the_group_when_a_record_cannot_move_to_custody_unchanged() {
        let mut ungrouped = delivered_event(&[("a1", "a", "poison", 2), ("b1", "b", "ok", 2)]);
        ungrouped.payload.records[0]
            .attributes
            .remove("MessageGroupId");
        let mut attributed = delivered_event(&[("a1", "a", "poison", 2), ("a2", "a", "ok", 2)]);
        attributed.payload.records[0]
            .message_attributes
            .insert("trace".to_owned(), Default::default());
        let mut uncounted = delivered_event(&[("a1", "a", "poison", 2), ("a2", "a", "ok", 2)]);
        uncounted.payload.records[0]
            .attributes
            .insert("ApproximateReceiveCount".to_owned(), "two".to_owned());

        for event in [ungrouped, attributed, uncounted] {
            let custody = RecordingCustody::default();
            let (outcome, started) =
                run_parking_batch(event, &custody, Duration::from_secs(1)).await;
            assert_eq!(failed(outcome.response).len(), 2);
            assert_eq!(started, ["poison"]);
            assert!(custody.parked_ids().is_empty());
        }
    }

    #[tokio::test]
    async fn should_retain_the_group_when_custody_is_unconfirmed_or_times_out() {
        let event = || delivered_event(&[("a1", "a", "poison", 3), ("a2", "a", "ok", 3)]);
        let unconfirmed = RecordingCustody {
            unconfirmed: true,
            ..Default::default()
        };
        let (outcome, _) = run_parking_batch(event(), &unconfirmed, Duration::from_secs(1)).await;
        assert_eq!(failed(outcome.response), ["a1", "a2"]);
        assert_eq!(outcome.unprocessed_message_ids, ["a2"]);

        let (outcome, _) =
            run_parking_batch(event(), &PendingCustody, Duration::from_millis(5)).await;
        assert_eq!(failed(outcome.response), ["a1", "a2"]);
        assert_eq!(outcome.unprocessed_message_ids, ["a2"]);
    }
}
