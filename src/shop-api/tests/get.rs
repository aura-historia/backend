use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use shop::{
    data::get_shop_data::GetShopData,
    dynamodb::repository::ShopDynamoDbRepositoryImpl,
    service::{
        command_service::{CommandShopService, CommandShopServiceImpl, MockCommandShopService},
        get_service::GetShopServiceImpl,
        query_service::MockQueryShopService,
    },
};
use shop_api::handle;
use test_api::*;

#[localstack_test(services = [DynamoDB()])]
async fn should_200_respond_shop_for_id() {
    let repository = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_service = GetShopServiceImpl::new(&repository);
    let command_service = CommandShopServiceImpl::new(&repository);

    let create_cmd = Faker.fake();
    let expected = command_service.create(create_cmd).await.unwrap();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/shops/{shopId}")
            .path_parameter("shopId", expected.shop_id)
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &get_service,
        &MockQueryShopService::default(),
        &MockCommandShopService::default(),
    )
    .await
    .unwrap();
    let actual = serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(200, response.status_code);
    assert_eq!(GetShopData::from(expected), actual)
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_respond_shop_for_domain() {
    let repository = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_service = GetShopServiceImpl::new(&repository);
    let command_service = CommandShopServiceImpl::new(&repository);

    let create_cmd = Faker.fake();
    let expected = command_service.create(create_cmd).await.unwrap();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/by-domain/shops/{shopDomain}")
            .path_parameter(
                "shopDomain",
                expected.domains.iter().next().unwrap().to_string(),
            )
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &get_service,
        &MockQueryShopService::default(),
        &MockCommandShopService::default(),
    )
    .await
    .unwrap();
    let actual = serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(200, response.status_code);
    assert_eq!(GetShopData::from(expected), actual)
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_respond_shop_for_slug() {
    let repository = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_service = GetShopServiceImpl::new(&repository);
    let command_service = CommandShopServiceImpl::new(&repository);

    let create_cmd = Faker.fake();
    let expected = command_service.create(create_cmd).await.unwrap();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/by-slug/shops/{shopSlugId}")
            .path_parameter("shopSlugId", expected.shop_slug_id.clone())
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &get_service,
        &MockQueryShopService::default(),
        &MockCommandShopService::default(),
    )
    .await
    .unwrap();
    let actual = serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(200, response.status_code);
    assert_eq!(GetShopData::from(expected), actual)
}
