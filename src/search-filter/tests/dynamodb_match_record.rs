use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::user_id::UserId;
use fake::{Fake, Faker};
use search_filter::dynamodb::repository::{
    UserSearchFilterDynamoDbRepository, UserSearchFilterDynamoDbRepositoryImpl,
};
use search_filter::dynamodb::user_search_filter_match_record::{
    UserSearchFilterMatchRecord, mk_pk, mk_sk,
};
use test_api::*;

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
    let filter_id = search_filter::core::user_search_filter_id::UserSearchFilterId::new();
    let other_filter_id = search_filter::core::user_search_filter_id::UserSearchFilterId::new();

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
