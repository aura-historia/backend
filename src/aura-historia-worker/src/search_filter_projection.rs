use std::sync::Arc;

use application::error::{BoxError, box_error};
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::use_cases::{
    ProjectSearchFilterChangeCommand, ProjectSearchFilterChangeUseCase,
    SearchFilterProjectionOperation,
};
use tracing::{error, info};

use crate::InMemoryQueueReceiver;
use crate::cdc::{CdcOperation, DomainJob, DomainJobPayload};
use crate::retry::{InMemoryDeadLetterQueue, RetryConfig, run_with_retry};

pub async fn consume_search_filter_projection_queue(
    mut receiver: InMemoryQueueReceiver<DomainJob>,
    handler: Arc<dyn ProjectSearchFilterChangeUseCase>,
) {
    let dead_letters = InMemoryDeadLetterQueue::new();

    while let Some(job) = receiver.recv().await {
        let idempotency_key = job.idempotency_key.as_str().to_owned();
        let ordering_key = job.ordering_key.as_str().to_owned();
        let handler_for_retry = Arc::clone(&handler);
        let result = run_with_retry(job, RetryConfig::default(), &dead_letters, move |job| {
            let handler = Arc::clone(&handler_for_retry);
            async move { project_search_filter_change(handler, job).await }
        })
        .await;

        match result {
            Ok(()) => info!(
                job_type = "search_filter_opensearch",
                %idempotency_key,
                %ordering_key,
                outcome = "applied",
                "search filter projection job completed"
            ),
            Err(error) => error!(
                job_type = "search_filter_opensearch",
                %idempotency_key,
                %ordering_key,
                error = %error,
                outcome = "dead_lettered_in_memory",
                "search filter projection job failed"
            ),
        }
    }
}

async fn project_search_filter_change(
    handler: Arc<dyn ProjectSearchFilterChangeUseCase>,
    job: DomainJob,
) -> Result<(), SearchFilterProjectionWorkerError> {
    let DomainJobPayload::SearchFilterChanged(change) = job.payload else {
        return Err(SearchFilterProjectionWorkerError::UnexpectedJobPayload);
    };
    let search_filter_id = UserSearchFilterId::try_from(change.user_search_filter_id.as_str())
        .map_err(
            |source| SearchFilterProjectionWorkerError::InvalidSearchFilterId {
                source: box_error(source),
            },
        )?;
    let operation = match change.operation {
        CdcOperation::Insert | CdcOperation::Update => SearchFilterProjectionOperation::Upsert,
        CdcOperation::Delete => SearchFilterProjectionOperation::Delete,
    };
    handler
        .execute(ProjectSearchFilterChangeCommand {
            search_filter_id,
            source_version: change.version,
            operation,
        })
        .await
        .map(|result| {
            info!(
                job_type = "search_filter_opensearch",
                search_filter_id = %search_filter_id,
                source_version = change.version,
                operation = %change.operation,
                outcome = ?result.outcome,
                "search filter projection write completed"
            );
        })
        .map_err(|source| SearchFilterProjectionWorkerError::Project {
            source: box_error(source),
        })
}

#[derive(Debug, thiserror::Error)]
enum SearchFilterProjectionWorkerError {
    #[error("search filter projection queue received an unexpected job payload")]
    UnexpectedJobPayload,
    #[error("search filter projection job has an invalid search filter id")]
    InvalidSearchFilterId {
        #[source]
        source: BoxError,
    },
    #[error("search filter projection failed")]
    Project {
        #[source]
        source: BoxError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::QueueConfig;
    use crate::cdc::{IdempotencyKey, OrderingKey, SearchFilterChangedJob, WorkerQueue};
    use crate::in_memory_queue;
    use search_filter_service::ports::SearchFilterProjectionWriteOutcome;
    use search_filter_service::use_cases::ProjectSearchFilterChangeError;
    use search_filter_service::use_cases::ProjectSearchFilterChangeResult;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Handler {
        commands: Mutex<Vec<ProjectSearchFilterChangeCommand>>,
    }

    #[async_trait::async_trait]
    impl ProjectSearchFilterChangeUseCase for Handler {
        async fn execute(
            &self,
            command: ProjectSearchFilterChangeCommand,
        ) -> Result<ProjectSearchFilterChangeResult, ProjectSearchFilterChangeError> {
            self.commands
                .lock()
                .map_err(|_| ProjectSearchFilterChangeError::WriteFailed {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                })?
                .push(command);
            Ok(ProjectSearchFilterChangeResult {
                outcome: SearchFilterProjectionWriteOutcome::Applied,
            })
        }
    }

    #[tokio::test]
    async fn should_map_cdc_change_to_projection_command() -> Result<(), Box<dyn std::error::Error>>
    {
        let (sender, receiver) = in_memory_queue(QueueConfig::new(1))?;
        let handler = Arc::new(Handler::default());
        let expected_id = UserSearchFilterId::new();
        sender
            .enqueue(DomainJob {
                target_queue: WorkerQueue::SearchFilterOpenSearch,
                idempotency_key: IdempotencyKey::new("search-filter:test:2:update"),
                ordering_key: OrderingKey::new("search-filter:test"),
                payload: DomainJobPayload::SearchFilterChanged(SearchFilterChangedJob {
                    user_id: "10000000-0000-0000-0000-000000000001".to_owned(),
                    user_search_filter_id: expected_id.to_string(),
                    version: 2,
                    operation: CdcOperation::Update,
                }),
            })
            .await?;
        drop(sender);

        consume_search_filter_projection_queue(receiver, handler.clone()).await;

        let commands = handler
            .commands
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?
            .clone();
        assert_eq!(
            vec![ProjectSearchFilterChangeCommand {
                search_filter_id: expected_id,
                source_version: 2,
                operation: SearchFilterProjectionOperation::Upsert,
            }],
            commands
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_map_cdc_delete_to_projection_delete_command()
    -> Result<(), Box<dyn std::error::Error>> {
        let (sender, receiver) = in_memory_queue(QueueConfig::new(1))?;
        let handler = Arc::new(Handler::default());
        let expected_id = UserSearchFilterId::new();
        sender
            .enqueue(DomainJob {
                target_queue: WorkerQueue::SearchFilterOpenSearch,
                idempotency_key: IdempotencyKey::new("search-filter:test:3:delete"),
                ordering_key: OrderingKey::new("search-filter:test"),
                payload: DomainJobPayload::SearchFilterChanged(SearchFilterChangedJob {
                    user_id: "10000000-0000-0000-0000-000000000001".to_owned(),
                    user_search_filter_id: expected_id.to_string(),
                    version: 3,
                    operation: CdcOperation::Delete,
                }),
            })
            .await?;
        drop(sender);

        consume_search_filter_projection_queue(receiver, handler.clone()).await;

        let commands = handler
            .commands
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?
            .clone();
        assert_eq!(
            vec![ProjectSearchFilterChangeCommand {
                search_filter_id: expected_id,
                source_version: 3,
                operation: SearchFilterProjectionOperation::Delete,
            }],
            commands
        );
        Ok(())
    }
}
