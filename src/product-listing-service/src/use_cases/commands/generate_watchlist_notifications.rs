use crate::ports::{
    ProductListingWatchlistNotificationChange, ProductListingWatchlistNotificationSource,
    ProductListingWatchlistNotificationSourceReadOutcome,
    ProductListingWatchlistNotificationSourceReader,
    ProductListingWatchlistNotificationSourceReaderFactory, WatchlistNotificationRecipientReader,
    WatchlistNotificationRecipientReaderFactory,
};
use application::{
    error::{BoxError, box_error},
    transaction::{Transaction, UnitOfWork},
};
use domain_primitives::event_id::EventId;
use notification_core::{
    notification::{
        NotificationContent, NotificationWatchlistChange, ProductListingNotificationSnapshot,
    },
    notification_id::NotificationId,
};
use notification_service::ports::notification_creator::{
    ExternalDeliveryRequest, NewNotification, NotificationCreationError,
    NotificationCreationOutcome, NotificationCreator, NotificationCreatorFactory,
};
use product_listing_core::{
    listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerateWatchlistNotificationsCommand {
    pub event_id: EventId,
    pub product_listing_id: ProductListingId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerateWatchlistNotificationsResult {
    Applied {
        recipient_count: usize,
        inserted_count: usize,
        already_exists_count: usize,
    },
    SuppressedForMissingSource,
    IgnoredEvent,
    SuppressedForWithdrawnProductListing,
}

#[derive(Debug, thiserror::Error)]
pub enum GenerateWatchlistNotificationsError {
    #[error("failed to begin watchlist notification read transaction")]
    BeginTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to read watchlist notification source")]
    SourceReadFailed {
        #[source]
        source: BoxError,
    },

    #[error("failed to read watchlist notification recipients")]
    RecipientReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to commit watchlist notification read transaction")]
    CommitTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to create watchlist notifications")]
    NotificationCreateFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait GenerateWatchlistNotificationsUseCase: Send + Sync {
    async fn execute(
        &self,
        command: GenerateWatchlistNotificationsCommand,
    ) -> Result<GenerateWatchlistNotificationsResult, GenerateWatchlistNotificationsError>;
}

pub struct GenerateWatchlistNotificationsHandler<U, S, R, N> {
    unit_of_work: U,
    sources: S,
    recipients: R,
    notifications: N,
}

impl<U, S, R, N> GenerateWatchlistNotificationsHandler<U, S, R, N> {
    pub fn new(unit_of_work: U, sources: S, recipients: R, notifications: N) -> Self {
        Self {
            unit_of_work,
            sources,
            recipients,
            notifications,
        }
    }
}

#[async_trait::async_trait]
impl<U, S, R, N> GenerateWatchlistNotificationsUseCase
    for GenerateWatchlistNotificationsHandler<U, S, R, N>
where
    U: UnitOfWork,
    S: ProductListingWatchlistNotificationSourceReaderFactory<U::Tx>,
    R: WatchlistNotificationRecipientReaderFactory<U::Tx>,
    N: NotificationCreatorFactory<U::Tx>,
{
    #[tracing::instrument(name = "generate_watchlist_notifications", skip_all, fields(event_id = %command.event_id, product_listing_id = %command.product_listing_id))]
    async fn execute(
        &self,
        command: GenerateWatchlistNotificationsCommand,
    ) -> Result<GenerateWatchlistNotificationsResult, GenerateWatchlistNotificationsError> {
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            GenerateWatchlistNotificationsError::BeginTransactionFailed {
                source: box_error(source),
            }
        })?;
        let source_outcome = self
            .sources
            .in_transaction(&mut tx)
            .find_source(command.event_id, command.product_listing_id)
            .await
            .map_err(
                |source| GenerateWatchlistNotificationsError::SourceReadFailed {
                    source: box_error(source),
                },
            )?;
        let source = match source_outcome {
            ProductListingWatchlistNotificationSourceReadOutcome::Found(source) => source,
            ProductListingWatchlistNotificationSourceReadOutcome::MissingSource => {
                tx.commit().await.map_err(|source| {
                    GenerateWatchlistNotificationsError::CommitTransactionFailed {
                        source: box_error(source),
                    }
                })?;
                return Ok(GenerateWatchlistNotificationsResult::SuppressedForMissingSource);
            }
            ProductListingWatchlistNotificationSourceReadOutcome::IgnoredEvent => {
                tx.commit().await.map_err(|source| {
                    GenerateWatchlistNotificationsError::CommitTransactionFailed {
                        source: box_error(source),
                    }
                })?;
                return Ok(GenerateWatchlistNotificationsResult::IgnoredEvent);
            }
        };
        if source.lifecycle != ListingLifecycle::Active {
            tx.commit().await.map_err(|source| {
                GenerateWatchlistNotificationsError::CommitTransactionFailed {
                    source: box_error(source),
                }
            })?;
            return Ok(GenerateWatchlistNotificationsResult::SuppressedForWithdrawnProductListing);
        }

        let recipients = self
            .recipients
            .in_transaction(&mut tx)
            .find_eligible_for_product_at(command.product_listing_id, source.event_time)
            .await
            .map_err(
                |source| GenerateWatchlistNotificationsError::RecipientReadFailed {
                    source: box_error(source),
                },
            )?;
        let recipient_count = recipients.len();
        let notifications = recipients
            .into_iter()
            .flat_map(|recipient| {
                source.changes.iter().cloned().map({
                    let source = source.clone();
                    move |change| NewNotification {
                        notification: notification_core::notification::Notification::new(
                            NotificationId::new(),
                            recipient.user_id,
                            notification_content(command.event_id, source.clone(), change),
                        ),
                        external_delivery: if recipient.external_delivery_requested {
                            ExternalDeliveryRequest::Requested
                        } else {
                            ExternalDeliveryRequest::None
                        },
                    }
                })
            })
            .collect::<Vec<_>>();
        let outcomes = self
            .notifications
            .in_transaction(&mut tx)
            .create_many(&notifications)
            .await
            .map_err(|source: NotificationCreationError| {
                GenerateWatchlistNotificationsError::NotificationCreateFailed {
                    source: box_error(source),
                }
            })?;
        let inserted_count = outcomes
            .iter()
            .filter(|outcome| matches!(outcome, NotificationCreationOutcome::Inserted { .. }))
            .count();
        let already_exists_count = outcomes.len() - inserted_count;
        tx.commit().await.map_err(|source| {
            GenerateWatchlistNotificationsError::CommitTransactionFailed {
                source: box_error(source),
            }
        })?;

        Ok(GenerateWatchlistNotificationsResult::Applied {
            recipient_count,
            inserted_count,
            already_exists_count,
        })
    }
}

fn notification_content(
    origin_event_id: EventId,
    source: ProductListingWatchlistNotificationSource,
    source_change: ProductListingWatchlistNotificationChange,
) -> NotificationContent {
    let change = match source_change {
        ProductListingWatchlistNotificationChange::PriceChanged {
            old_price,
            new_price,
        } => NotificationWatchlistChange::PriceChange {
            old_price,
            new_price,
        },
        ProductListingWatchlistNotificationChange::AvailabilityChanged {
            old_availability,
            new_availability,
        } => NotificationWatchlistChange::AvailabilityChange {
            old_availability,
            new_availability,
        },
    };
    NotificationContent::Watchlist {
        origin_event_id,
        product_listing_id: source.product_listing_id,
        snapshot: ProductListingNotificationSnapshot {
            listing_source_id: source.source.listing_source_id,
            source_listing_id: source.source_listing_id,
            listing_source_slug_id: source.source.slug_id,
            product_listing_title_slug_id: source.product_listing_title_slug_id,
            listing_source_name: source.source.name,
            title: source.title,
            image: source.image.map(|image| image.url().clone()),
            content_policy: source.content_policy,
            url: source.url,
            view_url: source.view_url,
        },
        change,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::ListingSourceSummary;
    use crate::ports::{
        ProductListingWatchlistNotificationSourceReadError, WatchlistNotificationRecipient,
        WatchlistNotificationRecipientReadError,
    };
    use application::transaction::{TransactionError, UnitOfWork};
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use product_listing_core::{
        product_listing_slug_id::ProductListingSlugId, source_listing_id::SourceListingId,
    };
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::OffsetDateTime;
    use url::Url;
    use user_core::user_id::UserId;

    #[derive(Default)]
    struct State {
        sources: Vec<ProductListingWatchlistNotificationSource>,
        recipient_count: usize,
        recipients: Vec<WatchlistNotificationRecipient>,
        notification_batches: Vec<Vec<NewNotification>>,
        begins: usize,
        commits: usize,
    }

    type SharedState = Arc<Mutex<State>>;

    struct UnitOfWorkFake(SharedState);
    struct TransactionFake(SharedState);
    struct Sources(SharedState);
    struct SourceReader(SharedState);
    struct Recipients(SharedState);
    struct RecipientReader(SharedState);
    struct Notifications(SharedState);
    struct NotificationCreatorFake(SharedState);

    fn lock(state: &SharedState) -> MutexGuard<'_, State> {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn state() -> SharedState {
        Arc::new(Mutex::new(State::default()))
    }

    fn source(
        event_id: EventId,
        product_listing_id: ProductListingId,
        event_time: OffsetDateTime,
    ) -> ProductListingWatchlistNotificationSource {
        ProductListingWatchlistNotificationSource {
            event_id,
            event_time,
            product_listing_id,
            lifecycle: ListingLifecycle::Active,
            product_listing_title_slug_id: ProductListingSlugId::raw("product-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            source: ListingSourceSummary {
                listing_source_id: ListingSourceId::new(),
                name: ListingSourceName::try_from("Source")
                    .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
                slug_id: ListingSourceSlugId::raw("source")
                    .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
            },
            source_listing_id: SourceListingId::try_from("product-1")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            title: None,
            image: None,
            content_policy: None,
            url: Url::parse("https://example.test/product")
                .unwrap_or_else(|error| panic!("test URL invalid: {error}")),
            view_url: Url::parse("https://example.test/product/view")
                .unwrap_or_else(|error| panic!("test URL invalid: {error}")),
            changes: vec![ProductListingWatchlistNotificationChange::PriceChanged {
                old_price: None,
                new_price: None,
            }],
        }
    }

    fn handler(
        state: &SharedState,
    ) -> GenerateWatchlistNotificationsHandler<UnitOfWorkFake, Sources, Recipients, Notifications>
    {
        GenerateWatchlistNotificationsHandler::new(
            UnitOfWorkFake(Arc::clone(state)),
            Sources(Arc::clone(state)),
            Recipients(Arc::clone(state)),
            Notifications(Arc::clone(state)),
        )
    }

    fn command(
        event_id: EventId,
        product_listing_id: ProductListingId,
    ) -> GenerateWatchlistNotificationsCommand {
        GenerateWatchlistNotificationsCommand {
            event_id,
            product_listing_id,
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for UnitOfWorkFake {
        type Tx = TransactionFake;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let mut state = lock(&self.0);
            state.begins += 1;
            Ok(TransactionFake(Arc::clone(&self.0)))
        }
    }

    #[async_trait::async_trait]
    impl Transaction for TransactionFake {
        async fn commit(self) -> Result<(), TransactionError> {
            lock(&self.0).commits += 1;
            Ok(())
        }
    }

    impl ProductListingWatchlistNotificationSourceReaderFactory<TransactionFake> for Sources {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TransactionFake,
        ) -> impl ProductListingWatchlistNotificationSourceReader + 'tx {
            SourceReader(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl ProductListingWatchlistNotificationSourceReader for SourceReader {
        async fn find_source(
            &mut self,
            event_id: EventId,
            product_listing_id: ProductListingId,
        ) -> Result<
            ProductListingWatchlistNotificationSourceReadOutcome,
            ProductListingWatchlistNotificationSourceReadError,
        > {
            Ok(lock(&self.0)
                .sources
                .iter()
                .find(|source| {
                    source.event_id == event_id && source.product_listing_id == product_listing_id
                })
                .cloned()
                .map_or(
                    ProductListingWatchlistNotificationSourceReadOutcome::MissingSource,
                    ProductListingWatchlistNotificationSourceReadOutcome::Found,
                ))
        }
    }

    impl WatchlistNotificationRecipientReaderFactory<TransactionFake> for Recipients {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TransactionFake,
        ) -> impl WatchlistNotificationRecipientReader + 'tx {
            RecipientReader(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl WatchlistNotificationRecipientReader for RecipientReader {
        async fn find_eligible_for_product_at(
            &mut self,
            _product_listing_id: ProductListingId,
            _event_time: OffsetDateTime,
        ) -> Result<Vec<WatchlistNotificationRecipient>, WatchlistNotificationRecipientReadError>
        {
            let mut state = lock(&self.0);
            state.recipient_count += 1;
            Ok(state.recipients.clone())
        }
    }

    impl NotificationCreatorFactory<TransactionFake> for Notifications {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TransactionFake,
        ) -> impl NotificationCreator + 'tx {
            NotificationCreatorFake(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl NotificationCreator for NotificationCreatorFake {
        async fn create_many(
            &mut self,
            notifications: &[NewNotification],
        ) -> Result<Vec<NotificationCreationOutcome>, NotificationCreationError> {
            let mut state = lock(&self.0);
            state.notification_batches.push(notifications.to_vec());
            Ok(notifications
                .iter()
                .map(|notification| NotificationCreationOutcome::Inserted {
                    notification_id: notification.notification.notification_id(),
                })
                .collect())
        }
    }

    #[tokio::test]
    async fn should_suppress_and_commit_without_notifications_when_exact_source_is_missing() {
        let state = state();
        let product_listing_id = ProductListingId::new();
        let requested_event_id = EventId::new();
        lock(&state).sources.push(source(
            EventId::new(),
            product_listing_id,
            OffsetDateTime::UNIX_EPOCH,
        ));

        let result = handler(&state)
            .execute(command(requested_event_id, product_listing_id))
            .await;

        assert!(matches!(
            result,
            Ok(GenerateWatchlistNotificationsResult::SuppressedForMissingSource)
        ));
        let state = lock(&state);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.commits);
        assert_eq!(0, state.recipient_count);
        assert!(state.notification_batches.is_empty());
    }

    #[tokio::test]
    async fn should_suppress_withdrawn_listing_without_recipient_lookup_or_notifications() {
        let state = state();
        let event_id = EventId::new();
        let product_listing_id = ProductListingId::new();
        let mut notification_source =
            source(event_id, product_listing_id, OffsetDateTime::UNIX_EPOCH);
        notification_source.lifecycle = ListingLifecycle::Withdrawn;
        lock(&state).sources.push(notification_source);

        let result = handler(&state)
            .execute(command(event_id, product_listing_id))
            .await;

        assert!(matches!(
            result,
            Ok(GenerateWatchlistNotificationsResult::SuppressedForWithdrawnProductListing)
        ));
        let state = lock(&state);
        assert_eq!(1, state.commits);
        assert_eq!(0, state.recipient_count);
        assert!(state.notification_batches.is_empty());
    }

    #[tokio::test]
    async fn should_create_one_notification_per_relevant_change_for_each_recipient() {
        let state = state();
        let event_id = EventId::new();
        let product_listing_id = ProductListingId::new();
        {
            let mut state = lock(&state);
            let mut notification_source =
                source(event_id, product_listing_id, OffsetDateTime::UNIX_EPOCH);
            notification_source.changes.push(
                ProductListingWatchlistNotificationChange::AvailabilityChanged {
                    old_availability: None,
                    new_availability: Some(
                        product_listing_core::listing_availability::ListingAvailability::InStock,
                    ),
                },
            );
            state.sources.push(notification_source);
            state.recipients.push(WatchlistNotificationRecipient {
                user_id: UserId::new(),
                external_delivery_requested: true,
            });
        }

        let result = handler(&state)
            .execute(command(event_id, product_listing_id))
            .await;

        assert!(matches!(
            result,
            Ok(GenerateWatchlistNotificationsResult::Applied {
                recipient_count: 1,
                inserted_count: 2,
                already_exists_count: 0,
            })
        ));
        let state = lock(&state);
        assert_eq!(1, state.notification_batches.len());
        assert_eq!(2, state.notification_batches[0].len());
        assert!(state.notification_batches[0].iter().all(|notification| {
            notification.external_delivery == ExternalDeliveryRequest::Requested
        }));
        assert!(state.notification_batches[0].iter().any(|notification| {
            matches!(
                notification.notification.content(),
                NotificationContent::Watchlist {
                    change: NotificationWatchlistChange::PriceChange { .. },
                    ..
                }
            )
        }));
        assert!(state.notification_batches[0].iter().any(|notification| {
            matches!(
                notification.notification.content(),
                NotificationContent::Watchlist {
                    change: NotificationWatchlistChange::AvailabilityChange { .. },
                    ..
                }
            )
        }));
    }

    #[tokio::test]
    async fn should_create_notifications_for_each_active_historical_event() {
        let state = state();
        let product_listing_id = ProductListingId::new();
        let older_event_id = EventId::new();
        let current_event_id = EventId::new();
        let older_event_time = OffsetDateTime::UNIX_EPOCH;
        let current_event_time = older_event_time + time::Duration::seconds(1);
        {
            let mut state = lock(&state);
            state
                .sources
                .push(source(older_event_id, product_listing_id, older_event_time));
            state.sources.push(source(
                current_event_id,
                product_listing_id,
                current_event_time,
            ));

            state.recipients.push(WatchlistNotificationRecipient {
                user_id: UserId::new(),
                external_delivery_requested: true,
            });
        }

        let older_result = handler(&state)
            .execute(command(older_event_id, product_listing_id))
            .await;
        let current_result = handler(&state)
            .execute(command(current_event_id, product_listing_id))
            .await;

        for result in [older_result, current_result] {
            assert!(matches!(
                result,
                Ok(GenerateWatchlistNotificationsResult::Applied {
                    recipient_count: 1,
                    inserted_count: 1,
                    already_exists_count: 0,
                })
            ));
        }
        let state = lock(&state);
        assert_eq!(2, state.commits);
        assert_eq!(2, state.recipient_count);
        assert_eq!(2, state.notification_batches.len());
        assert_eq!(
            vec![Some(older_event_id), Some(current_event_id)],
            state
                .notification_batches
                .iter()
                .map(|batch| batch[0].notification.origin_event_id())
                .collect::<Vec<_>>(),
        );
    }
}
