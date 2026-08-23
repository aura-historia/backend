use crate::scheduled_job::{ActiveExecutionTracker, CronDrainError, CronJob, ScheduledJobRunner};
use chrono::Utc;
use cron_tab::AsyncCron;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::{
    sync::oneshot,
    task::{AbortHandle, JoinError},
};
use tracing::{error, info};

#[doc(hidden)]
pub struct JobRegistration {
    pub name: &'static str,
    pub schedule: String,
    pub max_run_duration: Option<Duration>,
    pub job: Arc<dyn CronJob>,
}

#[doc(hidden)]
pub struct CronScheduler {
    tracker: Arc<ActiveExecutionTracker>,
    scheduler_abort: AbortHandle,
    scheduler_exit: Option<oneshot::Receiver<Result<(), JoinError>>>,
}

impl CronScheduler {
    pub async fn start(
        registrations: Vec<JobRegistration>,
    ) -> Result<Self, CronSchedulerStartError> {
        if registrations.is_empty() {
            return Err(CronSchedulerStartError::NoJobs);
        }
        let mut names = HashSet::new();
        let tracker = Arc::new(ActiveExecutionTracker::new());
        let mut cron = AsyncCron::new(Utc);
        for registration in registrations {
            if !names.insert(registration.name) {
                return Err(CronSchedulerStartError::DuplicateJob {
                    name: registration.name,
                });
            }
            let runner = Arc::new(ScheduledJobRunner::new(
                registration.job,
                Arc::clone(&tracker),
                registration.schedule.clone(),
                registration.max_run_duration,
            ));
            cron.add_fn(&registration.schedule, move || {
                let runner = Arc::clone(&runner);
                async move { runner.run().await }
            })
            .await
            .map_err(|error| CronSchedulerStartError::InvalidSchedule {
                name: registration.name,
                detail: error.to_string(),
            })?;
        }
        let scheduler_task = tokio::spawn(async move { cron.start_blocking().await });
        let scheduler_abort = scheduler_task.abort_handle();
        let (scheduler_exit_sender, scheduler_exit) = oneshot::channel();
        tokio::spawn(async move {
            let _ = scheduler_exit_sender.send(scheduler_task.await);
        });
        info!(job_count = names.len(), "cron.scheduler.started");
        Ok(Self {
            tracker,
            scheduler_abort,
            scheduler_exit: Some(scheduler_exit),
        })
    }

    pub async fn wait_for_exit(&mut self) -> CronSchedulerTaskExit {
        let Some(scheduler_exit) = self.scheduler_exit.take() else {
            return CronSchedulerTaskExit::ObserverLost;
        };
        match scheduler_exit.await {
            Ok(Ok(())) => CronSchedulerTaskExit::Exited,
            Ok(Err(error)) if error.is_panic() => CronSchedulerTaskExit::Panicked,
            Ok(Err(_)) => CronSchedulerTaskExit::Cancelled,
            Err(_) => CronSchedulerTaskExit::ObserverLost,
        }
    }

    pub async fn shutdown(mut self, grace: Duration) -> Result<(), CronSchedulerShutdownError> {
        let started_at = Instant::now();
        self.tracker.stop_accepting();
        self.scheduler_abort.abort();
        if let Some(scheduler_exit) = self.scheduler_exit.take() {
            let _ = scheduler_exit.await;
        }
        match self.tracker.drain(grace).await {
            Ok(()) => {
                info!(
                    grace_ms = grace.as_millis() as u64,
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    "cron.scheduler.drained"
                );
                Ok(())
            }
            Err(error @ CronDrainError::TimedOut { active }) => {
                error!(
                    grace_ms = grace.as_millis() as u64,
                    duration_ms = started_at.elapsed().as_millis() as u64,
                    active_executions = active,
                    "cron.scheduler.drain_failed"
                );
                Err(error.into())
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[doc(hidden)]
pub enum CronSchedulerStartError {
    #[error("at least one cron job must be registered")]
    NoJobs,
    #[error("duplicate cron job registration: {name}")]
    DuplicateJob { name: &'static str },
    #[error("invalid cron schedule for {name}: {detail}")]
    InvalidSchedule { name: &'static str, detail: String },
}

#[derive(Debug, thiserror::Error)]
#[doc(hidden)]
pub enum CronSchedulerShutdownError {
    #[error(transparent)]
    Drain(#[from] CronDrainError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[doc(hidden)]
pub enum CronSchedulerTaskExit {
    #[error("scheduler task exited")]
    Exited,
    #[error("scheduler task panicked")]
    Panicked,
    #[error("scheduler task was cancelled")]
    Cancelled,
    #[error("scheduler task observer stopped")]
    ObserverLost,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduled_job::CronJobExecutionError;
    use async_trait::async_trait;

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

    #[tokio::test]
    async fn should_reject_invalid_seven_field_schedule() {
        let result = CronScheduler::start(vec![JobRegistration {
            name: "success",
            schedule: "invalid".to_owned(),
            max_run_duration: None,
            job: Arc::new(SuccessJob),
        }])
        .await;
        assert!(matches!(
            result,
            Err(CronSchedulerStartError::InvalidSchedule { .. })
        ));
    }

    #[tokio::test]
    async fn should_report_unexpected_scheduler_exit() {
        let mut scheduler = scheduler_for_task(tokio::spawn(async {}));

        assert_eq!(
            scheduler.wait_for_exit().await,
            CronSchedulerTaskExit::Exited
        );
    }

    #[tokio::test]
    async fn should_report_scheduler_panic() {
        let mut scheduler = scheduler_for_task(tokio::spawn(async {
            std::panic::panic_any("scheduler test panic");
        }));

        assert_eq!(
            scheduler.wait_for_exit().await,
            CronSchedulerTaskExit::Panicked
        );
    }

    fn scheduler_for_task(scheduler_task: tokio::task::JoinHandle<()>) -> CronScheduler {
        let scheduler_abort = scheduler_task.abort_handle();
        let (scheduler_exit_sender, scheduler_exit) = oneshot::channel();
        tokio::spawn(async move {
            let _ = scheduler_exit_sender.send(scheduler_task.await);
        });
        CronScheduler {
            tracker: Arc::new(ActiveExecutionTracker::new()),
            scheduler_abort,
            scheduler_exit: Some(scheduler_exit),
        }
    }
}
