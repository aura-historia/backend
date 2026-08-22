use crate::scheduled_job::{CronJob, CronJobExecutionError};
use search_filter_service::use_cases::{
    RunPeriodicSearchFilterMatchingCommand, RunPeriodicSearchFilterMatchingOutcome,
    RunPeriodicSearchFilterMatchingUseCase,
};
use std::sync::Arc;
use time::OffsetDateTime;

pub(crate) struct SearchFilterPeriodicMatchJob {
    use_case: Arc<dyn RunPeriodicSearchFilterMatchingUseCase>,
}
impl SearchFilterPeriodicMatchJob {
    pub(crate) fn new(use_case: Arc<dyn RunPeriodicSearchFilterMatchingUseCase>) -> Self {
        Self { use_case }
    }
}
#[async_trait::async_trait]
impl CronJob for SearchFilterPeriodicMatchJob {
    fn name(&self) -> &'static str {
        "search-filter-periodic-match"
    }
    async fn execute(&self) -> Result<(), CronJobExecutionError> {
        match self
            .use_case
            .execute(RunPeriodicSearchFilterMatchingCommand {
                started_at: OffsetDateTime::now_utc(),
            })
            .await
            .map_err(|error| CronJobExecutionError::new(error.to_string()))?
        {
            RunPeriodicSearchFilterMatchingOutcome::SkippedAlreadyRunning => {
                tracing::info!(job = self.name(), "cron.job.skipped_distributed_overlap");
                Ok(())
            }
            RunPeriodicSearchFilterMatchingOutcome::Applied(report)
                if report.filters_failed == 0 =>
            {
                tracing::info!(job = self.name(), ?report, "cron.job.completed");
                Ok(())
            }
            RunPeriodicSearchFilterMatchingOutcome::Applied(report) => {
                tracing::error!(
                    job = self.name(),
                    ?report,
                    "cron.job.completed_with_failed_filters"
                );
                Err(CronJobExecutionError::new(format!(
                    "{} filters remain failed",
                    report.filters_failed
                )))
            }
        }
    }
}
