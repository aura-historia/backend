use common::{pagination::cursor::Cursor, user_id::UserId};
use fake::{Fake, Faker};
use item_watchlist::{
    record::{WatchlistItemRecord, mk_pk},
    record_update::WatchlistItemRecordUpdate,
    repository::{WatchlistItemDynamoDbRepository, WatchlistItemDynamoDbRepositoryImpl},
};
use test_api::*;

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
        .get_watchlist_record(&expected.user_id, &expected.created)
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
        .delete_watchlist_record(&expected.user_id, &expected.created)
        .await
        .unwrap();

    let actual = repository
        .get_watchlist_record(&expected.user_id, &expected.created)
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
fn should_update_watchlist_record() {
    let repository = get_repository().await;

    let initial = Faker.fake::<WatchlistItemRecord>();
    let _ = repository
        .put_watchlist_record(initial.clone())
        .await
        .unwrap();

    let _ = repository
        .update_watchlist_record(
            &initial.user_id,
            &initial.created,
            WatchlistItemRecordUpdate {
                notifications: Some(!initial.notifications),
            },
        )
        .await
        .unwrap();

    let actual = repository
        .get_watchlist_record(&initial.user_id, &initial.created)
        .await
        .unwrap()
        .unwrap();

    let mut expected = initial;
    expected.notifications = !expected.notifications;
    assert_eq!(expected, actual);
}
