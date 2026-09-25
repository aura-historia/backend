#[cfg(test)]
use crate::cdc::{DomainJob, DomainJobPayload};
use crate::{
    WorkerScope,
    queue::{JobOutcome, WorkerQueueReceiver},
};
use product_listing_service::use_cases::GenerateWatchlistNotificationsUseCase;
#[cfg(test)]
use product_listing_service::use_cases::{
    GenerateWatchlistNotificationsCommand, GenerateWatchlistNotificationsResult,
};
use std::sync::Arc;
use watchlist_notification_lambda::execute_job;
#[cfg(test)]
use watchlist_notification_lambda::watchlist_outcome;
pub use watchlist_notification_lambda::{
    WatchlistNotificationJobDisposition, process_watchlist_notification_job,
};

pub async fn consume_watchlist_notification_queue(
    receiver: impl Into<WorkerQueueReceiver>,
    handler: Arc<dyn GenerateWatchlistNotificationsUseCase>,
) {
    receiver
        .into()
        .run(WorkerScope::WatchlistNotification, move |job| {
            let handler = Arc::clone(&handler);
            async move { polling_outcome(execute_job(handler.as_ref(), job).await) }
        })
        .await;
}

fn polling_outcome(disposition: WatchlistNotificationJobDisposition) -> JobOutcome {
    match disposition {
        WatchlistNotificationJobDisposition::Complete(category) => JobOutcome::Complete(category),
        WatchlistNotificationJobDisposition::Retry(category) => JobOutcome::Retry(category),
        WatchlistNotificationJobDisposition::DependencyUnavailable(category) => {
            JobOutcome::DependencyUnavailable(category)
        }
        WatchlistNotificationJobDisposition::Poison(category) => JobOutcome::Invalid(category),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        QueueConfig,
        cdc::{IdempotencyKey, OrderingKey, ProductListingEventJob, WorkerQueue},
        in_memory_queue,
    };
    use domain_primitives::event_id::EventId;
    use product_listing_core::product_listing_id::ProductListingId;
    use std::sync::Mutex;
    struct Handler(Mutex<Vec<GenerateWatchlistNotificationsCommand>>);
    #[async_trait::async_trait]
    impl GenerateWatchlistNotificationsUseCase for Handler {
        async fn execute(
            &self,
            command: GenerateWatchlistNotificationsCommand,
        ) -> Result<
            GenerateWatchlistNotificationsResult,
            product_listing_service::use_cases::GenerateWatchlistNotificationsError,
        > {
            self.0.lock().unwrap().push(command);
            Ok(GenerateWatchlistNotificationsResult::IgnoredEvent)
        }
    }
    #[tokio::test]
    async fn should_map_product_event_job_to_watchlist_notification_command()
    -> Result<(), Box<dyn std::error::Error>> {
        let (sender, receiver) = in_memory_queue(QueueConfig::new(2))?;
        let event_id = EventId::new();
        let product_listing_id = ProductListingId::new();
        let job = DomainJob {
            target_queue: WorkerQueue::WatchlistNotification,
            idempotency_key: IdempotencyKey::new(format!("product-event:{event_id}")),
            ordering_key: OrderingKey::new(format!("product:{product_listing_id}")),
            payload: DomainJobPayload::ProductListingEvent(ProductListingEventJob {
                event_id,
                product_listing_id,
            }),
        };
        sender.enqueue(job.clone()).await?;
        sender.enqueue(job).await?;
        drop(sender);
        let handler = Arc::new(Handler(Mutex::new(vec![])));
        consume_watchlist_notification_queue(receiver, handler.clone()).await;
        assert_eq!(
            vec![
                GenerateWatchlistNotificationsCommand {
                    event_id,
                    product_listing_id
                };
                2
            ],
            *handler.0.lock().unwrap()
        );
        Ok(())
    }
    #[test]
    fn should_retain_missing_source_and_complete_verified_historical_outcomes() {
        assert_eq!(
            WatchlistNotificationJobDisposition::Retry("missing_source"),
            watchlist_outcome(GenerateWatchlistNotificationsResult::SuppressedForMissingSource)
        );
        for result in [
            GenerateWatchlistNotificationsResult::IgnoredEvent,
            GenerateWatchlistNotificationsResult::SuppressedForWithdrawnProductListing,
            GenerateWatchlistNotificationsResult::Applied {
                recipient_count: 1,
                inserted_count: 1,
                already_exists_count: 0,
            },
            GenerateWatchlistNotificationsResult::Applied {
                recipient_count: 1,
                inserted_count: 0,
                already_exists_count: 1,
            },
        ] {
            assert!(matches!(
                watchlist_outcome(result),
                WatchlistNotificationJobDisposition::Complete(_)
            ));
        }
    }
}
