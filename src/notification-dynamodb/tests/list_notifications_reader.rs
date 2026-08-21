mod support;

use application::pagination::Cursor;
use domain_primitives::event_id::EventId;
use notification_service::ports::{
    list_notifications_reader::ListNotificationsReader,
    notification_repository::NotificationRepository,
};
use product_core::product_id::ProductId;
use test_api::*;
use user_core::user_id::UserId;

#[aura_integration_test(services = [DynamoDB()])]
fn should_list_and_count_notifications_for_user() {
    let repository = support::repository().await;
    let reader = support::list_reader().await;
    let user_id = UserId::new();
    for _ in 0..3 {
        let notification = support::product_notification(user_id, EventId::new(), ProductId::new());
        repository.insert(&notification).await.unwrap();
    }

    let cursor = Cursor {
        size: 2,
        search_after: None,
    };
    let items = reader.list_by_user(&user_id, &cursor, true).await.unwrap();
    let total = reader
        .count_by_user(&user_id, &Cursor::default(), true)
        .await
        .unwrap();

    assert_eq!(2, items.len());
    assert_eq!(3, total);
    assert!(items.iter().all(|item| item.user_id == user_id));
}
