mod support;

use domain_primitives::event_id::EventId;
use notification_service::ports::{
    notification_repository::NotificationRepository,
    product_notifications_reader::ProductNotificationsReader,
};
use product_core::product_id::ProductId;
use test_api::*;
use user_core::user_id::UserId;

#[aura_integration_test(services = [DynamoDB()])]
fn should_list_notifications_for_product_with_limit() {
    let repository = support::repository().await;
    let reader = support::product_reader().await;
    let user_id = UserId::new();
    let product_id = ProductId::new();
    let other_product_id = ProductId::new();
    for _ in 0..3 {
        let notification = support::product_notification(user_id, EventId::new(), product_id);
        repository.insert(&notification).await.unwrap();
    }
    let other = support::product_notification(user_id, EventId::new(), other_product_id);
    repository.insert(&other).await.unwrap();

    let items = reader
        .list_by_product(&user_id, &product_id, Some(2), true)
        .await
        .unwrap();

    assert_eq!(2, items.len());
    assert!(items.iter().all(|item| item.user_id == user_id));
}
