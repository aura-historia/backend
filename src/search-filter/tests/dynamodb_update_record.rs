use fake::{Fake, Faker};
use search_filter::dynamodb::{
    repository::{UserSearchFilterDynamoDbRepository, UserSearchFilterDynamoDbRepositoryImpl},
    user_search_filter_record::UserSearchFilterRecord,
    user_search_filter_record_update::UserSearchFilterRecordUpdate,
};
use test_api::*;
use time::OffsetDateTime;

#[localstack_test(services = [DynamoDB()])]
async fn should_update_search_filter_record() {
    let repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");

    let record = Faker.fake::<UserSearchFilterRecord>();
    let _ = repository
        .put_user_search_filter_record(record.clone())
        .await
        .unwrap();

    let updated = OffsetDateTime::now_utc();
    let update = UserSearchFilterRecordUpdate {
        name: Some("my cool name".into()),
        notifications: None,
        state: None,
        enhanced_search_description: None,
        product_query: Some("boopel boop doop".try_into().unwrap()),
        shop_name_query: None,
        exclude_shop_name_query: None,
        seller_name_query: None,
        exclude_seller_name_query: None,
        shop_slug_id_query: None,
        exclude_shop_slug_id_query: None,
        seller_slug_id_query: None,
        exclude_seller_slug_id_query: None,
        shop_type_query: None,
        country_query: None,
        continent_query: None,
        geo_address_distance_query: None,
        price_query: None,
        state_query: None,
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        language: None,
        currency: None,
        updated_by: common::actor::record::ActorRecord::System,
        updated,
        last_hybrid_search_matched: None,
    };
    let _ = repository
        .update_user_search_filter_record(&record.user_id, &record.user_search_filter_id, update)
        .await
        .unwrap();

    let mut expected = record.clone();
    expected.name = "my cool name".into();
    expected.product_query = Some("boopel boop doop".try_into().unwrap());
    expected.updated = updated;

    let actual = repository
        .get_user_search_filter_record(&record.user_id, &record.user_search_filter_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected, actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_update_multiple_times() {
    let repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");

    let record = Faker.fake::<UserSearchFilterRecord>();
    let _ = repository
        .put_user_search_filter_record(record.clone())
        .await
        .unwrap();

    for _ in 0..100 {
        let _ = repository
            .update_user_search_filter_record(
                &record.user_id,
                &record.user_search_filter_id,
                Faker.fake(),
            )
            .await
            .unwrap();
    }

    let _ = repository
        .get_user_search_filter_record(&record.user_id, &record.user_search_filter_id)
        .await
        .unwrap()
        .unwrap();
}
