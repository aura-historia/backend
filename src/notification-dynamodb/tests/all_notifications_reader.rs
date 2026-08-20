mod support;

use common::{event_id::EventId, user_id::UserId};
use notification_service::ports::{
    all_notifications_reader::AllNotificationsReader,
    notification_repository::NotificationRepository,
};
use product_core::product_id::ProductId;
use test_api::*;

#[aura_integration_test(services = [DynamoDB()])]
fn should_list_all_notifications_for_user() {
    let repository = support::repository().await;
    let reader = support::all_reader().await;
    let user_id = UserId::new();
    for _ in 0..3 {
        let notification = support::product_notification(user_id, EventId::new(), ProductId::new());
        repository.insert(&notification).await.unwrap();
    }

    let items = reader.list_all_by_user(&user_id).await.unwrap();

    assert_eq!(3, items.len());
    assert!(items.iter().all(|item| item.user_id == user_id));
}
