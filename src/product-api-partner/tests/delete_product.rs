use common::actor::domain::Actor;
use common::product_lifecycle::record::ProductLifecycleRecord;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product::dynamodb::product_record::ProductRecord;
use product::dynamodb::repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl};
use product::service::command_service::CommandProductServiceImpl;
use shop::core::partner_shop::PartnerShop;
use shop::service::get_service::{MockGetShopService, VerifyPartnerShopError};
use std::collections::HashSet;
use test_api::*;
use time::OffsetDateTime;
use user::core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, Scope,
};
use user::core::{role::UserRole, tier::UserTier, user::User};
use user::service::authenticator_service::{AuthenticatedPrincipal, MockAuthenticatorService};
use user::service::user_service::{MockUserService, UserServiceError};

fn make_user(user_id: common::user_id::UserId, shop_id: Option<common::shop_id::ShopId>) -> User {
    User {
        user_id,
        email: "partner@example.com".try_into().unwrap(),
        first_name: None,
        last_name: None,
        language: None,
        currency: None,
        measurement_unit: None,
        prohibited_content_consent: false,
        tier: UserTier::Free,
        role: UserRole::User,
        stripe_customer_id: None,
        structured_address: None,
        geo_address: None,
        partner_shops: shop_id.into_iter().collect(),
        created_by: Actor::User(user_id),
        updated_by: Actor::User(user_id),
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    }
}

fn make_access_token(user_id: common::user_id::UserId) -> AccessToken {
    AccessToken {
        id: AccessTokenId::new(),
        hashed_token: user::core::access_token::RawAccessToken::new().into(),
        user_id,
        name: AccessTokenName::from("integration token"),
        scopes: HashSet::from([Scope::ProductsWrite]),
        origin: AccessTokenOrigin::User,
        expires: None,
        created_by: Actor::User(user_id),
        updated_by: Actor::User(user_id),
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
    shops_product_id: &common::shops_product_id::ShopsProductId,
    authorization: &str,
) -> LambdaEvent<aws_lambda_events::apigw::ApiGatewayV2httpRequest> {
    LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::DELETE)
            .route_key("DELETE /api/v1/shops/{shopId}/products/{shopsProductId}")
            .path_parameter("shopId", shop_id.to_string())
            .path_parameter("shopsProductId", shops_product_id.to_string())
            .header("Authorization", authorization)
            .build(),
        context: Default::default(),
    }
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_204_and_mark_product_deleted_when_product_exists() {
    let product_repository =
        ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let command_product_service = CommandProductServiceImpl::new_delete_only(&product_repository);
    let mut product_record: ProductRecord = Faker.fake();
    product_record.lifecycle = ProductLifecycleRecord::Active;
    product_repository
        .put_product_records([product_record.clone()].into())
        .await
        .unwrap();

    let mut partner_shop: PartnerShop = Faker.fake();
    partner_shop.shop_id = product_record.shop_id;
    let mut get_shop_service = MockGetShopService::default();
    get_shop_service
        .expect_find_partner_shop()
        .return_once(move |_| Box::pin(async move { Ok(partner_shop) }));

    let (user_service, authenticator_service) = authorized_services(product_record.shop_id);
    let response = product_api_partner::handle(
        make_event(
            product_record.shop_id,
            &product_record.shops_product_id,
            "Bearer token",
        ),
        &get_shop_service,
        &user_service,
        &authenticator_service,
        &product_lambda_ingest_partner_products::service::MockAsyncProductCommandService::default(),
        &command_product_service,
    )
    .await
    .unwrap();

    assert_eq!(204, response.status_code);
    let deleted = product_repository
        .get_product_record(&product_record.shop_id, &product_record.shops_product_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ProductLifecycleRecord::Deleted, deleted.lifecycle);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_404_when_product_does_not_exist_for_delete() {
    let product_repository =
        ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let command_product_service = CommandProductServiceImpl::new_delete_only(&product_repository);
    let shop_id = common::shop_id::ShopId::new();
    let shops_product_id =
        common::shops_product_id::ShopsProductId::from("missing-product".to_string());

    let mut partner_shop: PartnerShop = Faker.fake();
    partner_shop.shop_id = shop_id;
    let mut get_shop_service = MockGetShopService::default();
    get_shop_service
        .expect_find_partner_shop()
        .return_once(move |_| Box::pin(async move { Ok(partner_shop) }));

    let (user_service, authenticator_service) = authorized_services(shop_id);
    let response = product_api_partner::handle(
        make_event(shop_id, &shops_product_id, "Bearer token"),
        &get_shop_service,
        &user_service,
        &authenticator_service,
        &product_lambda_ingest_partner_products::service::MockAsyncProductCommandService::default(),
        &command_product_service,
    )
    .await
    .unwrap_err();

    assert_eq!(404, response.status);
}

#[tokio::test]
async fn should_return_401_when_access_token_is_invalid_for_delete() {
    let shop_id = common::shop_id::ShopId::new();
    let shops_product_id = common::shops_product_id::ShopsProductId::from("delete-me".to_string());
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
        make_event(shop_id, &shops_product_id, "Bearer invalid"),
        &get_shop_service,
        &user_service,
        &authenticator_service,
        &product_lambda_ingest_partner_products::service::MockAsyncProductCommandService::default(),
        &product::service::command_service::MockCommandProductService::default(),
    )
    .await
    .unwrap_err();

    assert_eq!(401, response.status);
}

#[tokio::test]
async fn should_return_404_when_shop_does_not_exist_for_delete() {
    let shop_id = common::shop_id::ShopId::new();
    let shops_product_id = common::shops_product_id::ShopsProductId::from("delete-me".to_string());
    let mut get_shop_service = MockGetShopService::default();
    get_shop_service
        .expect_find_partner_shop()
        .return_once(move |_| {
            Box::pin(async move { Err(VerifyPartnerShopError::ShopNotFound(shop_id)) })
        });

    let (user_service, authenticator_service) = authorized_services(shop_id);
    let response = product_api_partner::handle(
        make_event(shop_id, &shops_product_id, "Bearer token"),
        &get_shop_service,
        &user_service,
        &authenticator_service,
        &product_lambda_ingest_partner_products::service::MockAsyncProductCommandService::default(),
        &product::service::command_service::MockCommandProductService::default(),
    )
    .await
    .unwrap_err();

    assert_eq!(404, response.status);
}

#[tokio::test]
async fn should_return_403_when_user_is_not_associated_with_shop_for_delete() {
    let shop_id = common::shop_id::ShopId::new();
    let shops_product_id = common::shops_product_id::ShopsProductId::from("delete-me".to_string());
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
        make_event(shop_id, &shops_product_id, "Bearer token"),
        &get_shop_service,
        &user_service,
        &authenticator_service,
        &product_lambda_ingest_partner_products::service::MockAsyncProductCommandService::default(),
        &product::service::command_service::MockCommandProductService::default(),
    )
    .await
    .unwrap_err();

    assert_eq!(403, response.status);
}
