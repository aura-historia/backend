use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product::service::get_service::MockGetProductService;
use product_personalization::service::MockProductPersonalizationService;
use search_filter::data::user_search_filter_data::UserSearchFilterData;
use search_filter::dynamodb::repository::{
    UserSearchFilterDynamoDbRepository, UserSearchFilterDynamoDbRepositoryImpl,
};
use search_filter::service::user_search_filter_service::UserSearchFilterServiceImpl;
use search_filter_api::handle;
use search_filter_api::post_types::PostUserSearchFilterData;
use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_save_search_filter() {
    let user_id = UserId::new();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .route_key("POST /api/v1/me/search-filters")
            .body_serde(&Faker.fake::<PostUserSearchFilterData>())
            .jwt_claim("sub", user_id)
            .build(),
        context: Default::default(),
    };

    let repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = UserSearchFilterServiceImpl::new(&repository);
    let get_product_service = MockGetProductService::default();
    let personalization_service = MockProductPersonalizationService::default();
    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(201, response.status_code);

    let json = extract_apigw_response_json_body!(response);
    let actual: UserSearchFilterData = serde_json::from_value(json).unwrap();
    assert_eq!(user_id, actual.user_id);

    let record = repository
        .get_user_search_filter_record(&user_id, &actual.user_search_filter_id)
        .await
        .unwrap();
    assert!(record.is_some());
}
