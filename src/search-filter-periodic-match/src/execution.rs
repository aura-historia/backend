use search_filter_service::use_cases::{
    RunPeriodicSearchFilterMatchingCommand, RunPeriodicSearchFilterMatchingOutcome,
    RunPeriodicSearchFilterMatchingUseCase,
};
use std::{future::Future, sync::Arc, time::Duration};
use time::OffsetDateTime;
use tokio::time::{Instant, sleep};

pub(crate) const STARTUP_LIMIT: Duration = Duration::from_secs(120);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ExecutionError {
    StartupFailed,
    StartupTimedOut,
    Cancelled,
    MatchingTimedOut,
    FailedFilters(usize),
    ServiceFailed,
    Panicked,
    TaskFailed,
}

pub(crate) async fn run<S, F, E>(
    startup: F,
    shutdown: S,
    startup_limit: Duration,
) -> Result<(), ExecutionError>
where
    S: Future<Output = ()>,
    F: Future<Output = Result<(Arc<dyn RunPeriodicSearchFilterMatchingUseCase>, Duration), E>>,
{
    tokio::pin!(shutdown);
    let started = Instant::now();
    let startup_deadline = started + startup_limit;
    // Bias toward shutdown/deadline when readiness and cancellation coincide.
    let (use_case, max_run_duration) = tokio::select! {
        biased;
        () = &mut shutdown => return Err(ExecutionError::Cancelled),
        () = tokio::time::sleep_until(startup_deadline) => return Err(ExecutionError::StartupTimedOut),
        result = startup => result.map_err(|_| ExecutionError::StartupFailed)?,
    };
    if Instant::now() >= startup_deadline {
        return Err(ExecutionError::StartupTimedOut);
    }
    // Poll the already-registered signal before launching even if startup just completed.
    tokio::select! {
        biased;
        () = &mut shutdown => return Err(ExecutionError::Cancelled),
        () = tokio::task::yield_now() => {}
    }
    let matching_started = Instant::now();
    let mut matching = tokio::spawn(async move {
        use_case
            .execute(RunPeriodicSearchFilterMatchingCommand {
                started_at: OffsetDateTime::now_utc(),
            })
            .await
    });
    let result = tokio::select! {
        biased;
        () = &mut shutdown => {
            matching.abort();
            let _ = matching.await;
            Err(ExecutionError::Cancelled)
        }
        () = sleep(max_run_duration) => {
            matching.abort();
            let _ = matching.await;
            Err(ExecutionError::MatchingTimedOut)
        }
        joined = &mut matching => match joined {
            Ok(Ok(RunPeriodicSearchFilterMatchingOutcome::SkippedAlreadyRunning)) => {
                tracing::info!(outcome = "excluded", "cron.matching.completed");
                Ok(())
            }
            Ok(Ok(RunPeriodicSearchFilterMatchingOutcome::Applied(report))) => {
                tracing::info!(outcome = if report.filters_failed == 0 { "applied" } else { "incomplete" },
                    window_end = %report.window_end, filters_selected = report.filters_selected,
                    filters_completed = report.filters_completed, filters_failed = report.filters_failed,
                    "cron.matching.report");
                if report.filters_failed == 0 { Ok(()) } else { Err(ExecutionError::FailedFilters(report.filters_failed)) }
            }
            Ok(Err(_)) => Err(ExecutionError::ServiceFailed),
            Err(error) if error.is_panic() => Err(ExecutionError::Panicked),
            Err(_) => Err(ExecutionError::TaskFailed),
        }
    };
    tracing::info!(
        duration_ms = matching_started.elapsed().as_millis(),
        "cron.matching.finished"
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use search_filter_service::use_cases::{
        PeriodicSearchFilterMatchingReport, RunPeriodicSearchFilterMatchingError,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fake {
        calls: Arc<AtomicUsize>,
        result: Option<RunPeriodicSearchFilterMatchingOutcome>,
        block: bool,
        dropped: Option<Arc<AtomicUsize>>,
    }
    struct DropCount(Arc<AtomicUsize>);
    impl Drop for DropCount {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    #[async_trait]
    impl RunPeriodicSearchFilterMatchingUseCase for Fake {
        async fn execute(
            &self,
            _: RunPeriodicSearchFilterMatchingCommand,
        ) -> Result<RunPeriodicSearchFilterMatchingOutcome, RunPeriodicSearchFilterMatchingError>
        {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let _guard = self.dropped.as_ref().map(|count| DropCount(count.clone()));
            if self.block {
                std::future::pending::<()>().await;
            }
            self.result
                .clone()
                .ok_or(RunPeriodicSearchFilterMatchingError::InvalidPolicy)
        }
    }
    fn report(failed: usize) -> PeriodicSearchFilterMatchingReport {
        PeriodicSearchFilterMatchingReport {
            window_end: OffsetDateTime::UNIX_EPOCH,
            filters_selected: 0,
            filters_completed: 0,
            filters_already_covered: 0,
            filters_changed_or_inactive: 0,
            filters_progress_superseded: 0,
            filter_attempts: 0,
            filters_retried: 0,
            filters_failed: failed,
            filters_invalid_persisted_state: 0,
            candidates_scanned: 0,
            candidates_existing: 0,
            candidates_missing_source: 0,
            candidates_stale: 0,
            candidates_withdrawn: 0,
            candidates_rejected: 0,
            permanent_evaluation_failures: 0,
            retryable_evaluation_failures: 0,
            matches_inserted: 0,
            matches_duplicate: 0,
        }
    }
    async fn fake(
        result: Option<RunPeriodicSearchFilterMatchingOutcome>,
        block: bool,
        dropped: Option<Arc<AtomicUsize>>,
    ) -> (
        Arc<dyn RunPeriodicSearchFilterMatchingUseCase>,
        Arc<AtomicUsize>,
    ) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(Fake {
                calls: calls.clone(),
                result,
                block,
                dropped,
            }),
            calls,
        )
    }
    #[tokio::test]
    async fn maps_reports_overlap_and_service_errors_once() {
        for (result, expected) in [
            (
                Some(RunPeriodicSearchFilterMatchingOutcome::Applied(report(0))),
                Ok(()),
            ),
            (
                Some(RunPeriodicSearchFilterMatchingOutcome::Applied(report(1))),
                Err(ExecutionError::FailedFilters(1)),
            ),
            (
                Some(RunPeriodicSearchFilterMatchingOutcome::SkippedAlreadyRunning),
                Ok(()),
            ),
            (None, Err(ExecutionError::ServiceFailed)),
        ] {
            let (job, calls) = fake(result, false, None).await;
            assert_eq!(
                run(
                    async { Ok::<_, ()>((job, Duration::from_secs(1))) },
                    std::future::pending(),
                    STARTUP_LIMIT
                )
                .await,
                expected
            );
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }
    #[tokio::test]
    async fn startup_timeout_and_cancel_do_not_launch() {
        let (job, calls) = fake(None, false, None).await;
        assert_eq!(
            run(
                std::future::pending::<Result<_, ()>>(),
                std::future::pending(),
                Duration::from_millis(1)
            )
            .await,
            Err(ExecutionError::StartupTimedOut)
        );
        assert_eq!(
            run(
                async { Ok::<_, ()>((job, Duration::from_secs(1))) },
                async {},
                STARTUP_LIMIT
            )
            .await,
            Err(ExecutionError::Cancelled)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn startup_cancellation_drops_startup_and_prevents_launch() {
        let dropped = Arc::new(AtomicUsize::new(0));

        let started = tokio::sync::Notify::new();
        let guard = dropped.clone();
        let startup = async {
            let _guard = DropCount(guard);
            started.notify_one();
            std::future::pending::<
                Result<(Arc<dyn RunPeriodicSearchFilterMatchingUseCase>, Duration), ()>,
            >()
            .await
        };
        let result = run(
            startup,
            async {
                started.notified().await;
            },
            STARTUP_LIMIT,
        )
        .await;
        assert_eq!(result, Err(ExecutionError::Cancelled));
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn panic_maps_to_failure() {
        struct Panicking;
        #[async_trait]
        impl RunPeriodicSearchFilterMatchingUseCase for Panicking {
            async fn execute(
                &self,
                _: RunPeriodicSearchFilterMatchingCommand,
            ) -> Result<RunPeriodicSearchFilterMatchingOutcome, RunPeriodicSearchFilterMatchingError>
            {
                panic!("sensitive test payload")
            }
        }
        assert_eq!(
            run(
                async {
                    Ok::<_, ()>((
                        Arc::new(Panicking) as Arc<dyn RunPeriodicSearchFilterMatchingUseCase>,
                        Duration::from_secs(1),
                    ))
                },
                std::future::pending(),
                STARTUP_LIMIT
            )
            .await,
            Err(ExecutionError::Panicked)
        );
    }

    #[tokio::test]
    async fn timeout_and_cancellation_abort_and_join_matching() {
        for cancel in [false, true] {
            let dropped = Arc::new(AtomicUsize::new(0));
            let (job, calls) = fake(None, true, Some(dropped.clone())).await;
            let shutdown = async move {
                if cancel {
                    // Allow the matching task to start before signalling shutdown.
                    while calls.load(Ordering::SeqCst) == 0 {
                        tokio::task::yield_now().await;
                    }
                } else {
                    std::future::pending::<()>().await;
                }
            };
            let result = run(
                async { Ok::<_, ()>((job, Duration::from_millis(10))) },
                shutdown,
                STARTUP_LIMIT,
            )
            .await;
            assert_eq!(
                result,
                Err(if cancel {
                    ExecutionError::Cancelled
                } else {
                    ExecutionError::MatchingTimedOut
                })
            );
            assert_eq!(dropped.load(Ordering::SeqCst), 1);
        }
    }
}
