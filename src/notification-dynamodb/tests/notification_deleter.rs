mod support;

use domain_primitives::event_id::EventId;
use notification_service::ports::{
    all_notifications_reader::AllNotificationsReader, notification_deleter::NotificationDeleter,
    notification_repository::NotificationRepository,
};
use product_core::product_id::ProductId;
use test_api::*;
use user_core::user_id::UserId;

#[aura_integration_test(services = [DynamoDB()])]
fn should_delete_one_notification() {
    let repository = support::repository().await;
    let deleter = support::deleter().await;
    let user_id = UserId::new();
    let origin_event_id = EventId::new();
    let notification = support::product_notification(user_id, origin_event_id, ProductId::new());
    repository.insert(&notification).await.unwrap();

    deleter
        .delete_by_origin_event_id(&user_id, &origin_event_id)
        .await
        .unwrap();
    let found = repository
        .find_by_origin_event_id(&user_id, &origin_event_id)
        .await
        .unwrap();

    assert!(found.is_none());
}

#[aura_integration_test(services = [DynamoDB()])]
fn should_delete_many_notifications() {
    let repository = support::repository().await;
    let deleter = support::deleter().await;
    let reader = support::all_reader().await;
    let user_id = UserId::new();
    let ids = (0..3).map(|_| EventId::new()).collect::<Vec<_>>();
    for id in &ids {
        let notification = support::product_notification(user_id, *id, ProductId::new());
        repository.insert(&notification).await.unwrap();
    }

    deleter
        .delete_many_by_origin_event_id(&user_id, &ids)
        .await
        .unwrap();
    let remaining = reader.list_all_by_user(&user_id).await.unwrap();

    assert!(remaining.is_empty());
}
