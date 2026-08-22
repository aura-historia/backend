use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify};
use tracing::{error, info, warn};

#[async_trait]
#[doc(hidden)]
pub trait CronJob: Send + Sync {
    fn name(&self) -> &'static str;
    async fn execute(&self) -> Result<(), CronJobExecutionError>;
}

#[derive(Debug, thiserror::Error)]
#[error("{detail}")]
#[doc(hidden)]
pub struct CronJobExecutionError {
    detail: String,
}

impl CronJobExecutionError {
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
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
    max_run_duration: Option<Duration>,
    status: Mutex<CronJobStatus>,
}

impl ScheduledJobRunner {
    pub fn new(
        job: Arc<dyn CronJob>,
        tracker: Arc<ActiveExecutionTracker>,
        max_run_duration: Option<Duration>,
    ) -> Self {
        Self {
            job,
            running: AtomicBool::new(false),
            tracker,
            max_run_duration,
            status: Mutex::new(CronJobStatus::NeverRun),
        }
    }

    pub async fn run(&self) {
        let Some(_active) = self.tracker.try_track() else {
            self.set_status(CronJobStatus::SkippedShutdown).await;
            info!(job = self.job.name(), "cron.job.skipped_shutdown");
            return;
        };
        if self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            self.set_status(CronJobStatus::SkippedLocalOverlap).await;
            warn!(job = self.job.name(), "cron.job.skipped_local_overlap");
            return;
        }
        let _running = RunningGuard {
            running: &self.running,
        };
        let job = Arc::clone(&self.job);
        let mut child = tokio::spawn(async move { job.execute().await });
        let outcome = match self.max_run_duration {
            Some(duration) => match tokio::time::timeout(duration, &mut child).await {
                Ok(result) => result,
                Err(_) => {
                    child.abort();
                    let _ = child.await;
                    self.set_status(CronJobStatus::TimedOut).await;
                    error!(job = self.job.name(), "cron.job.timed_out");
                    return;
                }
            },
            None => child.await,
        };
        match outcome {
            Ok(Ok(())) => {
                self.set_status(CronJobStatus::Succeeded).await;
                info!(job = self.job.name(), "cron.job.succeeded");
            }
            Ok(Err(error)) => {
                self.set_status(CronJobStatus::Failed).await;
                error!(job = self.job.name(), %error, "cron.job.failed");
            }
            Err(error) if error.is_panic() => {
                self.set_status(CronJobStatus::Panicked).await;
                error!(job = self.job.name(), "cron.job.panicked");
            }
            Err(error) => {
                self.set_status(CronJobStatus::Failed).await;
                error!(job = self.job.name(), %error, "cron.job.cancelled");
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
