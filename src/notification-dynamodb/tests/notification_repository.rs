mod support;

use common::{event_id::EventId, product_id::ProductId, user_id::UserId};
use notification_core::notification_type::NotificationType;
use notification_service::ports::notification_repository::NotificationRepository;
use test_api::*;

#[aura_integration_test(services = [DynamoDB()])]
fn should_insert_find_and_update_notification() {
    let repository = support::repository().await;
    let user_id = UserId::new();
    let origin_event_id = EventId::new();
    let mut notification =
        support::product_notification(user_id, origin_event_id, ProductId::new());

    repository.insert(&notification).await.unwrap();

    let persisted = repository
        .find_by_origin_event_id(&user_id, &origin_event_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(origin_event_id, persisted.origin_event_id());
    assert!(!persisted.seen());

    notification.mark_seen(true);
    notification.mark_sent_as(NotificationType::Email);
    let updated = repository.update(&notification).await.unwrap();

    assert!(updated.seen());
    assert_eq!(Some(NotificationType::Email), updated.notification_type());
}

#[aura_integration_test(services = [DynamoDB()])]
fn should_return_none_when_notification_missing() {
    let repository = support::repository().await;

    let actual = repository
        .find_by_origin_event_id(&UserId::new(), &EventId::new())
        .await
        .unwrap();

    assert!(actual.is_none());
}
