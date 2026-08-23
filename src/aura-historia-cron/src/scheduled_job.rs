use async_trait::async_trait;
use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use time::OffsetDateTime;
use tokio::sync::{Mutex, Notify};
use tracing::{error, info, warn};

#[async_trait]
#[doc(hidden)]
pub trait CronJob: Send + Sync {
    fn name(&self) -> &'static str;
    async fn execute(&self) -> Result<(), CronJobExecutionError>;
}

#[derive(Debug, thiserror::Error)]
#[doc(hidden)]
pub enum CronJobExecutionError {
    #[error("cron job execution failed")]
    Failed(#[source] Box<dyn Error + Send + Sync>),
}

impl CronJobExecutionError {
    pub fn from_source(source: impl Error + Send + Sync + 'static) -> Self {
        Self::Failed(Box::new(source))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub enum CronJobStatus {
    NeverRun,
    Succeeded,
    Failed,
    Panicked,
    TimedOut,
    SkippedLocalOverlap,
    SkippedShutdown,
}

#[doc(hidden)]
pub struct ActiveExecutionTracker {
    accepting: AtomicBool,
    active: AtomicUsize,
    drained: Notify,
}

impl Default for ActiveExecutionTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ActiveExecutionTracker {
    pub fn new() -> Self {
        Self {
            accepting: AtomicBool::new(true),
            active: AtomicUsize::new(0),
            drained: Notify::new(),
        }
    }

    pub fn stop_accepting(&self) {
        self.accepting.store(false, Ordering::Release);
    }

    fn try_track(&self) -> Option<ActiveExecutionGuard<'_>> {
        if !self.accepting.load(Ordering::Acquire) {
            return None;
        }
        self.active.fetch_add(1, Ordering::AcqRel);
        if self.accepting.load(Ordering::Acquire) {
            Some(ActiveExecutionGuard { tracker: self })
        } else {
            self.release();
            None
        }
    }

    fn release(&self) {
        if self.active.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.drained.notify_waiters();
        }
    }

    pub async fn drain(&self, timeout: Duration) -> Result<(), CronDrainError> {
        let wait = async {
            loop {
                let notified = self.drained.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.active.load(Ordering::Acquire) == 0 {
                    return;
                }
                notified.await;
            }
        };
        tokio::time::timeout(timeout, wait)
            .await
            .map_err(|_| CronDrainError::TimedOut {
                active: self.active.load(Ordering::Acquire),
            })
    }
}

struct ActiveExecutionGuard<'a> {
    tracker: &'a ActiveExecutionTracker,
}
impl Drop for ActiveExecutionGuard<'_> {
    fn drop(&mut self) {
        self.tracker.release();
    }
}

#[derive(Debug, thiserror::Error)]
#[doc(hidden)]
pub enum CronDrainError {
    #[error("timed out draining {active} active cron executions")]
    TimedOut { active: usize },
}

#[derive(Debug)]
#[doc(hidden)]
pub enum CronJobExecutionOutcome {
    Succeeded,
    Failed(CronJobExecutionError),
    Panicked,
    TimedOut,
    SkippedLocalOverlap,
    SkippedShutdown,
}

impl CronJobExecutionOutcome {
    pub const fn status(&self) -> CronJobStatus {
        match self {
            Self::Succeeded => CronJobStatus::Succeeded,
            Self::Failed(_) => CronJobStatus::Failed,
            Self::Panicked => CronJobStatus::Panicked,
            Self::TimedOut => CronJobStatus::TimedOut,
            Self::SkippedLocalOverlap => CronJobStatus::SkippedLocalOverlap,
            Self::SkippedShutdown => CronJobStatus::SkippedShutdown,
        }
    }
}

#[doc(hidden)]
pub struct ScheduledJobRunner {
    job: Arc<dyn CronJob>,
    running: AtomicBool,
    tracker: Arc<ActiveExecutionTracker>,
    schedule: String,
    max_run_duration: Option<Duration>,
    status: Mutex<CronJobStatus>,
}

impl ScheduledJobRunner {
    pub fn new(
        job: Arc<dyn CronJob>,
        tracker: Arc<ActiveExecutionTracker>,
        schedule: String,
        max_run_duration: Option<Duration>,
    ) -> Self {
        Self {
            job,
            running: AtomicBool::new(false),
            tracker,
            schedule,
            max_run_duration,
            status: Mutex::new(CronJobStatus::NeverRun),
        }
    }

    pub async fn run(&self) {
        let started_at = Instant::now();
        let actual_started_at = OffsetDateTime::now_utc();
        info!(job = self.job.name(), schedule = self.schedule, actual_started_at = %actual_started_at, "cron.job.started");

        let outcome = self.execute_once().await;
        let finished_at = OffsetDateTime::now_utc();
        let duration_ms = started_at.elapsed().as_millis();
        match outcome {
            CronJobExecutionOutcome::Succeeded => {
                info!(job = self.job.name(), schedule = self.schedule, actual_started_at = %actual_started_at, finished_at = %finished_at, duration_ms, outcome = "succeeded", "cron.job.completed");
            }
            CronJobExecutionOutcome::Failed(error) => {
                error!(job = self.job.name(), schedule = self.schedule, actual_started_at = %actual_started_at, finished_at = %finished_at, duration_ms, outcome = "failed", error = %error, error_source = ?error.source(), "cron.job.completed");
            }
            CronJobExecutionOutcome::Panicked => {
                error!(job = self.job.name(), schedule = self.schedule, actual_started_at = %actual_started_at, finished_at = %finished_at, duration_ms, outcome = "panicked", "cron.job.completed");
            }
            CronJobExecutionOutcome::TimedOut => {
                error!(job = self.job.name(), schedule = self.schedule, actual_started_at = %actual_started_at, finished_at = %finished_at, duration_ms, outcome = "timed_out", "cron.job.completed");
            }
            CronJobExecutionOutcome::SkippedLocalOverlap => {
                warn!(job = self.job.name(), schedule = self.schedule, actual_started_at = %actual_started_at, finished_at = %finished_at, duration_ms, outcome = "skipped_local_overlap", "cron.job.completed");
            }
            CronJobExecutionOutcome::SkippedShutdown => {
                info!(job = self.job.name(), schedule = self.schedule, actual_started_at = %actual_started_at, finished_at = %finished_at, duration_ms, outcome = "skipped_shutdown", "cron.job.completed");
            }
        }
    }

    pub async fn execute_once(&self) -> CronJobExecutionOutcome {
        let Some(_active) = self.tracker.try_track() else {
            return self
                .complete(CronJobExecutionOutcome::SkippedShutdown)
                .await;
        };
        if self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return self
                .complete(CronJobExecutionOutcome::SkippedLocalOverlap)
                .await;
        }

        let _running = RunningGuard {
            running: &self.running,
        };
        let job = Arc::clone(&self.job);
        let mut child = tokio::spawn(async move { job.execute().await });
        let outcome = match self.max_run_duration {
            Some(duration) => match tokio::time::timeout(duration, &mut child).await {
                Ok(Ok(Ok(()))) => CronJobExecutionOutcome::Succeeded,
                Ok(Ok(Err(error))) => CronJobExecutionOutcome::Failed(error),
                Ok(Err(error)) if error.is_panic() => CronJobExecutionOutcome::Panicked,
                Ok(Err(error)) => {
                    CronJobExecutionOutcome::Failed(CronJobExecutionError::from_source(error))
                }
                Err(_) => {
                    child.abort();
                    let _ = child.await;
                    CronJobExecutionOutcome::TimedOut
                }
            },
            None => match child.await {
                Ok(Ok(())) => CronJobExecutionOutcome::Succeeded,
                Ok(Err(error)) => CronJobExecutionOutcome::Failed(error),
                Err(error) if error.is_panic() => CronJobExecutionOutcome::Panicked,
                Err(error) => {
                    CronJobExecutionOutcome::Failed(CronJobExecutionError::from_source(error))
                }
            },
        };

        self.complete(outcome).await
    }

    async fn complete(&self, outcome: CronJobExecutionOutcome) -> CronJobExecutionOutcome {
        self.set_status(outcome.status()).await;
        outcome
    }

    pub async fn status(&self) -> CronJobStatus {
        *self.status.lock().await
    }
    async fn set_status(&self, status: CronJobStatus) {
        *self.status.lock().await = status;
    }
}

struct RunningGuard<'a> {
    running: &'a AtomicBool,
}
impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    #[derive(Debug, thiserror::Error)]
    #[error("test job failure")]
    struct TestJobError;

    struct BlockingJob {
        started: Arc<Notify>,
        release: Arc<Notify>,
        runs: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl CronJob for BlockingJob {
        fn name(&self) -> &'static str {
            "test"
        }
        async fn execute(&self) -> Result<(), CronJobExecutionError> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            self.release.notified().await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn should_record_failure_with_its_source() {
        struct FailingJob;
        #[async_trait]
        impl CronJob for FailingJob {
            fn name(&self) -> &'static str {
                "failing"
            }

            async fn execute(&self) -> Result<(), CronJobExecutionError> {
                Err(CronJobExecutionError::from_source(TestJobError))
            }
        }

        let error = CronJobExecutionError::from_source(TestJobError);
        assert!(error.source().is_some());
        let runner = ScheduledJobRunner::new(
            Arc::new(FailingJob),
            Arc::new(ActiveExecutionTracker::new()),
            "test schedule".to_owned(),
            None,
        );
        let outcome = runner.execute_once().await;
        assert!(matches!(outcome, CronJobExecutionOutcome::Failed(_)));
        assert_eq!(CronJobStatus::Failed, runner.status().await);
    }

    #[tokio::test]
    async fn should_record_panic() {
        struct PanickingJob;
        #[async_trait]
        impl CronJob for PanickingJob {
            fn name(&self) -> &'static str {
                "panicking"
            }

            async fn execute(&self) -> Result<(), CronJobExecutionError> {
                std::panic::panic_any("test job panic");
            }
        }

        let runner = ScheduledJobRunner::new(
            Arc::new(PanickingJob),
            Arc::new(ActiveExecutionTracker::new()),
            "test schedule".to_owned(),
            None,
        );
        let outcome = runner.execute_once().await;
        assert!(matches!(outcome, CronJobExecutionOutcome::Panicked));
        assert_eq!(CronJobStatus::Panicked, runner.status().await);
    }

    #[tokio::test]
    async fn should_record_timeout() {
        struct SlowJob;
        #[async_trait]
        impl CronJob for SlowJob {
            fn name(&self) -> &'static str {
                "slow"
            }

            async fn execute(&self) -> Result<(), CronJobExecutionError> {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok(())
            }
        }

        let runner = ScheduledJobRunner::new(
            Arc::new(SlowJob),
            Arc::new(ActiveExecutionTracker::new()),
            "test schedule".to_owned(),
            Some(Duration::from_millis(10)),
        );
        let outcome = runner.execute_once().await;
        assert!(matches!(outcome, CronJobExecutionOutcome::TimedOut));
        assert_eq!(CronJobStatus::TimedOut, runner.status().await);
    }

    #[tokio::test]
    async fn should_drain_after_active_execution_finishes() {
        let tracker = Arc::new(ActiveExecutionTracker::new());
        let active = tracker.try_track();
        assert!(active.is_some());
        let Some(active) = active else {
            return;
        };
        let drain = tokio::spawn({
            let tracker = Arc::clone(&tracker);
            async move { tracker.drain(Duration::from_secs(1)).await }
        });
        tokio::task::yield_now().await;
        drop(active);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), drain)
                .await
                .is_ok_and(|result| matches!(result, Ok(Ok(()))))
        );
    }

    #[tokio::test]
    async fn should_skip_overlapping_execution() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let runs = Arc::new(AtomicUsize::new(0));
        let runner = Arc::new(ScheduledJobRunner::new(
            Arc::new(BlockingJob {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                runs: Arc::clone(&runs),
            }),
            Arc::new(ActiveExecutionTracker::new()),
            "test schedule".to_owned(),
            None,
        ));
        let first = tokio::spawn({
            let runner = Arc::clone(&runner);
            async move { runner.run().await }
        });
        started.notified().await;
        let outcome = runner.execute_once().await;
        assert!(matches!(
            outcome,
            CronJobExecutionOutcome::SkippedLocalOverlap
        ));
        assert_eq!(CronJobStatus::SkippedLocalOverlap, runner.status().await);
        assert_eq!(1, runs.load(Ordering::SeqCst));
        release.notify_one();
        let _ = first.await;
    }

    #[tokio::test]
    async fn should_return_shutdown_skip_outcome() {
        struct SuccessJob;
        #[async_trait]
        impl CronJob for SuccessJob {
            fn name(&self) -> &'static str {
                "success"
            }

            async fn execute(&self) -> Result<(), CronJobExecutionError> {
                Ok(())
            }
        }

        let tracker = Arc::new(ActiveExecutionTracker::new());
        tracker.stop_accepting();
        let runner = ScheduledJobRunner::new(
            Arc::new(SuccessJob),
            tracker,
            "test schedule".to_owned(),
            None,
        );
        let outcome = runner.execute_once().await;
        assert!(matches!(outcome, CronJobExecutionOutcome::SkippedShutdown));
        assert_eq!(CronJobStatus::SkippedShutdown, runner.status().await);
    }

    #[tokio::test]
    async fn should_return_successful_execution_outcome() {
        struct SuccessJob;
        #[async_trait]
        impl CronJob for SuccessJob {
            fn name(&self) -> &'static str {
                "success"
            }

            async fn execute(&self) -> Result<(), CronJobExecutionError> {
                Ok(())
            }
        }

        let runner = ScheduledJobRunner::new(
            Arc::new(SuccessJob),
            Arc::new(ActiveExecutionTracker::new()),
            "test schedule".to_owned(),
            None,
        );
        let outcome = runner.execute_once().await;
        assert!(matches!(outcome, CronJobExecutionOutcome::Succeeded));
        assert_eq!(CronJobStatus::Succeeded, runner.status().await);
    }
}
