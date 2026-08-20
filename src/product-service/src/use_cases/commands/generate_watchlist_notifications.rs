use crate::ports::{
    ProductWatchlistNotificationChange, ProductWatchlistNotificationSource,
    ProductWatchlistNotificationSourceReader, ProductWatchlistNotificationSourceReaderFactory,
    WatchlistNotificationRecipientReader, WatchlistNotificationRecipientReaderFactory,
};
use common::{
    error::boxed::{BoxError, box_error},
    event_id::EventId,
    transaction::{Transaction, UnitOfWork},
};
use notification_core::notification::{NotificationPayload, NotificationWatchlistPayload};
use notification_service::use_cases::commands::create_notification::{
    CreateNotificationCommand, CreateNotificationResult, CreateNotificationUseCase,
};
use product_core::product_id::ProductId;
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
    N: CreateNotificationUseCase,
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
        tx.commit().await.map_err(|source| {
            GenerateWatchlistNotificationsError::CommitTransactionFailed {
                source: box_error(source),
            }
        })?;

        let recipient_count = recipients.len();
        if recipients.is_empty() {
            return Ok(GenerateWatchlistNotificationsResult {
                recipient_count,
                inserted_count: 0,
                already_exists_count: 0,
            });
        }
        let notification_payload = notification_payload(source);
        let mut inserted_count = 0;
        let mut already_exists_count = 0;
        for recipient in recipients {
            match self
                .notifications
                .execute(CreateNotificationCommand {
                    user_id: recipient.user_id,
                    origin_event_id: command.event_id,
                    notification_payload: notification_payload.clone(),
                    external: recipient.external,
                })
                .await
                .map_err(
                    |source| GenerateWatchlistNotificationsError::NotificationCreateFailed {
                        source: box_error(source),
                    },
                )? {
                CreateNotificationResult::Created { .. } => inserted_count += 1,
                CreateNotificationResult::AlreadyExists => already_exists_count += 1,
            }
        }

        Ok(GenerateWatchlistNotificationsResult {
            recipient_count,
            inserted_count,
            already_exists_count,
        })
    }
}

fn notification_payload(source: ProductWatchlistNotificationSource) -> NotificationPayload {
    let watchlist_payload = match source.change {
        ProductWatchlistNotificationChange::PriceChanged {
            old_price,
            new_price,
        } => NotificationWatchlistPayload::PriceChange {
            old_price: old_price.map(price_map).unwrap_or_default(),
            new_price: new_price.map(price_map).unwrap_or_default(),
        },
        ProductWatchlistNotificationChange::StateChanged {
            old_state,
            new_state,
        } => NotificationWatchlistPayload::StateChange {
            old_state,
            new_state,
        },
    };
    NotificationPayload::Watchlist {
        product_id: source.product_id,
        shop_id: source.shop_id,
        shops_product_id: source.shops_product_id,
        shop_slug_id: source.shop_slug_id,
        product_slug_id: source.product_slug_id,
        shop_name: source.shop_name,
        title: source.title,
        image: source.image,
        url: source.url,
        view_url: source.view_url,
        watchlist_payload,
    }
}

fn price_map(price: money::Price) -> HashMap<money::Currency, money::MonetaryAmount> {
    HashMap::from([(price.currency, price.monetary_amount)])
}
