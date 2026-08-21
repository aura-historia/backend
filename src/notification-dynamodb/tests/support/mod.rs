#![allow(dead_code)]

use domain_primitives::event_id::EventId;
use notification_core::notification::{
    Notification, NotificationPartnerApplicationPayload, NotificationPayload,
    NotificationWatchlistPayload,
};
use notification_dynamodb::{
    all_notifications_reader::DynamoDbAllNotificationsReader,
    batch_writer::DynamoDbNotificationBatchInserter,
    conditional_writer::ConditionalDynamoDbNotificationWriter,
    deleter::DynamoDbNotificationDeleter,
    list_notifications_reader::DynamoDbListNotificationsReader,
    product_notifications_reader::DynamoDbProductNotificationsReader,
    repository::NotificationDynamoDbRepository,
};
use product_core::{
    product_id::ProductId, product_slug_id::ProductSlugId, product_state::ProductState,
    shops_product_id::ShopsProductId,
};
use shop_core::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
use shop_partner_core::partner_shop_application_id::PartnerShopApplicationId;
use test_api::get_dynamodb_client;
use url::Url;
use user_core::user_id::UserId;

pub async fn repository() -> NotificationDynamoDbRepository<'static> {
    NotificationDynamoDbRepository::new(get_dynamodb_client().await, "table_1")
}

pub async fn list_reader() -> DynamoDbListNotificationsReader<'static> {
    DynamoDbListNotificationsReader::new(get_dynamodb_client().await, "table_1")
}

pub async fn all_reader() -> DynamoDbAllNotificationsReader<'static> {
    DynamoDbAllNotificationsReader::new(get_dynamodb_client().await, "table_1")
}

pub async fn product_reader() -> DynamoDbProductNotificationsReader<'static> {
    DynamoDbProductNotificationsReader::new(get_dynamodb_client().await, "table_1")
}

pub async fn batch_inserter() -> DynamoDbNotificationBatchInserter<'static> {
    DynamoDbNotificationBatchInserter::new(get_dynamodb_client().await, "table_1")
}

pub async fn conditional_writer() -> ConditionalDynamoDbNotificationWriter {
    ConditionalDynamoDbNotificationWriter::new(get_dynamodb_client().await.clone(), "table_1")
}

pub async fn deleter() -> DynamoDbNotificationDeleter<'static> {
    DynamoDbNotificationDeleter::new(get_dynamodb_client().await, "table_1")
}

pub fn product_notification(
    user_id: UserId,
    origin_event_id: EventId,
    product_id: ProductId,
) -> Notification {
    Notification::new(user_id, origin_event_id, product_payload(product_id), true)
}

pub fn partner_notification(user_id: UserId, origin_event_id: EventId) -> Notification {
    Notification::new(
        user_id,
        origin_event_id,
        NotificationPayload::PartnerApplication {
            shop_name: ShopName::from("Test Shop"),
            image: None,
            partner_application_payload: NotificationPartnerApplicationPayload::Approved {
                partner_application_id: PartnerShopApplicationId::new(),
            },
        },
        true,
    )
}

fn product_payload(product_id: ProductId) -> NotificationPayload {
    NotificationPayload::Watchlist {
        product_id,
        shop_id: ShopId::new(),
        shops_product_id: ShopsProductId::new(),
        shop_slug_id: ShopSlugId::from("test-shop"),
        product_slug_id: ProductSlugId::from("test-product"),
        shop_name: ShopName::from("Test Shop"),
        title: None,
        image: None,
        url: test_url("source"),
        view_url: test_url("view"),
        watchlist_payload: NotificationWatchlistPayload::StateChange {
            old_state: ProductState::Available,
            new_state: ProductState::Sold,
        },
    }
}

fn test_url(path: &str) -> Url {
    match Url::parse(&format!("https://example.test/{path}")) {
        Ok(url) => url,
        Err(error) => panic!("test URL must parse: {error}"),
    }
}
