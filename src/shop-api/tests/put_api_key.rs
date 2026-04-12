use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use shop::{
    dynamodb::repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl},
    dynamodb::shop_record::ShopRecord,
    service::{
        command_service::{CommandShopService, CommandShopServiceImpl},
        get_service::GetShopServiceImpl,
        query_service::MockQueryShopService,
    },
};
use shop_api::handle;
use test_api::*;
use user::service::user_service::MockUserService;

#[localstack_test(services = [DynamoDB()])]
async fn should_200_respond_api_key_when_partner_creates_api_key() {
    let repository = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_service = GetShopServiceImpl::new(&repository);
    let command_service = CommandShopServiceImpl::new(&repository);

    let user_id = UserId::new();

    let create_cmd = Faker.fake();
    let shop = command_service.create(create_cmd).await.unwrap();

    // Make the shop a partner shop by updating the record directly
    let mut record: ShopRecord = Faker.fake();
    record.shop_id = shop.shop_id;
    record.shop_slug_id = shop.shop_slug_id.clone();
    record.name = shop.name.clone();
    record.partner_user_id = Some(user_id);
    record.gsi1_pk = Some(shop::dynamodb::shop_record::mk_gsi1_pk(&user_id));
    record.gsi1_sk = Some(shop::dynamodb::shop_record::mk_gsi1_sk(&shop.shop_id));
    repository.put_shop_record(record).await.unwrap();

    let mut user_service = MockUserService::default();
    user_service.expect_check_admin().return_once(move |_| {
        Box::pin(async { Err(user::service::user_service::UserServiceError::AdminRoleRequired) })
    });

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PUT)
            .route_key("PUT /api/v1/shops/{shopId}/api-key")
            .path_parameter("shopId", shop.shop_id.to_string())
            .jwt_claim("sub", user_id)
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &get_service,
        &MockQueryShopService::default(),
        &command_service,
        &user_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let body: serde_json::Value = match response.body {
        Some(aws_lambda_events::encodings::Body::Text(body_str)) => {
            serde_json::from_str(&body_str).unwrap()
        }
        _ => panic!("Expected response body to be Text"),
    };
    assert!(body["apiKey"].is_string());
}
