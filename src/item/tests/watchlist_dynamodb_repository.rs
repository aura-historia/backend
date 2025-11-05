use std::time::Duration;

use common::{item_id::ItemId, pagination::cursor::Cursor, user_id::UserId};
use fake::{Fake, Faker};
use item::watchlist::dynamodb::{
    record::{WatchlistItemRecord, mk_gsi1_pk, mk_gsi1_sk, mk_pk},
    record_update::WatchlistItemRecordUpdate,
    repository::{WatchlistItemDynamoDbRepository, WatchlistItemDynamoDbRepositoryImpl},
};
use test_api::*;
use time::OffsetDateTime;

async fn get_repository() -> WatchlistItemDynamoDbRepositoryImpl<'static> {
    WatchlistItemDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1")
}

#[localstack_test(services = [DynamoDB()])]
fn should_put_watchlist_record() {
    let repository = get_repository().await;

    let expected = Faker.fake::<WatchlistItemRecord>();
    let _ = repository
        .put_watchlist_record(expected.clone())
        .await
        .unwrap();

    let actual = repository
        .get_watchlist_record(
            &expected.user_id,
            &expected.shop_id,
            &expected.shops_item_id,
        )
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected, actual);
}

#[localstack_test(services = [DynamoDB()])]
fn should_delete_watchlist_record() {
    let repository = get_repository().await;

    let expected = Faker.fake::<WatchlistItemRecord>();
    let _ = repository
        .put_watchlist_record(expected.clone())
        .await
        .unwrap();

    let _ = repository
        .delete_watchlist_record(
            &expected.user_id,
            &expected.shop_id,
            &expected.shops_item_id,
        )
        .await
        .unwrap();

    let actual = repository
        .get_watchlist_record(
            &expected.user_id,
            &expected.shop_id,
            &expected.shops_item_id,
        )
        .await
        .unwrap();
    assert!(actual.is_none());
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_watchlist_records_when_lower_bounded_created_for_scan_index_true() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![WatchlistItemRecord; 42];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_watchlist_record(record.clone())
            .await
            .unwrap();
    }

    let expected = records.clone().into_iter().skip(37).collect::<Vec<_>>();

    let actual = repository
        .query_watchlist_records(
            &user_id,
            &Cursor {
                size: 100,
                search_after: Some(records.get(36).unwrap().created),
            },
            true,
        )
        .await
        .unwrap();
    assert_eq!(expected, actual);
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_watchlist_records_when_higher_bounded_created_for_scan_index_false() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![WatchlistItemRecord; 42];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_watchlist_record(record.clone())
            .await
            .unwrap();
    }

    let expected = records
        .clone()
        .into_iter()
        .take(5)
        .rev()
        .collect::<Vec<_>>();

    let actual = repository
        .query_watchlist_records(
            &user_id,
            &Cursor {
                size: 100,
                search_after: Some(records.get(5).unwrap().created),
            },
            false,
        )
        .await
        .unwrap();
    assert_eq!(expected, actual);
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_watchlist_records_when_not_bound_created_for_scan_index_true() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![WatchlistItemRecord; 42];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_watchlist_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .query_watchlist_records(
            &user_id,
            &Cursor {
                size: 100,
                search_after: None,
            },
            true,
        )
        .await
        .unwrap();
    assert_eq!(records, actual);
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_watchlist_records_and_respect_limit_for_scan_index_true() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![WatchlistItemRecord; 42];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_watchlist_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .query_watchlist_records(
            &user_id,
            &Cursor {
                size: 10,
                search_after: None,
            },
            true,
        )
        .await
        .unwrap();

    assert_eq!(10, actual.len());
    assert_eq!(records.into_iter().take(10).collect::<Vec<_>>(), actual);
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_watchlist_records_and_respect_limit_for_scan_index_false() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![WatchlistItemRecord; 42];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_watchlist_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .query_watchlist_records(
            &user_id,
            &Cursor {
                size: 10,
                search_after: None,
            },
            false,
        )
        .await
        .unwrap();

    assert_eq!(10, actual.len());
    assert_eq!(
        records.into_iter().skip(32).rev().collect::<Vec<_>>(),
        actual
    );
}

#[localstack_test(services = [DynamoDB()])]
fn should_set_notifications_true_for_update() {
    let repository = get_repository().await;

    let mut initial = Faker.fake::<WatchlistItemRecord>();
    initial.notifications = false;
    let _ = repository
        .put_watchlist_record(initial.clone())
        .await
        .unwrap();

    let updated = OffsetDateTime::now_utc();
    let _ = repository
        .update_watchlist_record(
            &initial.user_id,
            &initial.shop_id,
            &initial.shops_item_id,
            WatchlistItemRecordUpdate {
                gsi1_pk: Some(mk_gsi1_pk(&initial.item_id)),
                gsi1_sk: Some(mk_gsi1_sk(&initial.user_id)),
                notifications: Some(true),
                updated,
            },
        )
        .await
        .unwrap();

    let actual = repository
        .get_watchlist_record(&initial.user_id, &initial.shop_id, &initial.shops_item_id)
        .await
        .unwrap()
        .unwrap();

    assert!(actual.notifications);
    assert!(actual.gsi1_pk.is_some());
    assert!(actual.gsi1_sk.is_some());
}

#[localstack_test(services = [DynamoDB()])]
fn should_set_notifications_false_for_update() {
    let repository = get_repository().await;

    let mut initial = Faker.fake::<WatchlistItemRecord>();
    initial.notifications = true;
    let _ = repository
        .put_watchlist_record(initial.clone())
        .await
        .unwrap();

    let updated = OffsetDateTime::now_utc();
    let _ = repository
        .update_watchlist_record(
            &initial.user_id,
            &initial.shop_id,
            &initial.shops_item_id,
            WatchlistItemRecordUpdate {
                gsi1_pk: None,
                gsi1_sk: None,
                notifications: Some(false),
                updated,
            },
        )
        .await
        .unwrap();

    let actual = repository
        .get_watchlist_record(&initial.user_id, &initial.shop_id, &initial.shops_item_id)
        .await
        .unwrap()
        .unwrap();

    assert!(!actual.notifications);
    assert!(actual.gsi1_pk.is_none());
    assert!(actual.gsi1_sk.is_none());
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_users_with_notifications_enabled() {
    let repository = get_repository().await;

    let item_id = ItemId::new();
    for mut record in fake::vec![WatchlistItemRecord; 42] {
        record.notifications = true;
        record.gsi1_pk = Some(mk_gsi1_pk(&item_id));
        record.gsi1_sk = Some(mk_gsi1_sk(&record.user_id));
        record.item_id = item_id;
        let _ = repository.put_watchlist_record(record).await.unwrap();
    }

    // civilians
    for mut record in fake::vec![WatchlistItemRecord; 15] {
        record.notifications = false;
        let _ = repository.put_watchlist_record(record).await.unwrap();
    }

    // wait for gsi
    tokio::time::sleep(Duration::from_secs(5)).await;

    let actual = repository
        .query_user_records_with_notifications(&item_id)
        .await
        .unwrap();

    assert_eq!(42, actual.len());
}

#[localstack_test(services = [DynamoDB()])]
fn should_count_watchlist_records_and_respect_limit_for_scan_index_true() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![WatchlistItemRecord; 42];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_watchlist_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .count_watchlist_records(
            &user_id,
            &Cursor {
                size: 10,
                search_after: None,
            },
            true,
        )
        .await
        .unwrap();

    assert_eq!(42, actual);
}

#[localstack_test(services = [DynamoDB()])]
fn should_count_watchlist_records_and_respect_limit_for_scan_index_false() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![WatchlistItemRecord; 420];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_watchlist_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .count_watchlist_records(
            &user_id,
            &Cursor {
                size: 10,
                search_after: None,
            },
            false,
        )
        .await
        .unwrap();

    assert_eq!(420, actual);
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_watchlist_records_all_for_scan_index_true() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut expected = fake::vec![WatchlistItemRecord; 42];
    for record in &mut expected {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_watchlist_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .query_watchlist_records_all(&user_id, true)
        .await
        .unwrap();
    assert_eq!(expected.len(), actual.len());
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_watchlist_records_all_for_scan_index_false() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut expected = fake::vec![WatchlistItemRecord; 42];
    for record in &mut expected {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_watchlist_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .query_watchlist_records_all(&user_id, false)
        .await
        .unwrap();
    assert_eq!(expected.len(), actual.len());
}
