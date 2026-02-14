use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use test_api::*;
use user::{
    data::get_user_data::GetUserAccountData,
    dynamodb::repository::UserDynamoDbRepositoryImpl,
    service::user_service::{UserService, UserServiceImpl},
};
use user_api::handler;

#[localstack_test(services = [DynamoDB()])]
async fn should_200_respond_user_when_exists() {
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = UserServiceImpl::new(&repository);

    let user = service.create_user(Faker.fake()).await.unwrap();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/account")
            .jwt_claim("sub", user.user_id)
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &service).await.unwrap();
    let actual: GetUserAccountData =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();

    assert_eq!(GetUserAccountData::from(user), actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_include_no_store_cache_control_header() {
    let repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let service = UserServiceImpl::new(&repository);

    let user = service.create_user(Faker.fake()).await.unwrap();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/me/account")
            .jwt_claim("sub", user.user_id)
            .build(),
        context: Default::default(),
    };
    let response = handler(lambda_event, &service).await.unwrap();

    assert_eq!(200, response.status_code);
    let cache_control = response
        .headers
        .get(http::header::CACHE_CONTROL)
        .expect("Cache-Control header should be present")
        .to_str()
        .unwrap();
    assert_eq!("no-store", cache_control);
}
