use common::query::range_query::RangeQuery;
use common::user_id::UserId;
use fake::{Fake, Faker};
use product::core::item_search::ItemSearch;
use lambda_runtime::LambdaEvent;
use search_filter::data::user_search_filter_data::UserSearchFilterData;
use search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepositoryImpl;
use search_filter::service::user_search_filter_service::{
    UserSearchFilterService, UserSearchFilterServiceImpl,
};
use search_filter_api_patch_search_filter::{
    handler,
    patch::{PatchItemSearchData, PatchUserSearchFilterData},
};
use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_update_search_filter() {
    let repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = UserSearchFilterServiceImpl::new(&repository);

    let user_id = UserId::new();
    let initial = service
        .save_user_search_filter(&user_id, Faker.fake(), Faker.fake::<ItemSearch>())
        .await
        .unwrap();

    let patch = PatchUserSearchFilterData {
        name: Some("thorbens filter".into()),
        search: Some(PatchItemSearchData {
            language: None,
            currency: None,
            item_query: None,
            shop_name_query: Some("Whoop boop woah".try_into().unwrap()),
            price_query: Some(RangeQuery {
                min: Some(37),
                max: Some(42),
            }),
            state_query: None,
            created_query: None,
            updated_query: None,
        }),
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .jwt_claim("sub", user_id)
            .path_parameter("userSearchFilterId", initial.user_search_filter_id)
            .body_serde(&patch)
            .build(),
        context: Default::default(),
    };

    let response = handler(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let actual: UserSearchFilterData = serde_json::from_value(json).unwrap();
    assert_eq!(patch.name.unwrap(), actual.name);
    assert_eq!(
        patch.search.clone().unwrap().shop_name_query.unwrap(),
        actual.search.shop_name_query.unwrap()
    );
    assert_eq!(
        patch.search.clone().unwrap().price_query.unwrap(),
        actual.search.price_query.unwrap()
    );
}
