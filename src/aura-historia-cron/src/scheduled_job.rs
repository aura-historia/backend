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
                if self.active.load(Ordering::Acquire) == 0 {
                    return;
                }
                self.drained.notified().await;
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
        let Some(_active) = self.tracker.try_track() else {
            self.set_status(CronJobStatus::SkippedShutdown).await;
            info!(
                job = self.job.name(),
                schedule = self.schedule,
                outcome = "skipped_shutdown",
                "cron.job.skipped_shutdown"
            );
            return;
        };
        if self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            self.set_status(CronJobStatus::SkippedLocalOverlap).await;
            warn!(
                job = self.job.name(),
                schedule = self.schedule,
                outcome = "skipped_local_overlap",
                "cron.job.skipped_local_overlap"
            );
            return;
        }
        let _running = RunningGuard {
            running: &self.running,
        };
        let started_at = Instant::now();
        let actual_started_at = OffsetDateTime::now_utc();
        info!(job = self.job.name(), schedule = self.schedule, actual_started_at = %actual_started_at, "cron.job.started");
        let job = Arc::clone(&self.job);
        let mut child = tokio::spawn(async move { job.execute().await });
        let outcome = match self.max_run_duration {
            Some(duration) => match tokio::time::timeout(duration, &mut child).await {
                Ok(result) => result,
                Err(_) => {
                    child.abort();
                    let _ = child.await;
                    self.set_status(CronJobStatus::TimedOut).await;
                    let finished_at = OffsetDateTime::now_utc();
                    let duration_ms = started_at.elapsed().as_millis() as u64;
                    error!(job = self.job.name(), schedule = self.schedule, actual_started_at = %actual_started_at, finished_at = %finished_at, duration_ms, outcome = "timed_out", "cron.job.timed_out");
                    error!(job = self.job.name(), schedule = self.schedule, actual_started_at = %actual_started_at, finished_at = %finished_at, duration_ms, outcome = "timed_out", "cron.job.completed");
                    return;
                }
            },
            None => child.await,
        };
        match outcome {
            Ok(Ok(())) => {
                self.set_status(CronJobStatus::Succeeded).await;
                let finished_at = OffsetDateTime::now_utc();
                info!(job = self.job.name(), schedule = self.schedule, actual_started_at = %actual_started_at, finished_at = %finished_at, duration_ms = started_at.elapsed().as_millis() as u64, outcome = "succeeded", "cron.job.completed");
            }
            Ok(Err(error)) => {
                self.set_status(CronJobStatus::Failed).await;
                let finished_at = OffsetDateTime::now_utc();
                let duration_ms = started_at.elapsed().as_millis() as u64;
                error!(job = self.job.name(), schedule = self.schedule, actual_started_at = %actual_started_at, finished_at = %finished_at, duration_ms, outcome = "failed", error = %error, error_source = ?error.source(), "cron.job.failed");
                error!(job = self.job.name(), schedule = self.schedule, actual_started_at = %actual_started_at, finished_at = %finished_at, duration_ms, outcome = "failed", error = %error, error_source = ?error.source(), "cron.job.completed");
            }
            Err(error) if error.is_panic() => {
                self.set_status(CronJobStatus::Panicked).await;
                let finished_at = OffsetDateTime::now_utc();
                let duration_ms = started_at.elapsed().as_millis() as u64;
                error!(job = self.job.name(), schedule = self.schedule, actual_started_at = %actual_started_at, finished_at = %finished_at, duration_ms, outcome = "panicked", "cron.job.panicked");
                error!(job = self.job.name(), schedule = self.schedule, actual_started_at = %actual_started_at, finished_at = %finished_at, duration_ms, outcome = "panicked", "cron.job.completed");
            }
            Err(error) => {
                self.set_status(CronJobStatus::Failed).await;
                let finished_at = OffsetDateTime::now_utc();
                let duration_ms = started_at.elapsed().as_millis() as u64;
                error!(job = self.job.name(), schedule = self.schedule, actual_started_at = %actual_started_at, finished_at = %finished_at, duration_ms, outcome = "cancelled", error = %error, "cron.job.failed");
                error!(job = self.job.name(), schedule = self.schedule, actual_started_at = %actual_started_at, finished_at = %finished_at, duration_ms, outcome = "cancelled", error = %error, "cron.job.completed");
            }
        }
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
        runner.run().await;
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
        runner.run().await;
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
        runner.run().await;
        assert_eq!(CronJobStatus::TimedOut, runner.status().await);
    }

    #[tokio::test]
    async fn should_drain_after_active_execution_finishes() {
        let tracker = ActiveExecutionTracker::new();
        let active = tracker.try_track();
        assert!(active.is_some());
        let Some(active) = active else {
            return;
        };
        assert!(
            tokio::time::timeout(
                Duration::from_millis(10),
                tracker.drain(Duration::from_secs(1))
            )
            .await
            .is_err()
        );
        drop(active);
        assert!(tracker.drain(Duration::from_secs(1)).await.is_ok());
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
        runner.run().await;
        assert_eq!(CronJobStatus::SkippedLocalOverlap, runner.status().await);
        assert_eq!(1, runs.load(Ordering::SeqCst));
        release.notify_one();
        let _ = first.await;
    }
}
