use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product_lambda_ingest_partner_products::AsyncProductCommandServiceImpl;
use shop::{
    core::partner_shop::PartnerShop,
    service::get_service::{MockGetShopService, VerifyPartnerShopError},
};
use std::collections::HashSet;
use test_api::*;
use time::OffsetDateTime;
use user::{
    core::{
        access_token::{
            AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, HashedRawAccessToken,
            RawAccessToken, Scope,
        },
        role::UserRole,
        tier::UserTier,
        user::User,
    },
    service::{
        authenticator_service::{AuthenticatedPrincipal, MockAuthenticatorService},
        user_service::{MockUserService, UserServiceError},
    },
};

use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
use user::dynamodb::access_token_record::AccessTokenRecord;
use user::dynamodb::repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl};
use user::dynamodb::user_record::UserRecord;
use user::service::authenticator_service::AuthenticatorServiceImpl;
use user::service::user_service::UserServiceImpl;

const SQS: Sqs = Sqs {
    name: "product_api_partner_put_products",
};

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

fn make_user(user_id: common::user_id::UserId, shop_id: Option<common::shop_id::ShopId>) -> User {
    User {
        user_id,
        email: "partner@example.com".try_into().unwrap(),
        first_name: None,
        last_name: None,
        language: None,
        currency: None,
        prohibited_content_consent: false,
        tier: UserTier::Free,
        role: UserRole::User,
        stripe_customer_id: None,
        structured_address: None,
        geo_address: None,
        partner_shops: shop_id.into_iter().collect(),
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    }
}

fn make_access_token(user_id: common::user_id::UserId) -> AccessToken {
    let raw = RawAccessToken::new();
    AccessToken {
        id: AccessTokenId::new(),
        hashed_token: raw.into(),
        user_id,
        name: AccessTokenName::from("integration token"),
        scopes: HashSet::from([Scope::ProductsWrite]),
        origin: AccessTokenOrigin::User,
        expires: None,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    }
}

fn authorized_services(
    shop_id: common::shop_id::ShopId,
) -> (MockUserService, MockAuthenticatorService) {
    let user_id = common::user_id::UserId::new();

    let mut user_service = MockUserService::default();
    user_service
        .expect_find_user()
        .return_once(move |_| Box::pin(async move { Ok(make_user(user_id, Some(shop_id))) }));

    let access_token = make_access_token(user_id);

    let mut authenticator_service = MockAuthenticatorService::default();
    authenticator_service
        .expect_authenticate()
        .return_once(move |_| {
            Box::pin(async move { Ok(Some(AuthenticatedPrincipal::AccessToken(access_token))) })
        });

    (user_service, authenticator_service)
}

fn make_event(
    shop_id: common::shop_id::ShopId,
    body: serde_json::Value,
    authorization: &str,
) -> LambdaEvent<aws_lambda_events::apigw::ApiGatewayV2httpRequest> {
    LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PUT)
            .route_key("PUT /api/v1/shops/{shopId}/products")
            .path_parameter("shopId", shop_id.to_string())
            .header("Authorization", authorization)
            .body_serde(&body)
            .build(),
        context: Default::default(),
    }
}

#[localstack_test(services = [DynamoDB(), SQS])]
async fn should_return_202_when_upserting_product() {
    let shop_id = common::shop_id::ShopId::new();
    let command_product_service =
        AsyncProductCommandServiceImpl::new(get_sqs_client().await, SQS.queue_url());

    let mut partner_shop: PartnerShop = Faker.fake();
    partner_shop.shop_id = shop_id;
    let mut get_shop_service = MockGetShopService::default();
    get_shop_service
        .expect_find_partner_shop()
        .return_once(move |_| Box::pin(async move { Ok(partner_shop) }));

    let user_repo = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let (_, raw_token) = seed_partner_user_with_token(&user_repo, shop_id).await;
    let user_service_for_auth = UserServiceImpl::new(&user_repo);
    let user_service_for_handler = UserServiceImpl::new(&user_repo);
    let verifier = MockAccessTokenVerifierService::default();
    let authenticator_service = AuthenticatorServiceImpl::new(&verifier, &user_service_for_auth);
    let response = product_api_partner::handle(
        make_event(
            shop_id,
            serde_json::json!([{
                "shopsProductId": "integration-put-product-1",
                "state": "AVAILABLE"
            }]),
            &format!("Bearer {}", String::from(raw_token)),
        ),
        &get_shop_service,
        &user_service_for_handler,
        &authenticator_service,
        &command_product_service,
    )
    .await
    .unwrap();
    assert_eq!(202, response.status_code);
}

#[tokio::test]
async fn should_return_401_when_access_token_is_invalid_for_put() {
    let shop_id = common::shop_id::ShopId::new();
    let mut get_shop_service = MockGetShopService::default();
    get_shop_service.expect_find_partner_shop().never();

    let user_service = MockUserService::default();
    let mut authenticator_service = MockAuthenticatorService::default();
    authenticator_service
        .expect_authenticate()
        .return_once(|_| {
            Box::pin(async move { Err(UserServiceError::AccessTokenNotFoundByRaw.into()) })
        });

    let response = product_api_partner::handle(
        make_event(shop_id, serde_json::json!([]), "Bearer invalid"),
        &get_shop_service,
        &user_service,
        &authenticator_service,
        &product_lambda_ingest_partner_products::service::MockAsyncProductCommandService::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(401, response.status);
}

#[tokio::test]
async fn should_return_404_when_shop_does_not_exist_for_put() {
    let shop_id = common::shop_id::ShopId::new();
    let mut get_shop_service = MockGetShopService::default();
    get_shop_service
        .expect_find_partner_shop()
        .return_once(move |_| {
            Box::pin(async move { Err(VerifyPartnerShopError::ShopNotFound(shop_id)) })
        });

    let (user_service, authenticator_service) = authorized_services(shop_id);
    let response = product_api_partner::handle(
        make_event(shop_id, serde_json::json!([]), "Bearer invalid"),
        &get_shop_service,
        &user_service,
        &authenticator_service,
        &product_lambda_ingest_partner_products::service::MockAsyncProductCommandService::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(404, response.status);
}

#[tokio::test]
async fn should_return_403_when_user_is_not_associated_with_shop_for_put() {
    let shop_id = common::shop_id::ShopId::new();

    let mut get_shop_service = MockGetShopService::default();
    get_shop_service.expect_find_partner_shop().never();

    let user_id = common::user_id::UserId::new();
    let mut user_service = MockUserService::default();
    user_service
        .expect_find_user()
        .return_once(move |_| Box::pin(async move { Ok(make_user(user_id, None)) }));

    let access_token = make_access_token(user_id);
    let mut authenticator_service = MockAuthenticatorService::default();
    authenticator_service
        .expect_authenticate()
        .return_once(move |_| {
            Box::pin(async move { Ok(Some(AuthenticatedPrincipal::AccessToken(access_token))) })
        });

    let response = product_api_partner::handle(
        make_event(shop_id, serde_json::json!([]), "Bearer invalid"),
        &get_shop_service,
        &user_service,
        &authenticator_service,
        &product_lambda_ingest_partner_products::service::MockAsyncProductCommandService::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(403, response.status);
}
