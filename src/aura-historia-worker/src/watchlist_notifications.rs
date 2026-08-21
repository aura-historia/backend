use std::sync::Arc;

use common::{
    error::boxed::{BoxError, box_error},
    event_id::EventId,
    product_id::ProductId,
};
use product_service::use_cases::{
    GenerateWatchlistNotificationsCommand, GenerateWatchlistNotificationsUseCase,
};
use tracing::{error, info};

use crate::retry::{InMemoryDeadLetterQueue, RetryConfig, run_with_retry};
use crate::{
    InMemoryQueueReceiver,
    cdc::{DomainJob, DomainJobPayload},
};

pub async fn consume_watchlist_notification_queue(
    mut receiver: InMemoryQueueReceiver<DomainJob>,
    handler: Arc<dyn GenerateWatchlistNotificationsUseCase>,
) {
    let dead_letters = InMemoryDeadLetterQueue::new();
    while let Some(job) = receiver.recv().await {
        let idempotency_key = job.idempotency_key.as_str().to_owned();
        let ordering_key = job.ordering_key.as_str().to_owned();
        let handler_for_retry = Arc::clone(&handler);
        let result = run_with_retry(job, RetryConfig::default(), &dead_letters, move |job| {
            let handler = Arc::clone(&handler_for_retry);
            async move { generate_watchlist_notifications(handler, job).await }
        })
        .await;
        match result {
            Ok(outcome) => match outcome {
                WatchlistNotificationWorkerOutcome::Applied {
                    recipient_count,
                    inserted_count,
                    already_exists_count,
                } => info!(
                    job_type = "watchlist_notification",
                    %idempotency_key,
                    %ordering_key,
                    recipient_count,
                    inserted_count,
                    already_exists_count,
                    outcome = "applied",
                    "watchlist notification job completed"
                ),
                WatchlistNotificationWorkerOutcome::Duplicate {
                    recipient_count,
                    already_exists_count,
                } => info!(
                    job_type = "watchlist_notification",
                    %idempotency_key,
                    %ordering_key,
                    recipient_count,
                    already_exists_count,
                    outcome = "duplicate",
                    "watchlist notification job completed"
                ),
                WatchlistNotificationWorkerOutcome::SuppressedForMissingSource => info!(
                    job_type = "watchlist_notification",
                    %idempotency_key,
                    %ordering_key,
                    outcome = "suppressed_for_missing_source",
                    "watchlist notification job completed"
                ),
                WatchlistNotificationWorkerOutcome::SuppressedForStaleProductEvent => info!(
                    job_type = "watchlist_notification",
                    %idempotency_key,
                    %ordering_key,
                    outcome = "suppressed_for_stale_product_event",
                    "watchlist notification job completed"
                ),
            },
            Err(error) => {
                error!(job_type = "watchlist_notification", %idempotency_key, %ordering_key, error = %error, outcome = "dead_lettered_in_memory", "watchlist notification job failed")
            }
        }
    }
}

async fn generate_watchlist_notifications(
    handler: Arc<dyn GenerateWatchlistNotificationsUseCase>,
    job: DomainJob,
) -> Result<WatchlistNotificationWorkerOutcome, WatchlistNotificationWorkerError> {
    let DomainJobPayload::ProductEvent(event) = job.payload else {
        return Err(WatchlistNotificationWorkerError::UnexpectedJobPayload);
    };
    let event_id = EventId::try_from(event.event_id.as_str()).map_err(|source| {
        WatchlistNotificationWorkerError::InvalidEventId {
            source: box_error(source),
        }
    })?;
    let product_id = ProductId::try_from(event.product_id.as_str()).map_err(|source| {
        WatchlistNotificationWorkerError::InvalidProductId {
            source: box_error(source),
        }
    })?;
    handler
        .execute(GenerateWatchlistNotificationsCommand {
            event_id,
            product_id,
        })
        .await
        .map(|result| match result {
            product_service::use_cases::GenerateWatchlistNotificationsResult::Applied {
                recipient_count,
                inserted_count,
                already_exists_count,
            } if inserted_count == 0 && already_exists_count > 0 => {
                WatchlistNotificationWorkerOutcome::Duplicate {
                    recipient_count,
                    already_exists_count,
                }
            }
            product_service::use_cases::GenerateWatchlistNotificationsResult::Applied {
                recipient_count,
                inserted_count,
                already_exists_count,
            } => WatchlistNotificationWorkerOutcome::Applied {
                recipient_count,
                inserted_count,
                already_exists_count,
            },
            product_service::use_cases::GenerateWatchlistNotificationsResult::SuppressedForMissingSource => {
                WatchlistNotificationWorkerOutcome::SuppressedForMissingSource
            }
            product_service::use_cases::GenerateWatchlistNotificationsResult::SuppressedForStaleProductEvent => {
                WatchlistNotificationWorkerOutcome::SuppressedForStaleProductEvent
            }
        })
        .map_err(|source| WatchlistNotificationWorkerError::Generate {
            source: box_error(source),
        })
}

#[derive(Debug)]
enum WatchlistNotificationWorkerOutcome {
    Applied {
        recipient_count: usize,
        inserted_count: usize,
        already_exists_count: usize,
    },
    Duplicate {
        recipient_count: usize,
        already_exists_count: usize,
    },
    SuppressedForMissingSource,
    SuppressedForStaleProductEvent,
}

#[derive(Debug, thiserror::Error)]
enum WatchlistNotificationWorkerError {
    #[error("watchlist notification queue received an unexpected job payload")]
    UnexpectedJobPayload,
    #[error("watchlist notification job has an invalid event id")]
    InvalidEventId {
        #[source]
        source: BoxError,
    },
    #[error("watchlist notification job has an invalid product id")]
    InvalidProductId {
        #[source]
        source: BoxError,
    },
    #[error("watchlist notification generation failed")]
    Generate {
        #[source]
        source: BoxError,
    },
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::{
        QueueConfig,
        cdc::{IdempotencyKey, OrderingKey, ProductEventJob, WorkerQueue},
        in_memory_queue,
    };
    use product_service::use_cases::GenerateWatchlistNotificationsResult;

    #[derive(Default)]
    struct Handler {
        commands: Mutex<Vec<GenerateWatchlistNotificationsCommand>>,
    }

    #[async_trait::async_trait]
    impl GenerateWatchlistNotificationsUseCase for Handler {
        async fn execute(
            &self,
            command: GenerateWatchlistNotificationsCommand,
        ) -> Result<
            GenerateWatchlistNotificationsResult,
            product_service::use_cases::GenerateWatchlistNotificationsError,
        > {
            self.commands
                .lock()
                .map_err(|_| {
                    product_service::use_cases::GenerateWatchlistNotificationsError::NotificationCreateFailed {
                        source: box_error(std::io::Error::other("test mutex poisoned")),
                    }
                })?
                .push(command);
            Ok(GenerateWatchlistNotificationsResult::Applied {
                recipient_count: 1,
                inserted_count: 1,
                already_exists_count: 0,
            })
        }
    }

    #[tokio::test]
    async fn should_map_product_event_job_to_watchlist_notification_command()
    -> Result<(), Box<dyn std::error::Error>> {
        let (sender, receiver) = in_memory_queue(QueueConfig::new(1))?;
        let event_id = EventId::new();
        let product_id = ProductId::new();
        sender
            .enqueue(DomainJob {
                target_queue: WorkerQueue::WatchlistNotification,
                idempotency_key: IdempotencyKey::new(format!("product-event:{event_id}")),
                ordering_key: OrderingKey::new(format!("product:{product_id}")),
                payload: DomainJobPayload::ProductEvent(ProductEventJob {
                    event_id: event_id.to_string(),
                    product_id: product_id.to_string(),
                    event_type: "PRODUCT_PRICE_CHANGED".to_owned(),
                    event_group: "DOMAIN".to_owned(),
                }),
            })
            .await?;
        drop(sender);
        let handler = Arc::new(Handler::default());

        consume_watchlist_notification_queue(receiver, handler.clone()).await;

        assert_eq!(
            vec![GenerateWatchlistNotificationsCommand {
                event_id,
                product_id
            }],
            handler
                .commands
                .lock()
                .map_err(|_| std::io::Error::other("test mutex poisoned"))?
                .clone()
        );
        Ok(())
    }
}
