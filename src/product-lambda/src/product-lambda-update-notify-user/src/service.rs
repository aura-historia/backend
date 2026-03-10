use common::currency::domain::Currency;
use common::price::domain::MonetaryAmount;
use common::product_state::domain::ProductState;
use notification::core::notification::{NotificationPayload, NotificationWatchlistPayload};
use notification::service::command::CreateNotificationCommand;
use product::core::product::Product;
use product::core::product_event::ProductDomainEvent;
use product::core::product_event::domain::{
    ProductCommonEventPayload, ProductCreatedDomainEventPayload, ProductDomainEventPayload,
    ProductStateChangeDomainEventPayload,
};
use product::service::get_service::{GetProductError, GetProductService};
use product_watchlist::service::product_watchlist_service::{
    ProductWatchListService, WatchProductError,
};
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum ProductEventWatchlistNotificationsServiceError {
    #[error("WatchProductError: {0}")]
    WatchProductError(#[from] WatchProductError),

    #[error("GetProductError: {0}")]
    GetProductError(#[from] GetProductError),
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductEventWatchlistNotificationsService {
    async fn determine_notification_commands(
        &self,
        event: ProductDomainEvent,
    ) -> Result<Vec<CreateNotificationCommand>, ProductEventWatchlistNotificationsServiceError>;
}

pub struct ProductEventWatchlistNotificationsServiceImpl<'a> {
    watchlist_service: &'a (dyn ProductWatchListService + Sync),
    get_product_service: &'a (dyn GetProductService + Sync),
}

impl<'a> ProductEventWatchlistNotificationsServiceImpl<'a> {
    pub fn new(
        watchlist_service: &'a (dyn ProductWatchListService + Sync),
        get_product_service: &'a (dyn GetProductService + Sync),
    ) -> Self {
        ProductEventWatchlistNotificationsServiceImpl {
            watchlist_service,
            get_product_service,
        }
    }
}

#[async_trait::async_trait]
impl<'a> ProductEventWatchlistNotificationsService
    for ProductEventWatchlistNotificationsServiceImpl<'a>
{
    async fn determine_notification_commands(
        &self,
        event: ProductDomainEvent,
    ) -> Result<Vec<CreateNotificationCommand>, ProductEventWatchlistNotificationsServiceError>
    {
        let user_ids = self
            .watchlist_service
            .find_user_ids_with_notifications(&event.aggregate_id)
            .await?;
        if user_ids.is_empty() {
            return Ok(vec![]);
        }

        let product = self
            .get_product_service
            .find_product(event.payload.shop_id(), event.payload.shops_product_id())
            .await?;

        let notifications = user_ids
            .into_iter()
            .map(|user_id| CreateNotificationCommand {
                user_id,
                notification_payload: mk_notification_payload(&product, &event.payload),
            })
            .collect();
        Ok(notifications)
    }
}

fn mk_notification_payload(
    product: &Product,
    event_payload: &ProductDomainEventPayload,
) -> NotificationPayload {
    match event_payload {
        ProductDomainEventPayload::Created(payload) => {
            mk_created_watchlist_notification_payload(product, payload)
        }
        ProductDomainEventPayload::StateListed(payload) => {
            mk_state_change_watchlist_notification_payload(product, payload, &ProductState::Listed)
        }
        ProductDomainEventPayload::StateAvailable(payload) => {
            mk_state_change_watchlist_notification_payload(
                product,
                payload,
                &ProductState::Available,
            )
        }
        ProductDomainEventPayload::StateReserved(payload) => {
            mk_state_change_watchlist_notification_payload(
                product,
                payload,
                &ProductState::Reserved,
            )
        }
        ProductDomainEventPayload::StateSold(payload) => {
            mk_state_change_watchlist_notification_payload(product, payload, &ProductState::Sold)
        }
        ProductDomainEventPayload::StateRemoved(payload) => {
            mk_state_change_watchlist_notification_payload(product, payload, &ProductState::Removed)
        }
        ProductDomainEventPayload::StateUnknown(payload) => {
            mk_state_change_watchlist_notification_payload(product, payload, &ProductState::Unknown)
        }
        ProductDomainEventPayload::PriceDiscovered(payload) => {
            mk_price_change_watchlist_notification_payload(
                product,
                Default::default(),
                payload.prices(),
            )
        }
        ProductDomainEventPayload::PriceDropped(payload) => {
            mk_price_change_watchlist_notification_payload(
                product,
                payload.old_prices(),
                payload.new_prices(),
            )
        }
        ProductDomainEventPayload::PriceIncreased(payload) => {
            mk_price_change_watchlist_notification_payload(
                product,
                payload.old_prices(),
                payload.new_prices(),
            )
        }
        ProductDomainEventPayload::PriceRemoved(payload) => {
            mk_price_change_watchlist_notification_payload(
                product,
                payload.old_prices(),
                Default::default(),
            )
        }
    }
}

fn mk_created_watchlist_notification_payload(
    product: &Product,
    payload: &ProductCreatedDomainEventPayload,
) -> NotificationPayload {
    NotificationPayload::Watchlist {
        product_id: product.product_id,
        shop_id: product.shop_id,
        shops_product_id: product.shops_product_id.clone(),
        shop_slug_id: product.shop_slug_id.clone(),
        product_slug_id: product.product_slug_id.clone(),
        shop_name: product.shop_name.clone(),
        title: product.titles(),
        watchlist_payload: NotificationWatchlistPayload::StateChange {
            old_state: ProductState::Unknown,
            new_state: payload.state,
        },
    }
}

fn mk_state_change_watchlist_notification_payload(
    product: &Product,
    payload: &ProductStateChangeDomainEventPayload,
    new_state: &ProductState,
) -> NotificationPayload {
    NotificationPayload::Watchlist {
        product_id: product.product_id,
        shop_id: product.shop_id,
        shops_product_id: product.shops_product_id.clone(),
        shop_slug_id: product.shop_slug_id.clone(),
        product_slug_id: product.product_slug_id.clone(),
        shop_name: product.shop_name.clone(),
        title: product.titles(),
        watchlist_payload: NotificationWatchlistPayload::StateChange {
            old_state: payload.old_state,
            new_state: *new_state,
        },
    }
}

fn mk_price_change_watchlist_notification_payload(
    product: &Product,
    old_price: HashMap<Currency, MonetaryAmount>,
    new_price: HashMap<Currency, MonetaryAmount>,
) -> NotificationPayload {
    NotificationPayload::Watchlist {
        product_id: product.product_id,
        shop_id: product.shop_id,
        shops_product_id: product.shops_product_id.clone(),
        shop_slug_id: product.shop_slug_id.clone(),
        product_slug_id: product.product_slug_id.clone(),
        shop_name: product.shop_name.clone(),
        title: product.titles(),
        watchlist_payload: NotificationWatchlistPayload::PriceChange {
            old_price,
            new_price,
        },
    }
}
