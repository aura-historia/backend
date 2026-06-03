use common::{batch::Batch, event_id::EventId, pagination::cursor::Cursor, user_id::UserId};
use fake::{Fake, Faker};
use notification::dynamodb::{
    notification_record::{NotificationRecord, mk_pk},
    notification_record_update::NotificationRecordUpdate,
    notification_type_record::NotificationTypeRecord,
    repository::{NotificationDynamoDbRepository, NotificationDynamoDbRepositoryImpl},
};
use test_api::*;
use time::OffsetDateTime;

async fn get_repository() -> NotificationDynamoDbRepositoryImpl<'static> {
    NotificationDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1")
}

#[localstack_test(services = [DynamoDB()])]
fn should_put_notification_record() {
    let repository = get_repository().await;

    let expected = Faker.fake::<NotificationRecord>();
    let _ = repository
        .put_notification_record(expected.clone())
        .await
        .unwrap();

    let actual = repository
        .get_notification_record(&expected.user_id, &expected.origin_event_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected, actual);
}

#[localstack_test(services = [DynamoDB()])]
fn should_return_none_when_notification_record_not_exists() {
    let repository = get_repository().await;

    let actual = repository
        .get_notification_record(&Faker.fake(), &EventId::new())
        .await
        .unwrap();

    assert!(actual.is_none());
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_notification_records_when_not_bound_for_scan_index_forward_true() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![NotificationRecord; 10];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_notification_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .query_notification_records(
            &user_id,
            &Cursor {
                size: 100,
                search_after: None,
            },
            true,
        )
        .await
        .unwrap();
    assert_eq!(records.len(), actual.len());
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_notification_records_when_not_bound_for_scan_index_forward_false() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![NotificationRecord; 10];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_notification_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .query_notification_records(
            &user_id,
            &Cursor {
                size: 100,
                search_after: None,
            },
            false,
        )
        .await
        .unwrap();
    assert_eq!(records.len(), actual.len());
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_notification_records_and_respect_limit() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![NotificationRecord; 20];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_notification_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .query_notification_records(
            &user_id,
            &Cursor {
                size: 5,
                search_after: None,
            },
            true,
        )
        .await
        .unwrap();

    assert_eq!(5, actual.len());
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_notification_records_with_cursor_for_scan_index_forward_true() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![NotificationRecord; 10];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_notification_record(record.clone())
            .await
            .unwrap();
    }

    // Sort records by sk ordering used by the repository
    records.sort_by(|a, b| a.sk.cmp(&b.sk));

    // Use 5th record's origin_event_id as cursor
    let cursor_origin_event_id = records[4].origin_event_id;

    let actual = repository
        .query_notification_records(
            &user_id,
            &Cursor {
                size: 100,
                search_after: Some(cursor_origin_event_id),
            },
            true,
        )
        .await
        .unwrap();

    // Should get records after the 5th one (sk order, ascending)
    let expected = records.into_iter().skip(5).collect::<Vec<_>>();
    assert_eq!(expected.len(), actual.len());
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_notification_records_with_cursor_for_scan_index_forward_false() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![NotificationRecord; 10];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_notification_record(record.clone())
            .await
            .unwrap();
    }

    // Sort records by sk ordering used by the repository
    records.sort_by(|a, b| a.sk.cmp(&b.sk));

    // Use 6th record's origin_event_id as cursor (0-indexed: index 5)
    let cursor_origin_event_id = records[5].origin_event_id;

    let actual = repository
        .query_notification_records(
            &user_id,
            &Cursor {
                size: 100,
                search_after: Some(cursor_origin_event_id),
            },
            false,
        )
        .await
        .unwrap();

    // Should get records before the 6th one in sk order (reversed)
    let expected = records.into_iter().take(5).rev().collect::<Vec<_>>();
    assert_eq!(expected.len(), actual.len());
}

#[localstack_test(services = [DynamoDB()])]
fn should_count_notification_records() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![NotificationRecord; 15];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_notification_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .count_notification_records(
            &user_id,
            &Cursor {
                size: 10,
                search_after: None,
            },
            true,
        )
        .await
        .unwrap();

    assert_eq!(15, actual);
}

#[localstack_test(services = [DynamoDB()])]
fn should_set_seen_true_for_update() {
    let repository = get_repository().await;

    let mut initial = Faker.fake::<NotificationRecord>();
    initial.seen = false;
    let _ = repository
        .put_notification_record(initial.clone())
        .await
        .unwrap();

    let updated = OffsetDateTime::now_utc();
    let _ = repository
        .update_notification_record(
            &initial.user_id,
            &initial.origin_event_id,
            NotificationRecordUpdate {
                seen: Some(true),
                notification_type: None,
                updated,
                updated_by: Faker.fake(),
            },
        )
        .await
        .unwrap();

    let actual = repository
        .get_notification_record(&initial.user_id, &initial.origin_event_id)
        .await
        .unwrap()
        .unwrap();

    assert!(actual.seen);
}

#[localstack_test(services = [DynamoDB()])]
fn should_set_seen_false_for_update() {
    let repository = get_repository().await;

    let mut initial = Faker.fake::<NotificationRecord>();
    initial.seen = true;
    let _ = repository
        .put_notification_record(initial.clone())
        .await
        .unwrap();

    let updated = OffsetDateTime::now_utc();
    let _ = repository
        .update_notification_record(
            &initial.user_id,
            &initial.origin_event_id,
            NotificationRecordUpdate {
                seen: Some(false),
                notification_type: None,
                updated,
                updated_by: Faker.fake(),
            },
        )
        .await
        .unwrap();

    let actual = repository
        .get_notification_record(&initial.user_id, &initial.origin_event_id)
        .await
        .unwrap()
        .unwrap();

    assert!(!actual.seen);
}

#[localstack_test(services = [DynamoDB()])]
fn should_set_type_email_for_update() {
    let repository = get_repository().await;

    let mut initial = Faker.fake::<NotificationRecord>();
    initial.notification_type = None;
    let _ = repository
        .put_notification_record(initial.clone())
        .await
        .unwrap();

    let updated = OffsetDateTime::now_utc();
    let _ = repository
        .update_notification_record(
            &initial.user_id,
            &initial.origin_event_id,
            NotificationRecordUpdate {
                seen: None,
                notification_type: Some(NotificationTypeRecord::Email),
                updated,
                updated_by: Faker.fake(),
            },
        )
        .await
        .unwrap();

    let actual = repository
        .get_notification_record(&initial.user_id, &initial.origin_event_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        NotificationTypeRecord::Email,
        actual.notification_type.unwrap()
    );
}

#[localstack_test(services = [DynamoDB()])]
fn should_put_notification_records_when_single_batch() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![NotificationRecord; 25];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
    }

    let batch: Batch<NotificationRecord, 25> = records.clone().try_into().unwrap();
    let _ = repository.put_notification_records(batch).await.unwrap();

    let actual = repository
        .query_notification_records(
            &user_id,
            &Cursor {
                size: 100,
                search_after: None,
            },
            true,
        )
        .await
        .unwrap();

    assert_eq!(25, actual.len());
}

#[localstack_test(services = [DynamoDB()])]
fn should_put_notification_records_when_less_than_25_records() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![NotificationRecord; 5];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
    }

    let batch: Batch<NotificationRecord, 25> = records.clone().try_into().unwrap();
    let _ = repository.put_notification_records(batch).await.unwrap();

    let actual = repository
        .query_notification_records(
            &user_id,
            &Cursor {
                size: 100,
                search_after: None,
            },
            true,
        )
        .await
        .unwrap();

    assert_eq!(5, actual.len());
}

#[localstack_test(services = [DynamoDB()])]
fn should_put_notification_records_for_different_users() {
    let repository = get_repository().await;
    let user_id_1 = UserId::new();
    let user_id_2 = UserId::new();

    let mut records_user_1 = fake::vec![NotificationRecord; 10];
    for record in &mut records_user_1 {
        record.pk = mk_pk(&user_id_1);
        record.user_id = user_id_1;
    }

    let mut records_user_2 = fake::vec![NotificationRecord; 8];
    for record in &mut records_user_2 {
        record.pk = mk_pk(&user_id_2);
        record.user_id = user_id_2;
    }

    let mut mixed: Vec<NotificationRecord> = records_user_1
        .iter()
        .cloned()
        .chain(records_user_2.iter().cloned())
        .collect();
    mixed.truncate(25);

    let batch: Batch<NotificationRecord, 25> = mixed.try_into().unwrap();
    let _ = repository.put_notification_records(batch).await.unwrap();

    let actual_user_1 = repository
        .query_notification_records(
            &user_id_1,
            &Cursor {
                size: 100,
                search_after: None,
            },
            true,
        )
        .await
        .unwrap();

    let actual_user_2 = repository
        .query_notification_records(
            &user_id_2,
            &Cursor {
                size: 100,
                search_after: None,
            },
            true,
        )
        .await
        .unwrap();

    assert_eq!(10, actual_user_1.len());
    assert_eq!(8, actual_user_2.len());
}

#[localstack_test(services = [DynamoDB()])]
fn should_delete_notification_record() {
    let repository = get_repository().await;

    let record: NotificationRecord = Faker.fake();
    let _ = repository
        .put_notification_record(record.clone())
        .await
        .unwrap();

    // Verify it exists before delete
    let before = repository
        .get_notification_record(&record.user_id, &record.origin_event_id)
        .await
        .unwrap();
    assert!(before.is_some());

    // Delete
    let _ = repository
        .delete_notification_record(&record.user_id, &record.origin_event_id)
        .await
        .unwrap();

    // Verify it no longer exists
    let after = repository
        .get_notification_record(&record.user_id, &record.origin_event_id)
        .await
        .unwrap();
    assert!(after.is_none());
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_all_notification_records() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![NotificationRecord; 5];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_notification_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .query_all_notification_records(&user_id)
        .await
        .unwrap();

    assert_eq!(5, actual.len());
}

#[localstack_test(services = [DynamoDB()])]
fn should_delete_notification_records_batch() {
    let repository = get_repository().await;
    let user_id = UserId::new();

    let mut records = fake::vec![NotificationRecord; 3];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        let _ = repository
            .put_notification_record(record.clone())
            .await
            .unwrap();
    }

    let ids: Vec<EventId> = records.iter().map(|r| r.origin_event_id).collect();
    let batch = Batch::try_from_iter(ids).unwrap();
    let _ = repository
        .delete_notification_records(&user_id, &batch)
        .await
        .unwrap();

    // Verify all records are deleted
    let remaining = repository
        .query_all_notification_records(&user_id)
        .await
        .unwrap();
    assert!(remaining.is_empty());
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_product_notification_records_when_product_has_notifications() {
    let repository = get_repository().await;
    let user_id = UserId::new();
    let product_id = common::product_id::ProductId::new();

    let mut records = fake::vec![NotificationRecord; 3];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        record.product_id = Some(product_id);
        record.lsi2_sk = Some(notification::dynamodb::notification_record::mk_lsi2_sk(
            &product_id,
            &record.origin_event_id,
        ));
        let _ = repository
            .put_notification_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .query_product_notification_records(&user_id, &product_id, None, true)
        .await
        .unwrap();

    assert_eq!(3, actual.len());
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_product_notification_records_empty_when_no_notifications_for_product() {
    let repository = get_repository().await;
    let user_id = UserId::new();
    let product_id = common::product_id::ProductId::new();
    let other_product_id = common::product_id::ProductId::new();

    let mut records = fake::vec![NotificationRecord; 3];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        record.product_id = Some(other_product_id);
        record.lsi2_sk = Some(notification::dynamodb::notification_record::mk_lsi2_sk(
            &other_product_id,
            &record.origin_event_id,
        ));
        let _ = repository
            .put_notification_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .query_product_notification_records(&user_id, &product_id, None, true)
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_product_notification_records_with_limit() {
    let repository = get_repository().await;
    let user_id = UserId::new();
    let product_id = common::product_id::ProductId::new();

    let mut records = fake::vec![NotificationRecord; 5];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        record.product_id = Some(product_id);
        record.lsi2_sk = Some(notification::dynamodb::notification_record::mk_lsi2_sk(
            &product_id,
            &record.origin_event_id,
        ));
        let _ = repository
            .put_notification_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .query_product_notification_records(&user_id, &product_id, Some(2), true)
        .await
        .unwrap();

    assert_eq!(2, actual.len());
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_product_notification_records_with_limit_returns_all_when_none() {
    let repository = get_repository().await;
    let user_id = UserId::new();
    let product_id = common::product_id::ProductId::new();

    let mut records = fake::vec![NotificationRecord; 5];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        record.product_id = Some(product_id);
        record.lsi2_sk = Some(notification::dynamodb::notification_record::mk_lsi2_sk(
            &product_id,
            &record.origin_event_id,
        ));
        let _ = repository
            .put_notification_record(record.clone())
            .await
            .unwrap();
    }

    let actual = repository
        .query_product_notification_records(&user_id, &product_id, None, true)
        .await
        .unwrap();

    assert_eq!(5, actual.len());
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_product_notification_records_scan_index_forward_false_returns_latest_first() {
    let repository = get_repository().await;
    let user_id = UserId::new();
    let product_id = common::product_id::ProductId::new();

    let mut records = fake::vec![NotificationRecord; 5];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        record.product_id = Some(product_id);
        record.lsi2_sk = Some(notification::dynamodb::notification_record::mk_lsi2_sk(
            &product_id,
            &record.origin_event_id,
        ));
        let _ = repository
            .put_notification_record(record.clone())
            .await
            .unwrap();
    }

    let forward = repository
        .query_product_notification_records(&user_id, &product_id, Some(1), true)
        .await
        .unwrap();

    let backward = repository
        .query_product_notification_records(&user_id, &product_id, Some(1), false)
        .await
        .unwrap();

    assert_eq!(1, forward.len());
    assert_eq!(1, backward.len());
    assert_ne!(
        forward[0].origin_event_id, backward[0].origin_event_id,
        "Forward and backward should return different records (first vs last)"
    );
}

#[localstack_test(services = [DynamoDB()])]
fn should_query_product_notification_records_only_for_matching_product() {
    let repository = get_repository().await;
    let user_id = UserId::new();
    let product_id_a = common::product_id::ProductId::new();
    let product_id_b = common::product_id::ProductId::new();

    let mut records_a = fake::vec![NotificationRecord; 3];
    for record in &mut records_a {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        record.product_id = Some(product_id_a);
        record.lsi2_sk = Some(notification::dynamodb::notification_record::mk_lsi2_sk(
            &product_id_a,
            &record.origin_event_id,
        ));
        let _ = repository
            .put_notification_record(record.clone())
            .await
            .unwrap();
    }

    let mut records_b = fake::vec![NotificationRecord; 2];
    for record in &mut records_b {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
        record.product_id = Some(product_id_b);
        record.lsi2_sk = Some(notification::dynamodb::notification_record::mk_lsi2_sk(
            &product_id_b,
            &record.origin_event_id,
        ));
        let _ = repository
            .put_notification_record(record.clone())
            .await
            .unwrap();
    }

    let actual_a = repository
        .query_product_notification_records(&user_id, &product_id_a, None, true)
        .await
        .unwrap();

    let actual_b = repository
        .query_product_notification_records(&user_id, &product_id_b, None, true)
        .await
        .unwrap();

    assert_eq!(3, actual_a.len());
    assert_eq!(2, actual_b.len());
}
