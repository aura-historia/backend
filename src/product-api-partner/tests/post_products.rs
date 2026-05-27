use common::has_key::HasKey;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product_lambda_ingest_partner_products::{
    AsyncProductCommandData, AsyncProductCommandServiceImpl,
};
use shop::{
    core::partner_shop::PartnerShop,
    service::get_service::{MockGetShopService, VerifyPartnerShopError},
};
use test_api::*;
use user::{
    core::access_token::{AccessToken, Scope},
    service::{
        authenticator_service::{AuthenticatedPrincipal, MockAuthenticatorService},
        user_service::{MockUserService, UserServiceError},
    },
};

const SQS: Sqs = Sqs {
    name: "product_api_partner_post_products",
};

fn authorized_services(
    shop_id: common::shop_id::ShopId,
) -> (MockUserService, MockAuthenticatorService) {
    let user_id = common::user_id::UserId::new();

    let mut user_service = MockUserService::default();
    user_service.expect_find_user().return_once(move |_| {
        let mut user: user::core::user::User = Faker.fake();
        user.user_id = user_id;
        user.partner_shops.insert(shop_id);
        Box::pin(async move { Ok(user) })
    });

    let mut access_token: AccessToken = Faker.fake();
    access_token.user_id = user_id;
    access_token.scopes = [Scope::ProductsWrite].into();

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
) -> LambdaEvent<aws_lambda_events::apigw::ApiGatewayV2httpRequest> {
    LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::POST)
            .route_key("POST /api/v1/shops/{shopId}/products")
            .path_parameter("shopId", shop_id.to_string())
            .header("Authorization", "******")
            .body_serde(&body)
            .build(),
        context: Default::default(),
    }
}

async fn receive_forwarded_command() -> AsyncProductCommandData {
    let messages = get_sqs_client()
        .await
        .receive_message()
        .queue_url(SQS.queue_url())
        .max_number_of_messages(1)
        .send()
        .await
        .unwrap()
        .messages
        .unwrap_or_default();
    let message = messages.first().expect("expected queued product command");
    serde_json::from_str(message.body.as_deref().unwrap()).unwrap()
}

#[localstack_test(services = [SQS])]
async fn should_return_202_and_forward_create_command_when_products_created_successfully() {
    let shop_id = common::shop_id::ShopId::new();
    let async_product_command_service =
        AsyncProductCommandServiceImpl::new(get_sqs_client().await, SQS.queue_url());

    let mut partner_shop: PartnerShop = Faker.fake();
    partner_shop.shop_id = shop_id;
    let mut get_shop_service = MockGetShopService::default();
    get_shop_service
        .expect_find_partner_shop()
        .return_once(move |_| Box::pin(async move { Ok(partner_shop) }));

    let (user_service, authenticator_service) = authorized_services(shop_id);
    let response = product_api_partner::handle(
        make_event(
            shop_id,
            serde_json::json!([{
                "shopsProductId": "integration-product-1",
                "title": { "text": "Test Product", "language": "en" },
                "description": { "text": "A test product", "language": "en" },
                "state": "AVAILABLE",
                "url": "https://example.com/product/1",
                "images": ["https://example.com/img.jpg"]
            }]),
        ),
        &get_shop_service,
        &user_service,
        &authenticator_service,
        &async_product_command_service,
    )
    .await
    .unwrap();
    assert_eq!(202, response.status_code);

    let command = receive_forwarded_command().await;
    assert_eq!(command.key().shop_id, shop_id);
    assert_eq!(
        command.key().shops_product_id.to_string(),
        "integration-product-1"
    );
    assert!(matches!(command, AsyncProductCommandData::Create(_)));
}

#[tokio::test]
async fn should_return_401_when_access_token_is_invalid() {
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
        make_event(shop_id, serde_json::json!([])),
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
async fn should_return_404_when_shop_does_not_exist() {
    let shop_id = common::shop_id::ShopId::new();
    let mut get_shop_service = MockGetShopService::default();
    get_shop_service
        .expect_find_partner_shop()
        .return_once(move |_| {
            Box::pin(async move { Err(VerifyPartnerShopError::ShopNotFound(shop_id)) })
        });

    let (user_service, authenticator_service) = authorized_services(shop_id);
    let response = product_api_partner::handle(
        make_event(shop_id, serde_json::json!([])),
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
async fn should_return_403_when_user_is_not_associated_with_shop() {
    let shop_id = common::shop_id::ShopId::new();

    let mut get_shop_service = MockGetShopService::default();
    get_shop_service.expect_find_partner_shop().never();

    let user_id = common::user_id::UserId::new();
    let mut user_service = MockUserService::default();
    user_service.expect_find_user().return_once(move |_| {
        let mut user: user::core::user::User = Faker.fake();
        user.user_id = user_id;
        Box::pin(async move { Ok(user) })
    });

    let mut access_token: AccessToken = Faker.fake();
    access_token.user_id = user_id;
    access_token.scopes = [Scope::ProductsWrite].into();
    let mut authenticator_service = MockAuthenticatorService::default();
    authenticator_service
        .expect_authenticate()
        .return_once(move |_| {
            Box::pin(async move { Ok(Some(AuthenticatedPrincipal::AccessToken(access_token))) })
        });

    let response = product_api_partner::handle(
        make_event(shop_id, serde_json::json!([])),
        &get_shop_service,
        &user_service,
        &authenticator_service,
        &product_lambda_ingest_partner_products::service::MockAsyncProductCommandService::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(403, response.status);
}
