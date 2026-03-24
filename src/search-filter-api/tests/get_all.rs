use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product::core::product_search::ProductSearch;
use product::service::get_service::MockGetProductService;
use product_personalization::service::MockProductPersonalizationService;
use search_filter::core::user_search_filter_id::UserSearchFilterId;
use search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepositoryImpl;
use search_filter::service::user_search_filter_service::{
    UserSearchFilterService, UserSearchFilterServiceImpl,
};
use search_filter_api::handle;
use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_return_actual_search_filters_sortet_oldest_for_order_asc() {
    let repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = UserSearchFilterServiceImpl::new(&repository);
    let get_product_service = MockGetProductService::default();
    let personalization_service = MockProductPersonalizationService::default();

    let user_id = UserId::new();
    let mut expected = vec![];
    for search_filter in fake::vec![ProductSearch; 81] {
        let saved = service
            .save_user_search_filter(&user_id, Faker.fake(), search_filter)
            .await
            .unwrap();
        expected.push(saved);
    }
    expected.sort_by(|l, r| l.created.cmp(&r.created));
    let expected: Vec<UserSearchFilterId> = expected
        .into_iter()
        .map(|filter| filter.user_search_filter_id)
        .collect();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/search-filters")
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "asc")
            .jwt_claim("sub", user_id)
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);
    let json = extract_apigw_response_json_body!(response);

    assert_eq!(
        expected,
        json["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|filter| filter["userSearchFilterId"].as_str().unwrap())
            .map(UserSearchFilterId::try_from)
            .map(Result::unwrap)
            .collect::<Vec<UserSearchFilterId>>()
    );
    assert_eq!(0, json["from"]);
    assert_eq!(81, json["size"]);
    assert_eq!(81, json["total"]);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_actual_search_filters_sortet_latest_for_order_desc() {
    let repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = UserSearchFilterServiceImpl::new(&repository);
    let get_product_service = MockGetProductService::default();
    let personalization_service = MockProductPersonalizationService::default();

    let user_id = UserId::new();
    let mut expected = vec![];
    for search_filter in fake::vec![ProductSearch; 81] {
        let saved = service
            .save_user_search_filter(&user_id, Faker.fake(), search_filter)
            .await
            .unwrap();
        expected.push(saved);
    }
    expected.sort_by(|l, r| l.created.cmp(&r.created).reverse());
    let expected: Vec<UserSearchFilterId> = expected
        .into_iter()
        .map(|filter| filter.user_search_filter_id)
        .collect();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/search-filters")
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "desc")
            .jwt_claim("sub", user_id)
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);
    let json = extract_apigw_response_json_body!(response);

    assert_eq!(
        expected,
        json["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|filter| filter["userSearchFilterId"].as_str().unwrap())
            .map(UserSearchFilterId::try_from)
            .map(Result::unwrap)
            .collect::<Vec<UserSearchFilterId>>()
    );
    assert_eq!(0, json["from"]);
    assert_eq!(81, json["size"]);
    assert_eq!(81, json["total"]);
}
