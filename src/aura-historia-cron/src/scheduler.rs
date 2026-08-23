use crate::scheduled_job::{ActiveExecutionTracker, CronDrainError, CronJob, ScheduledJobRunner};
use chrono::Utc;
use cron_tab::AsyncCron;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
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
    scheduler_task: JoinHandle<()>,
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
        info!(job_count = names.len(), "cron.scheduler.started");
        Ok(Self {
            tracker,
            scheduler_task,
        })
    }

    pub fn is_alive(&self) -> bool {
        !self.scheduler_task.is_finished()
    }

    pub async fn shutdown(self, grace: Duration) -> Result<(), CronSchedulerShutdownError> {
        let started_at = Instant::now();
        self.tracker.stop_accepting();
        self.scheduler_task.abort();
        let _ = self.scheduler_task.await;
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
}
