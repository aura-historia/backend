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

#[derive(Debug, thiserror::Error)]
#[error("{filters_failed} filters remain failed")]
struct PeriodicMatchIncompleteError {
    filters_failed: usize,
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
            .map_err(CronJobExecutionError::from_source)?
        {
            RunPeriodicSearchFilterMatchingOutcome::SkippedAlreadyRunning => {
                tracing::info!(job = self.name(), "cron.job.skipped_distributed_overlap");
                Ok(())
            }
            RunPeriodicSearchFilterMatchingOutcome::Applied(report)
                if report.filters_failed == 0 =>
            {
                Ok(())
            }
            RunPeriodicSearchFilterMatchingOutcome::Applied(report) => Err(
                CronJobExecutionError::from_source(PeriodicMatchIncompleteError {
                    filters_failed: report.filters_failed,
                }),
            ),
        }
    }
}
