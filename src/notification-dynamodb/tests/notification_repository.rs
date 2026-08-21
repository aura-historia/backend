mod support;

use domain_primitives::event_id::EventId;
use notification_core::notification_type::NotificationType;
use notification_service::ports::{
    NotificationWriteOutcome, NotificationWriter, all_notifications_reader::AllNotificationsReader,
    notification_repository::NotificationRepository,
};
use product_core::product_id::ProductId;
use test_api::*;
use user_core::user_id::UserId;

#[aura_integration_test(services = [DynamoDB()])]
fn should_insert_find_and_update_notification() {
    let repository = support::repository().await;
    let user_id = UserId::new();
    let origin_event_id = EventId::new();
    let mut notification =
        support::product_notification(user_id, origin_event_id, ProductId::new());

    NotificationRepository::insert(&repository, &notification)
        .await
        .unwrap();

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
fn should_preserve_existing_notification_when_conditional_writer_is_retried() {
    let writer = support::conditional_writer().await;
    let reader = support::all_reader().await;
    let user_id = UserId::new();
    let notification = support::product_notification(user_id, EventId::new(), ProductId::new());

    let initial = writer.insert(&notification).await.unwrap();
    assert!(matches!(initial, NotificationWriteOutcome::Inserted(_)));

    let retry =
        support::product_notification(user_id, notification.origin_event_id(), ProductId::new());
    assert_ne!(notification.notification_id(), retry.notification_id());
    let retry_outcome = writer.insert(&retry).await.unwrap();
    assert_eq!(NotificationWriteOutcome::AlreadyExists, retry_outcome);

    let items = reader.list_all_by_user(&user_id).await.unwrap();
    assert_eq!(1, items.len());
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
