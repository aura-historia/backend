use crate::ports::{
    ProductWatchlistNotificationChange, ProductWatchlistNotificationSource,
    ProductWatchlistNotificationSourceReader, ProductWatchlistNotificationSourceReaderFactory,
    WatchlistNotificationRecipientReader, WatchlistNotificationRecipientReaderFactory,
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
    NewNotification, NotificationCreationError, NotificationCreationOutcome, NotificationCreator,
    NotificationCreatorFactory,
};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerateWatchlistNotificationsCommand {
    pub event_id: EventId,
    pub product_id: ProductId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerateWatchlistNotificationsResult {
    pub recipient_count: usize,
    pub inserted_count: usize,
    pub already_exists_count: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum GenerateWatchlistNotificationsError {
    #[error("watchlist notification source was not found")]
    SourceNotFound,
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
    S: ProductWatchlistNotificationSourceReaderFactory<U::Tx>,
    R: WatchlistNotificationRecipientReaderFactory<U::Tx>,
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
        let source = self
            .sources
            .in_transaction(&mut tx)
            .find_source(command.event_id, command.product_id)
            .await
            .map_err(
                |source| GenerateWatchlistNotificationsError::SourceReadFailed {
                    source: box_error(source),
                },
            )?
            .ok_or(GenerateWatchlistNotificationsError::SourceNotFound)?;
        let recipients = self
            .recipients
            .in_transaction(&mut tx)
            .find_active_for_product(command.product_id)
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
                external_delivery_requested: recipient.external_delivery_requested,
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

        Ok(GenerateWatchlistNotificationsResult {
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
            old_price: old_price.map(price_map).unwrap_or_default(),
            new_price: new_price.map(price_map).unwrap_or_default(),
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

fn price_map(
    price: common::price::domain::Price,
) -> HashMap<common::currency::domain::Currency, common::price::domain::MonetaryAmount> {
    HashMap::from([(price.currency, price.monetary_amount)])
}
