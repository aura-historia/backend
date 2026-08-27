use crate::ports::{
    ProductListingCurrentRevisionCheck, ProductListingCurrentRevisionCheckError,
    ProductListingCurrentRevisionGuard, ProductListingCurrentRevisionGuardFactory,
    ProductListingWatchlistNotificationChange, ProductListingWatchlistNotificationSource,
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
use product_listing_core::product_listing_id::ProductListingId;
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
    SuppressedForStaleProductListingEvent,
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
    #[error("failed to lock and check the current ProductListing revision")]
    ProductListingCurrentRevisionCheckFailed {
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

pub struct GenerateWatchlistNotificationsHandler<U, S, R, G, N> {
    unit_of_work: U,
    sources: S,
    recipients: R,
    product_revision_guard: G,
    notifications: N,
}

impl<U, S, R, G, N> GenerateWatchlistNotificationsHandler<U, S, R, G, N> {
    pub fn new(
        unit_of_work: U,
        sources: S,
        recipients: R,
        product_revision_guard: G,
        notifications: N,
    ) -> Self {
        Self {
            unit_of_work,
            sources,
            recipients,
            product_revision_guard,
            notifications,
        }
    }
}

#[async_trait::async_trait]
impl<U, S, R, G, N> GenerateWatchlistNotificationsUseCase
    for GenerateWatchlistNotificationsHandler<U, S, R, G, N>
where
    U: UnitOfWork,
    S: ProductListingWatchlistNotificationSourceReaderFactory<U::Tx>,
    R: WatchlistNotificationRecipientReaderFactory<U::Tx>,
    G: ProductListingCurrentRevisionGuardFactory<U::Tx>,
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
        let Some(source) = self
            .sources
            .in_transaction(&mut tx)
            .find_source(command.event_id, command.product_listing_id)
            .await
            .map_err(
                |source| GenerateWatchlistNotificationsError::SourceReadFailed {
                    source: box_error(source),
                },
            )?
        else {
            tx.commit().await.map_err(|source| {
                GenerateWatchlistNotificationsError::CommitTransactionFailed {
                    source: box_error(source),
                }
            })?;
            return Ok(GenerateWatchlistNotificationsResult::SuppressedForMissingSource);
        };

        let revision = self
            .product_revision_guard
            .in_transaction(&mut tx)
            .lock_and_check(command.product_listing_id, command.event_id)
            .await
            .map_err(|source: ProductListingCurrentRevisionCheckError| {
                GenerateWatchlistNotificationsError::ProductListingCurrentRevisionCheckFailed {
                    source: box_error(source),
                }
            })?;
        if revision == ProductListingCurrentRevisionCheck::Stale {
            tx.commit().await.map_err(|source| {
                GenerateWatchlistNotificationsError::CommitTransactionFailed {
                    source: box_error(source),
                }
            })?;
            return Ok(GenerateWatchlistNotificationsResult::SuppressedForStaleProductListingEvent);
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
            .map(|recipient| NewNotification {
                notification: notification_core::notification::Notification::new(
                    NotificationId::new(),
                    recipient.user_id,
                    notification_content(command.event_id, source.clone()),
                ),
                external_delivery: if recipient.external_delivery_requested {
                    ExternalDeliveryRequest::Requested
                } else {
                    ExternalDeliveryRequest::None
                },
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
) -> NotificationContent {
    let change = match source.change {
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
            shop_id: source.shop_id,
            shop_listing_id: source.shop_listing_id,
            shop_slug_id: source.shop_slug_id,
            product_listing_slug_id: source.product_listing_slug_id,
            shop_name: source.shop_name,
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
    use crate::ports::{
        ProductListingCurrentRevisionRef, ProductListingWatchlistNotificationSourceReadError,
        WatchlistNotificationRecipient, WatchlistNotificationRecipientReadError,
    };
    use application::{
        error::static_error,
        transaction::{TransactionError, UnitOfWork},
    };
    use product_listing_core::{
        product_listing_slug_id::ProductListingSlugId, shop_listing_id::ShopListingId,
    };
    use shop_core::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
    use std::{
        collections::{HashMap, VecDeque},
        sync::{Arc, Mutex, MutexGuard},
    };
    use time::OffsetDateTime;
    use url::Url;
    use user_core::user_id::UserId;

    #[derive(Default)]
    struct State {
        sources: Vec<ProductListingWatchlistNotificationSource>,
        revision_checks: VecDeque<RevisionOutcome>,
        recipient_count: usize,
        recipients: Vec<WatchlistNotificationRecipient>,
        notification_batches: Vec<Vec<NewNotification>>,
        begins: usize,
        commits: usize,
    }

    #[derive(Clone, Copy)]
    enum RevisionOutcome {
        Current,
        Stale,
        Failure,
    }

    type SharedState = Arc<Mutex<State>>;

    struct UnitOfWorkFake(SharedState);
    struct TransactionFake(SharedState);
    struct Sources(SharedState);
    struct SourceReader(SharedState);
    struct RevisionGuards(SharedState);
    struct RevisionGuard(SharedState);
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
            product_listing_slug_id: ProductListingSlugId::from("product"),
            shop_id: ShopId::new(),
            shop_listing_id: ShopListingId::from("product-1"),
            shop_slug_id: ShopSlugId::from("shop"),
            shop_name: ShopName::from("Shop"),
            title: None,
            image: None,
            content_policy: None,
            url: Url::parse("https://example.test/product")
                .unwrap_or_else(|error| panic!("test URL invalid: {error}")),
            view_url: Url::parse("https://example.test/product/view")
                .unwrap_or_else(|error| panic!("test URL invalid: {error}")),
            change: ProductListingWatchlistNotificationChange::PriceChanged {
                old_price: None,
                new_price: None,
            },
        }
    }

    fn handler(
        state: &SharedState,
    ) -> GenerateWatchlistNotificationsHandler<
        UnitOfWorkFake,
        Sources,
        Recipients,
        RevisionGuards,
        Notifications,
    > {
        GenerateWatchlistNotificationsHandler::new(
            UnitOfWorkFake(Arc::clone(state)),
            Sources(Arc::clone(state)),
            Recipients(Arc::clone(state)),
            RevisionGuards(Arc::clone(state)),
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
            Option<ProductListingWatchlistNotificationSource>,
            ProductListingWatchlistNotificationSourceReadError,
        > {
            Ok(lock(&self.0)
                .sources
                .iter()
                .find(|source| {
                    source.event_id == event_id && source.product_listing_id == product_listing_id
                })
                .cloned())
        }
    }

    impl ProductListingCurrentRevisionGuardFactory<TransactionFake> for RevisionGuards {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TransactionFake,
        ) -> impl ProductListingCurrentRevisionGuard + 'tx {
            RevisionGuard(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl ProductListingCurrentRevisionGuard for RevisionGuard {
        async fn lock_and_check(
            &mut self,
            _product_listing_id: ProductListingId,
            _expected_event_id: EventId,
        ) -> Result<ProductListingCurrentRevisionCheck, ProductListingCurrentRevisionCheckError>
        {
            match lock(&self.0).revision_checks.pop_front() {
                Some(RevisionOutcome::Current) | None => {
                    Ok(ProductListingCurrentRevisionCheck::Current)
                }
                Some(RevisionOutcome::Stale) => Ok(ProductListingCurrentRevisionCheck::Stale),
                Some(RevisionOutcome::Failure) => {
                    Err(ProductListingCurrentRevisionCheckError::CheckFailed {
                        source: static_error("guard read failed"),
                    })
                }
            }
        }

        async fn lock_and_check_all(
            &mut self,
            refs: &[ProductListingCurrentRevisionRef],
        ) -> Result<
            HashMap<ProductListingCurrentRevisionRef, ProductListingCurrentRevisionCheck>,
            ProductListingCurrentRevisionCheckError,
        > {
            let mut checks = HashMap::new();
            for reference in refs {
                checks.insert(
                    *reference,
                    self.lock_and_check(reference.product_listing_id, reference.expected_event_id)
                        .await?,
                );
            }
            Ok(checks)
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
    async fn should_suppress_stale_product_event_without_notifications_or_delivery() {
        let state = state();
        let event_id = EventId::new();
        let product_listing_id = ProductListingId::new();
        {
            let mut state = lock(&state);
            state.sources.push(source(
                event_id,
                product_listing_id,
                OffsetDateTime::UNIX_EPOCH,
            ));
            state.revision_checks.push_back(RevisionOutcome::Stale);
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
            Ok(GenerateWatchlistNotificationsResult::SuppressedForStaleProductListingEvent)
        ));
        let state = lock(&state);
        assert_eq!(1, state.commits);
        assert_eq!(0, state.recipient_count);
        assert!(state.notification_batches.is_empty());
    }

    #[tokio::test]
    async fn should_return_typed_revision_check_failure_without_suppression() {
        let state = state();
        let event_id = EventId::new();
        let product_listing_id = ProductListingId::new();
        {
            let mut state = lock(&state);
            state.sources.push(source(
                event_id,
                product_listing_id,
                OffsetDateTime::UNIX_EPOCH,
            ));
            state.revision_checks.push_back(RevisionOutcome::Failure);
        }

        let result = handler(&state)
            .execute(command(event_id, product_listing_id))
            .await;

        assert!(matches!(
            result,
            Err(
                GenerateWatchlistNotificationsError::ProductListingCurrentRevisionCheckFailed { .. }
            )
        ));
        let state = lock(&state);
        assert_eq!(0, state.commits);
        assert_eq!(0, state.recipient_count);
        assert!(state.notification_batches.is_empty());
    }

    #[tokio::test]
    async fn should_create_notification_for_current_event_after_older_event_was_stale() {
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
            state.revision_checks.push_back(RevisionOutcome::Stale);
            state.revision_checks.push_back(RevisionOutcome::Current);
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

        assert!(matches!(
            older_result,
            Ok(GenerateWatchlistNotificationsResult::SuppressedForStaleProductListingEvent)
        ));
        assert!(matches!(
            current_result,
            Ok(GenerateWatchlistNotificationsResult::Applied {
                recipient_count: 1,
                inserted_count: 1,
                already_exists_count: 0,
            })
        ));
        let state = lock(&state);
        assert_eq!(2, state.commits);
        assert_eq!(1, state.recipient_count);
        assert_eq!(1, state.notification_batches.len());
        assert_eq!(1, state.notification_batches[0].len());
        assert_eq!(
            ExternalDeliveryRequest::Requested,
            state.notification_batches[0][0].external_delivery
        );
        assert_eq!(
            Some(current_event_id),
            state.notification_batches[0][0]
                .notification
                .origin_event_id()
        );
    }
}
