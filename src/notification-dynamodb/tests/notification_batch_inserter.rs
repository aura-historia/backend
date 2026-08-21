mod support;

use domain_primitives::event_id::EventId;
use notification_service::ports::{
    all_notifications_reader::AllNotificationsReader,
    notification_batch_inserter::NotificationBatchInserter,
};
use product_core::product_id::ProductId;
use test_api::*;
use user_core::user_id::UserId;

#[aura_integration_test(services = [DynamoDB()])]
fn should_insert_notifications_in_batch() {
    let inserter = support::batch_inserter().await;
    let reader = support::all_reader().await;
    let user_id = UserId::new();
    let notifications = (0..3)
        .map(|_| support::product_notification(user_id, EventId::new(), ProductId::new()))
        .collect::<Vec<_>>();

    let inserted = inserter.insert_many(&notifications).await.unwrap();
    let items = reader.list_all_by_user(&user_id).await.unwrap();

    assert_eq!(3, inserted.len());
    assert_eq!(3, items.len());
}
