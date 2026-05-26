use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use shop::{
    core::woocommerce_webhook_secret::WoocommerceWebhookSecret,
    data::patch_shop_data::PatchShopData,
    dynamodb::repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl},
    dynamodb::shop_record_update::ShopRecordUpdate,
    service::{
        command_service::{CommandShopService, CommandShopServiceImpl},
        get_service::GetShopServiceImpl,
        query_service::MockQueryShopService,
    },
};
use shop_api::handle;
use test_api::*;
use time::OffsetDateTime;
use user::service::user_service::MockUserService;

fn no_access_token_verifier() -> MockAccessTokenVerifierService {
    let mut verifier = MockAccessTokenVerifierService::default();
    verifier
        .expect_verify_extract_user_id()
        .returning(|_| Box::pin(async { Ok(None) }));
    verifier
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_respond_updated_shop_when_admin_patches_shop() {
    let repository = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_service = GetShopServiceImpl::new(&repository);
    let command_service = CommandShopServiceImpl::new(
        &repository,
        &shop::service::geocoding_service::NoopGeocodingService,
    );

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
        &no_access_token_verifier(),
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_respond_updated_shop_when_partner_patches_shop() {
    let repository = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_service = GetShopServiceImpl::new(&repository);
    let command_service = CommandShopServiceImpl::new(
        &repository,
        &shop::service::geocoding_service::NoopGeocodingService,
    );

    let user_id = UserId::new();

    let create_cmd = Faker.fake();
    let shop = command_service.create(create_cmd).await.unwrap();

    // Make the shop a partner shop by updating the existing record in-place
    repository
        .update_shop_record(
            &shop.shop_id,
            ShopRecordUpdate {
                partner_user_id: Some(user_id),
                gsi1_pk: Some(shop::dynamodb::shop_record::mk_gsi1_pk(&user_id)),
                gsi1_sk: Some(shop::dynamodb::shop_record::mk_gsi1_sk(&shop.shop_id)),
                gsi3_pk: None,
                gsi3_sk: None,
                shop_type: None,
                domains: None,
                shopify_domain: None,
                shopify_currency: None,
                shopify_language: None,
                woocommerce_webhook_secret: None,
                woocommerce_currency: None,
                woocommerce_language: None,
                url: None,
                view_url: None,
                image: None,
                structured_address_addressline: None,
                structured_address_addressline_extra: None,
                structured_address_locality: None,
                structured_address_region: None,
                structured_address_postal_code: None,
                structured_address_country: None,
                geo_address_lat: None,
                geo_address_lon: None,
                phone: None,
                email: None,
                partner_api_key_short: None,
                partner_api_key_long_hash: None,
                updated: OffsetDateTime::now_utc(),
            },
        )
        .await
        .unwrap();

    let mut user_service = MockUserService::default();
    user_service.expect_check_admin().return_once(move |_| {
        Box::pin(async { Err(user::service::user_service::UserServiceError::AdminRoleRequired) })
    });

    let patch_data = PatchShopData {
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
    };

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .route_key("PATCH /api/v1/shops/{shopId}")
            .path_parameter("shopId", shop.shop_id.to_string())
            .jwt_claim("sub", user_id)
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
        &no_access_token_verifier(),
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_respond_updated_shop_when_api_key_patches_shop() {
    let repository = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_service = GetShopServiceImpl::new(&repository);
    let command_service = CommandShopServiceImpl::new(
        &repository,
        &shop::service::geocoding_service::NoopGeocodingService,
    );

    let user_id = UserId::new();
    let shop = command_service.create(Faker.fake()).await.unwrap();
    repository
        .update_shop_record(
            &shop.shop_id,
            ShopRecordUpdate {
                partner_user_id: Some(user_id),
                gsi1_pk: Some(shop::dynamodb::shop_record::mk_gsi1_pk(&user_id)),
                gsi1_sk: Some(shop::dynamodb::shop_record::mk_gsi1_sk(&shop.shop_id)),
                gsi3_pk: None,
                gsi3_sk: None,
                shop_type: None,
                domains: None,
                shopify_domain: None,
                shopify_currency: None,
                shopify_language: None,
                woocommerce_webhook_secret: None,
                woocommerce_currency: None,
                woocommerce_language: None,
                url: None,
                view_url: None,
                image: None,
                structured_address_addressline: None,
                structured_address_addressline_extra: None,
                structured_address_locality: None,
                structured_address_region: None,
                structured_address_postal_code: None,
                structured_address_country: None,
                geo_address_lat: None,
                geo_address_lon: None,
                phone: None,
                email: None,
                partner_api_key_short: None,
                partner_api_key_long_hash: None,
                updated: OffsetDateTime::now_utc(),
            },
        )
        .await
        .unwrap();
    let api_key = command_service
        .create_api_key(&user_id, &shop.shop_id)
        .await
        .unwrap();
    let api_key: String = api_key.into();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .route_key("PATCH /api/v1/shops/{shopId}")
            .path_parameter("shopId", shop.shop_id.to_string())
            .header("x-aura-historia-access-token", api_key)
            .body_serde(&serde_json::json!({
                "woocommerceWebhookSecret": "integration-secret"
            }))
            .build(),
        context: Default::default(),
    };

    let response = handle(
        lambda_event,
        &get_service,
        &MockQueryShopService::default(),
        &command_service,
        &MockUserService::default(),
        &no_access_token_verifier(),
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
