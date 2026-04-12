use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use shop::{
    data::patch_shop_data::PatchShopData,
    dynamodb::repository::ShopDynamoDbRepositoryImpl,
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
async fn should_200_respond_updated_shop_when_admin_patches_shop() {
    let repository = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_service = GetShopServiceImpl::new(&repository);
    let command_service = CommandShopServiceImpl::new(&repository);

    let admin_user_id = UserId::new();

    let create_cmd = Faker.fake();
    let shop = command_service.create(create_cmd).await.unwrap();

    let mut user_service = MockUserService::default();
    user_service
        .expect_check_admin()
        .return_once(move |_| Box::pin(async { Ok(()) }));

    let patch_data = PatchShopData {
        shop_type: None,
        domains: None,
        image: Some(url::Url::parse("https://new-image.com/logo.png").unwrap()),
    };

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .route_key("PATCH /api/v1/shops/{shopId}")
            .path_parameter("shopId", shop.shop_id.to_string())
            .jwt_claim("sub", admin_user_id)
            .body_serde(&patch_data)
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
}
