use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use shop::{
    core::woocommerce_webhook_secret::WoocommerceWebhookSecret,
    data::patch_shop_data::PatchShopData,
    dynamodb::repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl},
    service::{
        command_service::{CommandShopService, CommandShopServiceImpl},
        get_service::GetShopServiceImpl,
        query_service::MockQueryShopService,
    },
};
use shop_api::handle;
use test_api::*;
use user::core::access_token::{HashedRawAccessToken, RawAccessToken};
use user::core::user::User;
use user::dynamodb::access_token_record::AccessTokenRecord;
use user::dynamodb::repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl};
use user::dynamodb::user_record::UserRecord;
use user::service::authenticator_service::AuthenticatorServiceImpl;
use user::service::user_service::UserServiceImpl;
use user::{
    core::access_token::AccessToken,
    service::{authenticator_service::MockAuthenticatorService, user_service::MockUserService},
};

fn system_ctx() -> common::actor::RequestContext {
    common::actor::RequestContext {
        actor: common::actor::domain::Actor::System,
    }
}

fn image_patch_data() -> PatchShopData {
    PatchShopData {
        shop_type: None,
        domains: None,
        shopify_domain: None,
        shopify_currency: None,
        shopify_language: None,
        woocommerce_webhook_secret: None,
        woocommerce_currency: None,
        woocommerce_language: None,
        url: None,
        image: Some(url::Url::parse("https://new-image.com/logo.png").unwrap()),
        structured_address: None,
        phone: None,
        email: None,
    }
}

fn no_authenticator() -> MockAuthenticatorService {
    let mut authenticator = MockAuthenticatorService::default();
    authenticator
        .expect_authenticate()
        .returning(|_| Box::pin(async { Ok(None) }));
    authenticator
}

async fn seed_partner_user_with_token(
    user_repo: &UserDynamoDbRepositoryImpl<'_>,
    shop_id: common::shop_id::ShopId,
) -> (common::user_id::UserId, RawAccessToken) {
    let user_id = common::user_id::UserId::new();

    let mut user: User = Faker.fake();
    user.user_id = user_id;
    user.partner_shops = std::iter::once(shop_id).collect();
    user_repo
        .put_user_record(UserRecord::from(user))
        .await
        .unwrap();

    let raw_token = RawAccessToken::new();
    let mut access_token: AccessToken = Faker.fake();
    access_token.user_id = user_id;
    access_token.hashed_token = HashedRawAccessToken::from(raw_token.clone());
    access_token.expires = None;
    user_repo
        .put_access_token_record(AccessTokenRecord::from(access_token))
        .await
        .unwrap();

    (user_id, raw_token)
}

#[aura_integration_test(services = [DynamoDB()])]
async fn should_200_respond_updated_shop_when_admin_patches_shop() {
    let repository = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_service = GetShopServiceImpl::new(&repository);
    let command_service = CommandShopServiceImpl::new(
        &repository,
        &shop::service::geocoding_service::NoopGeocodingService,
    );

    let admin_user_id = UserId::new();
    let shop = command_service
        .create(&system_ctx(), Faker.fake())
        .await
        .unwrap();

    let mut user_service = MockUserService::default();
    user_service
        .expect_check_admin()
        .return_once(move |_| Box::pin(async { Ok(()) }));

    let response = handle(
        LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/shops/{shopId}")
                .path_parameter("shopId", shop.shop_id.to_string())
                .jwt_claim("sub", admin_user_id)
                .body_serde(&image_patch_data())
                .build(),
            context: Default::default(),
        },
        &get_service,
        &MockQueryShopService::default(),
        &command_service,
        &user_service,
        &no_authenticator(),
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);
}

#[aura_integration_test(services = [DynamoDB()])]
async fn should_200_respond_updated_shop_when_partner_patches_shop() {
    let repository = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_service = GetShopServiceImpl::new(&repository);
    let command_service = CommandShopServiceImpl::new(
        &repository,
        &shop::service::geocoding_service::NoopGeocodingService,
    );

    let user_id = UserId::new();
    let shop = command_service
        .create(&system_ctx(), Faker.fake())
        .await
        .unwrap();

    let mut user_service = MockUserService::default();
    user_service.expect_check_admin().return_once(move |_| {
        Box::pin(async { Err(user::service::user_service::UserServiceError::AdminRoleRequired) })
    });
    user_service.expect_find_user().return_once(move |_| {
        let mut user: user::core::user::User = Faker.fake();
        user.user_id = user_id;
        user.partner_shops.insert(shop.shop_id);
        Box::pin(async move { Ok(user) })
    });

    let response = handle(
        LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/shops/{shopId}")
                .path_parameter("shopId", shop.shop_id.to_string())
                .jwt_claim("sub", user_id)
                .body_serde(&image_patch_data())
                .build(),
            context: Default::default(),
        },
        &get_service,
        &MockQueryShopService::default(),
        &command_service,
        &user_service,
        &no_authenticator(),
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);
}

#[aura_integration_test(services = [DynamoDB()])]
async fn should_200_respond_updated_shop_when_access_token_patches_shop() {
    let repository = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_service = GetShopServiceImpl::new(&repository);
    let command_service = CommandShopServiceImpl::new(
        &repository,
        &shop::service::geocoding_service::NoopGeocodingService,
    );

    let shop = command_service
        .create(&system_ctx(), Faker.fake())
        .await
        .unwrap();

    let user_repo = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let (_, raw_token) = seed_partner_user_with_token(&user_repo, shop.shop_id).await;
    let user_service_for_auth = UserServiceImpl::new(&user_repo);
    let user_service_for_handler = UserServiceImpl::new(&user_repo);
    let verifier = MockAccessTokenVerifierService::default();
    let authenticator = AuthenticatorServiceImpl::new(&verifier, &user_service_for_auth);

    let response = handle(
        LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::PATCH)
                .route_key("PATCH /api/v1/shops/{shopId}")
                .path_parameter("shopId", shop.shop_id.to_string())
                .header(
                    "Authorization",
                    format!("Bearer {}", String::from(raw_token)),
                )
                .body_serde(&serde_json::json!({
                    "woocommerceWebhookSecret": "integration-secret"
                }))
                .build(),
            context: Default::default(),
        },
        &get_service,
        &MockQueryShopService::default(),
        &command_service,
        &user_service_for_handler,
        &authenticator,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let record = repository
        .get_shop_record(&shop.shop_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        Some(WoocommerceWebhookSecret::from("integration-secret")),
        record.woocommerce_webhook_secret
    );
}
