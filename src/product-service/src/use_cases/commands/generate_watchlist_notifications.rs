use crate::ports::{
    ProductCurrentRevisionCheck, ProductCurrentRevisionCheckError, ProductCurrentRevisionGuard,
    ProductCurrentRevisionGuardFactory, ProductWatchlistNotificationChange,
    ProductWatchlistNotificationSource, ProductWatchlistNotificationSourceReader,
    ProductWatchlistNotificationSourceReaderFactory, WatchlistNotificationRecipientReader,
    WatchlistNotificationRecipientReaderFactory,
};
use common::{
    error::boxed::{BoxError, box_error},
    event_id::EventId,
    product_id::ProductId,
    transaction::{Transaction, UnitOfWork},
};
use notification_core::notification::{
    NotificationContent, NotificationWatchlistChange, ProductNotificationSnapshot,
};
use notification_service::ports::notification_creator::{
    ExternalDeliveryRequest, NewNotification, NotificationCreationError,
    NotificationCreationOutcome, NotificationCreator, NotificationCreatorFactory,
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerateWatchlistNotificationsCommand {
    pub event_id: EventId,
    pub product_id: ProductId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerateWatchlistNotificationsResult {
    Applied {
        recipient_count: usize,
        inserted_count: usize,
        already_exists_count: usize,
    },
    SuppressedForMissingSource,
    SuppressedForStaleProductEvent,
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
    #[error("failed to lock and check the current Product revision")]
    ProductCurrentRevisionCheckFailed {
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
    S: ProductWatchlistNotificationSourceReaderFactory<U::Tx>,
    R: WatchlistNotificationRecipientReaderFactory<U::Tx>,
    G: ProductCurrentRevisionGuardFactory<U::Tx>,
    N: NotificationCreatorFactory<U::Tx>,
{
    #[tracing::instrument(name = "generate_watchlist_notifications", skip_all, fields(event_id = %command.event_id, product_id = %command.product_id))]
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
            .find_source(command.event_id, command.product_id)
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
            .lock_and_check(command.product_id, command.event_id)
            .await
            .map_err(|source: ProductCurrentRevisionCheckError| {
                GenerateWatchlistNotificationsError::ProductCurrentRevisionCheckFailed {
                    source: box_error(source),
                }
            })?;
        if revision == ProductCurrentRevisionCheck::Stale {
            tx.commit().await.map_err(|source| {
                GenerateWatchlistNotificationsError::CommitTransactionFailed {
                    source: box_error(source),
                }
            })?;
            return Ok(GenerateWatchlistNotificationsResult::SuppressedForStaleProductEvent);
        }

        let recipients = self
            .recipients
            .in_transaction(&mut tx)
            .find_eligible_for_product_at(command.product_id, source.event_time)
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
                    common::notification_id::NotificationId::new(),
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
    source: ProductWatchlistNotificationSource,
) -> NotificationContent {
    let change = match source.change {
        ProductWatchlistNotificationChange::PriceChanged {
            old_price,
            new_price,
        } => NotificationWatchlistChange::PriceChange {
            old_price,
            new_price,
        },
        ProductWatchlistNotificationChange::StateChanged {
            old_state,
            new_state,
        } => NotificationWatchlistChange::StateChange {
            old_state,
            new_state,
        },
    };
    NotificationContent::Watchlist {
        origin_event_id,
        product_id: source.product_id,
        snapshot: ProductNotificationSnapshot {
            shop_id: source.shop_id,
            shops_product_id: source.shops_product_id,
            shop_slug_id: source.shop_slug_id,
            product_slug_id: source.product_slug_id,
            shop_name: source.shop_name,
            title: source.title,
            image: source.image,
            url: source.url,
            view_url: source.view_url,
        },
        change,
    }
}
