use fake::{Fake, Faker};
use search_filter_dynamodb::{
    repository::{SearchFilterDynamoDbRepository, SearchFilterDynamoDbRepositoryImpl},
    search_filter_record::SearchFilterRecord,
    search_filter_record_update::SearchFilterRecordUpdate,
};
use test_api::*;
use time::OffsetDateTime;

#[localstack_test(services = [DynamoDB()])]
async fn should_update_search_filter_record() {
    let repository =
        SearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");

    let record = Faker.fake::<SearchFilterRecord>();
    let _ = repository
        .put_search_filter_record(record.clone())
        .await
        .unwrap();

    let updated = OffsetDateTime::now_utc();
    let update = SearchFilterRecordUpdate {
        item_query: Some("boopel boop doop".try_into().unwrap()),
        shop_name_query: None,
        price_query: None,
        state_query: None,
        created_query: None,
        updated_query: None,
        language: None,
        currency: None,
        updated,
    };
    let _ = repository
        .update_search_filter_record(&record.user_id, &record.search_filter_id, update)
        .await
        .unwrap();

    let mut expected = record.clone();
    expected.item_query = "boopel boop doop".try_into().unwrap();
    expected.updated = updated;

    let actual = repository
        .get_search_filter_record(&record.user_id, &record.search_filter_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected, actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_update_multiple_times() {
    let repository =
        SearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");

    let record = Faker.fake::<SearchFilterRecord>();
    let _ = repository
        .put_search_filter_record(record.clone())
        .await
        .unwrap();

    for _ in 0..100 {
        let _ = repository
            .update_search_filter_record(&record.user_id, &record.search_filter_id, Faker.fake())
            .await
            .unwrap();
    }

    let _ = repository
        .get_search_filter_record(&record.user_id, &record.search_filter_id)
        .await
        .unwrap()
        .unwrap();
}
