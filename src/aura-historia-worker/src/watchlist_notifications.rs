use crate::{
    WorkerScope,
    cdc::{DomainJob, DomainJobPayload},
    queue::{JobOutcome, WorkerQueueReceiver},
};
use product_listing_service::use_cases::{
    GenerateWatchlistNotificationsCommand, GenerateWatchlistNotificationsResult,
    GenerateWatchlistNotificationsUseCase,
};
use std::sync::Arc;

/// Lambda- and polling-transport result for one historical watchlist notification job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchlistNotificationJobDisposition {
    Complete(&'static str),
    Retry(&'static str),
    DependencyUnavailable(&'static str),
    Poison(&'static str),
}

impl WatchlistNotificationJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category)
            | Self::Retry(category)
            | Self::DependencyUnavailable(category)
            | Self::Poison(category) => category,
        }
    }
}

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

/// Decode and execute one compact schema-2 watchlist notification job.
///
/// This is transport-neutral so Lambda and the polling worker retain the same historical-source,
/// lifecycle-lock, idempotency, and retry semantics.
pub async fn process_watchlist_notification_job(
    body: &str,
    handler: &(dyn GenerateWatchlistNotificationsUseCase + Send + Sync),
) -> WatchlistNotificationJobDisposition {
    match crate::wire::decode(body, WorkerScope::WatchlistNotification) {
        Ok(job) => execute_job(handler, job).await,
        Err(_) => WatchlistNotificationJobDisposition::Poison("invalid_wire_job"),
    }
}

async fn execute_job(
    handler: &(dyn GenerateWatchlistNotificationsUseCase + Send + Sync),
    job: DomainJob,
) -> WatchlistNotificationJobDisposition {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return WatchlistNotificationJobDisposition::Poison("unexpected_payload");
    };
    // The service loads the exact historical event and locks current lifecycle through commit.
    // No transport cache/current-event comparison may suppress a later historical notification.
    match handler
        .execute(GenerateWatchlistNotificationsCommand {
            event_id: event.event_id,
            product_listing_id: event.product_listing_id,
        })
        .await
    {
        Ok(result) => watchlist_outcome(result),
        Err(_) => WatchlistNotificationJobDisposition::DependencyUnavailable(
            "watchlist_notification_unavailable",
        ),
    }
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

fn watchlist_outcome(
    result: GenerateWatchlistNotificationsResult,
) -> WatchlistNotificationJobDisposition {
    match result {
        GenerateWatchlistNotificationsResult::Applied {
            recipient_count,
            inserted_count,
            already_exists_count,
        } => {
            tracing::info!(
                recipient_count,
                inserted_count,
                already_exists_count,
                "historical watchlist notifications committed"
            );
            WatchlistNotificationJobDisposition::Complete(
                if inserted_count == 0 && already_exists_count > 0 {
                    "duplicate"
                } else {
                    "applied"
                },
            )
        }
        GenerateWatchlistNotificationsResult::SuppressedForMissingSource => {
            WatchlistNotificationJobDisposition::Retry("missing_source")
        }
        GenerateWatchlistNotificationsResult::IgnoredEvent => {
            WatchlistNotificationJobDisposition::Complete("ignored_event")
        }
        GenerateWatchlistNotificationsResult::SuppressedForWithdrawnProductListing => {
            WatchlistNotificationJobDisposition::Complete("withdrawn")
        }
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
