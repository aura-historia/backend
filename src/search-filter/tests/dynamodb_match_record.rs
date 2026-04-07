use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use fake::{Fake, Faker};
use search_filter::dynamodb::repository::{
    UserSearchFilterDynamoDbRepository, UserSearchFilterDynamoDbRepositoryImpl,
};
use search_filter::dynamodb::user_search_filter_match_record::{
    UserSearchFilterMatchRecord, mk_lsi1_sk, mk_pk, mk_sk,
};
use test_api::*;
use time::OffsetDateTime;

async fn get_repository() -> UserSearchFilterDynamoDbRepositoryImpl<'static> {
    UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1")
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_none_when_no_match_record_exists() {
    let actual = get_repository()
        .await
        .get_user_search_filter_match_record(
            &Faker.fake(),
            &Faker.fake(),
            &Faker.fake(),
            &Faker.fake(),
        )
        .await
        .unwrap();

    assert!(actual.is_none());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_put_and_get_match_record() {
    let repository = get_repository().await;
    let expected = Faker.fake::<UserSearchFilterMatchRecord>();
    let _ = repository
        .put_user_search_filter_match_record(expected.clone())
        .await
        .unwrap();

    let actual = repository
        .get_user_search_filter_match_record(
            &expected.user_id,
            &expected.user_search_filter_id,
            &expected.shop_id,
            &expected.shops_product_id,
        )
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected, actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_query_all_match_records_for_user() {
    let repository = get_repository().await;
    let user_id = UserId::new();
    let mut records = fake::vec![UserSearchFilterMatchRecord; 5];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
    }
    for record in records.iter() {
        let _ = repository
            .put_user_search_filter_match_record(record.clone())
            .await
            .unwrap();
    }

    // Also insert a record for a different user to verify isolation
    let other_record = Faker.fake::<UserSearchFilterMatchRecord>();
    let _ = repository
        .put_user_search_filter_match_record(other_record)
        .await
        .unwrap();

    let actual = repository
        .query_user_search_filter_match_records_all(&user_id)
        .await
        .unwrap();

    assert_eq!(5, actual.len());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_query_match_records_for_specific_filter() {
    let repository = get_repository().await;
    let user_id = UserId::new();
    let filter_id = UserSearchFilterId::new();
    let other_filter_id = UserSearchFilterId::new();

    // Create records for the target filter
    for _ in 0..3 {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let mut record = Faker.fake::<UserSearchFilterMatchRecord>();
        record.pk = mk_pk(&user_id);
        record.sk = mk_sk(&filter_id, &shop_id, &shops_product_id);
        record.user_id = user_id;
        record.user_search_filter_id = filter_id;
        record.shop_id = shop_id;
        record.shops_product_id = shops_product_id;
        let _ = repository
            .put_user_search_filter_match_record(record)
            .await
            .unwrap();
    }

    // Create records for a different filter (same user)
    for _ in 0..2 {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let mut record = Faker.fake::<UserSearchFilterMatchRecord>();
        record.pk = mk_pk(&user_id);
        record.sk = mk_sk(&other_filter_id, &shop_id, &shops_product_id);
        record.user_id = user_id;
        record.user_search_filter_id = other_filter_id;
        record.shop_id = shop_id;
        record.shops_product_id = shops_product_id;
        let _ = repository
            .put_user_search_filter_match_record(record)
            .await
            .unwrap();
    }

    let actual = repository
        .query_user_search_filter_match_records_for_filter(&user_id, &filter_id, None, true)
        .await
        .unwrap();

    assert_eq!(3, actual.len());
    assert!(actual.iter().all(|r| r.user_search_filter_id == filter_id));
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_empty_when_no_match_records_for_user() {
    let repository = get_repository().await;

    let actual = repository
        .query_user_search_filter_match_records_all(&UserId::new())
        .await
        .unwrap();

    assert!(actual.is_empty());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_put_batch_match_records() {
    let repository = get_repository().await;
    let user_id = UserId::new();
    let mut records = fake::vec![UserSearchFilterMatchRecord; 5];
    for record in &mut records {
        record.pk = mk_pk(&user_id);
        record.user_id = user_id;
    }

    let batch = common::batch::Batch::try_from(records.clone()).unwrap();
    let _ = repository
        .put_user_search_filter_match_records(batch)
        .await
        .unwrap();

    let actual = repository
        .query_user_search_filter_match_records_all(&user_id)
        .await
        .unwrap();

    assert_eq!(5, actual.len());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_count_match_records_between_dates() {
    let repository = get_repository().await;
    let user_id = UserId::new();
    let filter_id = UserSearchFilterId::new();

    let now = OffsetDateTime::now_utc();
    let one_day_ago = now - time::Duration::days(1);
    let two_days_ago = now - time::Duration::days(2);
    let three_days_ago = now - time::Duration::days(3);

    // Insert 3 records: 3 days ago, 2 days ago, now
    for created in &[three_days_ago, two_days_ago, now] {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let mut record = Faker.fake::<UserSearchFilterMatchRecord>();
        record.pk = mk_pk(&user_id);
        record.sk = mk_sk(&filter_id, &shop_id, &shops_product_id);
        record.lsi1_sk = mk_lsi1_sk(created);
        record.user_id = user_id;
        record.user_search_filter_id = filter_id;
        record.shop_id = shop_id;
        record.shops_product_id = shops_product_id;
        record.created = *created;
        let _ = repository
            .put_user_search_filter_match_record(record)
            .await
            .unwrap();
    }

    // Count between 2 days ago and now → should include the last 2 records
    let count = repository
        .count_user_search_filter_match_records_for_between(&user_id, &two_days_ago, &now)
        .await
        .unwrap();
    assert_eq!(2, count);

    // Count between 1 day ago and now → should include only the 'now' record
    let count = repository
        .count_user_search_filter_match_records_for_between(&user_id, &one_day_ago, &now)
        .await
        .unwrap();
    assert_eq!(1, count);

    // Count between 3 days ago and now → should include all 3
    let count = repository
        .count_user_search_filter_match_records_for_between(&user_id, &three_days_ago, &now)
        .await
        .unwrap();
    assert_eq!(3, count);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_zero_when_no_match_records_between_dates() {
    let repository = get_repository().await;
    let user_id = UserId::new();
    let now = OffsetDateTime::now_utc();
    let one_month_ago = now - time::Duration::days(30);

    let count = repository
        .count_user_search_filter_match_records_for_between(&user_id, &one_month_ago, &now)
        .await
        .unwrap();
    assert_eq!(0, count);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_exclude_records_outside_date_range_for_between() {
    let repository = get_repository().await;
    let user_id = UserId::new();
    let filter_id = UserSearchFilterId::new();

    let now = OffsetDateTime::now_utc();
    let five_days_ago = now - time::Duration::days(5);
    let three_days_ago = now - time::Duration::days(3);
    let two_days_ago = now - time::Duration::days(2);
    let one_day_ago = now - time::Duration::days(1);

    // Insert record outside range (5 days ago)
    let shop_id = ShopId::new();
    let shops_product_id = ShopsProductId::new();
    let mut record = Faker.fake::<UserSearchFilterMatchRecord>();
    record.pk = mk_pk(&user_id);
    record.sk = mk_sk(&filter_id, &shop_id, &shops_product_id);
    record.lsi1_sk = mk_lsi1_sk(&five_days_ago);
    record.user_id = user_id;
    record.user_search_filter_id = filter_id;
    record.shop_id = shop_id;
    record.shops_product_id = shops_product_id;
    record.created = five_days_ago;
    let _ = repository
        .put_user_search_filter_match_record(record)
        .await
        .unwrap();

    // Insert record inside range (2 days ago)
    let shop_id = ShopId::new();
    let shops_product_id = ShopsProductId::new();
    let mut record = Faker.fake::<UserSearchFilterMatchRecord>();
    record.pk = mk_pk(&user_id);
    record.sk = mk_sk(&filter_id, &shop_id, &shops_product_id);
    record.lsi1_sk = mk_lsi1_sk(&two_days_ago);
    record.user_id = user_id;
    record.user_search_filter_id = filter_id;
    record.shop_id = shop_id;
    record.shops_product_id = shops_product_id;
    record.created = two_days_ago;
    let _ = repository
        .put_user_search_filter_match_record(record)
        .await
        .unwrap();

    // Count between 3 days ago and 1 day ago → should only include the 2-day-ago record
    let count = repository
        .count_user_search_filter_match_records_for_between(&user_id, &three_days_ago, &one_day_ago)
        .await
        .unwrap();
    assert_eq!(1, count);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_isolate_users_for_between_count() {
    let repository = get_repository().await;
    let user_id = UserId::new();
    let other_user_id = UserId::new();
    let filter_id = UserSearchFilterId::new();

    let now = OffsetDateTime::now_utc();
    let one_day_ago = now - time::Duration::days(1);

    // Insert record for target user
    let shop_id = ShopId::new();
    let shops_product_id = ShopsProductId::new();
    let mut record = Faker.fake::<UserSearchFilterMatchRecord>();
    record.pk = mk_pk(&user_id);
    record.sk = mk_sk(&filter_id, &shop_id, &shops_product_id);
    record.lsi1_sk = mk_lsi1_sk(&now);
    record.user_id = user_id;
    record.user_search_filter_id = filter_id;
    record.shop_id = shop_id;
    record.shops_product_id = shops_product_id;
    record.created = now;
    let _ = repository
        .put_user_search_filter_match_record(record)
        .await
        .unwrap();

    // Insert record for another user
    let shop_id = ShopId::new();
    let shops_product_id = ShopsProductId::new();
    let mut record = Faker.fake::<UserSearchFilterMatchRecord>();
    record.pk = mk_pk(&other_user_id);
    record.sk = mk_sk(&filter_id, &shop_id, &shops_product_id);
    record.lsi1_sk = mk_lsi1_sk(&now);
    record.user_id = other_user_id;
    record.user_search_filter_id = filter_id;
    record.shop_id = shop_id;
    record.shops_product_id = shops_product_id;
    record.created = now;
    let _ = repository
        .put_user_search_filter_match_record(record)
        .await
        .unwrap();

    // Count for target user → should be 1, not 2
    let count = repository
        .count_user_search_filter_match_records_for_between(&user_id, &one_day_ago, &now)
        .await
        .unwrap();
    assert_eq!(1, count);
}
