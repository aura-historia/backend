//! Native Lambda SQS batch mechanics. Business dispositions belong to the consuming adapter.

use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use futures_util::FutureExt;
use lambda_runtime::{Context, Error, LambdaEvent};
use platform_lambda_bootstrap::LambdaInvocationBudget;
use std::{
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
    let budget = budget_from_context(&event.context);
    require_ids(&event.payload)?;
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
        let record_started_at = Instant::now();
        let outcome = if budget.is_zero() {
            RecordOutcome::InsufficientBudget
        } else if let Some(body) = record.body {
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
        attempts.push(RecordAttempt {
            message_id,
            outcome,
            duration: record_started_at.elapsed(),
        });
    }
    Ok(BatchResults {
        record_count,
        started_at,
        attempts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lambda_events::sqs::SqsMessage;
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

    fn failed(response: SqsBatchResponse) -> Vec<String> {
        response
            .batch_item_failures
            .into_iter()
            .map(|f| f.item_identifier)
            .collect()
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
}
