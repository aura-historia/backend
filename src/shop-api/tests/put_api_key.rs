use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use shop::{
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

#[localstack_test(services = [DynamoDB()])]
async fn should_200_respond_api_key_when_partner_creates_api_key() {
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
        &MockAccessTokenVerifierService::default(),
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

#[localstack_test(services = [DynamoDB()])]
async fn should_200_respond_api_key_when_admin_creates_api_key() {
    let repository = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let get_service = GetShopServiceImpl::new(&repository);
    let command_service = CommandShopServiceImpl::new(
        &repository,
        &shop::service::geocoding_service::NoopGeocodingService,
    );

    let admin_user_id = UserId::new();
    let partner_user_id = UserId::new();

    let create_cmd = Faker.fake();
    let shop = command_service.create(create_cmd).await.unwrap();

    // Make the shop a partner shop by updating the existing record in-place
    repository
        .update_shop_record(
            &shop.shop_id,
            ShopRecordUpdate {
                partner_user_id: Some(partner_user_id),
                gsi1_pk: Some(shop::dynamodb::shop_record::mk_gsi1_pk(&partner_user_id)),
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
    user_service
        .expect_check_admin()
        .return_once(move |_| Box::pin(async { Ok(()) }));

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PUT)
            .route_key("PUT /api/v1/shops/{shopId}/api-key")
            .path_parameter("shopId", shop.shop_id.to_string())
            .jwt_claim("sub", admin_user_id)
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
    assert_eq!(200, response.status_code);

    let body: serde_json::Value = match response.body {
        Some(aws_lambda_events::encodings::Body::Text(body_str)) => {
            serde_json::from_str(&body_str).unwrap()
        }
        _ => panic!("Expected response body to be Text"),
    };
    assert!(body["apiKey"].is_string());
}
