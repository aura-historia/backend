use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use shop::{
    data::post_shop_data::PostShopData, dynamodb::repository::ShopDynamoDbRepositoryImpl,
    service::command_service::CommandShopServiceImpl, service::get_service::GetShopServiceImpl,
    service::query_service::MockQueryShopService,
};
use shop_api::handle;
use test_api::*;
use user::service::user_service::MockUserService;

#[localstack_test(services = [DynamoDB()])]
async fn should_201_respond_created_shop_when_admin_posts_shop() {
    let repository = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_service = GetShopServiceImpl::new(&repository);
    let command_service = CommandShopServiceImpl::new(
        &repository,
        &shop::service::geocoding_service::NoopGeocodingService,
    );

    let admin_user_id = UserId::new();

    let mut user_service = MockUserService::default();
    user_service
        .expect_check_admin()
        .return_once(move |_| Box::pin(async { Ok(()) }));

    let post_data: PostShopData = Faker.fake();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .route_key("POST /api/v1/shops")
            .jwt_claim("sub", admin_user_id)
            .body_serde(&post_data)
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &get_service,
        &MockQueryShopService::default(),
        &command_service,
        &user_service,
        &MockAccessTokenVerifierService::default(),
    )
    .await
    .unwrap();
    assert_eq!(201, response.status_code);
}
