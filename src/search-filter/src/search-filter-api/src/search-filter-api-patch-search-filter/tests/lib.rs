use common::query::range_query::RangeQuery;
use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use search_filter_api_patch_search_filter::{
    handler, search_filter_data_patch::SearchFilterDataPatch,
};
use search_filter_core::search_filter::SearchFilter;
use search_filter_data::user_search_filter_data::UserSearchFilterData;
use search_filter_dynamodb::repository::SearchFilterDynamoDbRepositoryImpl;
use search_filter_service::service::{SearchFilterService, SearchFilterServiceImpl};
use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_update_search_filter() {
    let repository =
        SearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = SearchFilterServiceImpl::new(&repository);

    let user_id = UserId::new();
    let initial = service
        .save_search_filter(&user_id, Faker.fake::<SearchFilter>())
        .await
        .unwrap();

    let patch = SearchFilterDataPatch {
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
    };
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .jwt_claim("sub", user_id)
            .path_parameter("searchFilterId", initial.search_filter_id)
            .body_serde(&patch)
            .build(),
        context: Default::default(),
    };

    let response = handler(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let actual: UserSearchFilterData = serde_json::from_value(json).unwrap();
    assert_eq!(
        patch.shop_name_query.unwrap(),
        actual.search_filter.shop_name_query.unwrap()
    );
    assert_eq!(
        patch.price_query.unwrap(),
        actual.search_filter.price_query.unwrap()
    );
}
