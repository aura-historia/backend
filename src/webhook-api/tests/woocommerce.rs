use base64::Engine;
use common::has_key::HasKey;
use fake::{Fake, Faker};
use http::HeaderMap;
use lambda_runtime::{Context, LambdaEvent};
use openssl::hash::MessageDigest;
use openssl::pkey::PKey;
use openssl::sign::Signer;
use product_lambda_ingest_partner_products::{
    AsyncProductCommandData, AsyncProductCommandServiceImpl,
};
use shop::core::woocommerce_webhook_secret::WoocommerceWebhookSecret;
use shop::dynamodb::partner_status_record::ShopPartnerStatusRecord;
use shop::dynamodb::repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl};
use shop::dynamodb::shop_record::ShopRecord;
use shop::service::get_service::GetShopServiceImpl;
use test_api::*;
use user::service::authenticator_service::{AuthenticatedPrincipal, MockAuthenticatorService};
use user::service::user_service::MockUserService;
use webhook_api::woocommerce::handler::{WOOCOMMERCE_TOPIC_PRODUCT_CREATED, handle_woocommerce};

const SQS: Sqs = Sqs {
    name: "webhook_api_woocommerce",
};
const SECRET: &str = "woocommerce-secret";

fn signature(body: &str) -> String {
    let key = PKey::hmac(SECRET.as_bytes()).unwrap();
    let mut signer = Signer::new(MessageDigest::sha256(), &key).unwrap();
    signer.update(body.as_bytes()).unwrap();
    base64::engine::general_purpose::STANDARD.encode(signer.sign_to_vec().unwrap())
}

fn make_partner_shop_record() -> ShopRecord {
    let mut record: ShopRecord = Faker.fake();
    record.shop_partner_status = ShopPartnerStatusRecord::Partnered;
    record.woocommerce_webhook_secret = Some(WoocommerceWebhookSecret::from(SECRET));
    record.woocommerce_currency = Some(common::currency::record::CurrencyRecord::Eur);
    record.woocommerce_language = Some(common::language::record::LanguageRecord::En);
    record
}

fn authorized_services(
    shop_id: common::shop_id::ShopId,
) -> (MockAuthenticatorService, MockUserService) {
    let user_id = common::user_id::UserId::new();

    let mut access_token: user::core::access_token::AccessToken = Faker.fake();
    access_token.user_id = user_id;

    let mut authenticator_service = MockAuthenticatorService::default();
    authenticator_service
        .expect_authenticate()
        .return_once(move |_| {
            Box::pin(async move { Ok(Some(AuthenticatedPrincipal::AccessToken(access_token))) })
        });

    let mut user_service = MockUserService::default();
    user_service.expect_find_user().return_once(move |_| {
        let mut user: user::core::user::User = Faker.fake();
        user.user_id = user_id;
        user.partner_shops.insert(shop_id);
        Box::pin(async move { Ok(user) })
    });

    (authenticator_service, user_service)
}

#[localstack_test(services = [DynamoDB(), SQS])]
async fn should_return_202_and_forward_woocommerce_create_webhook_to_sqs() {
    let ddb_client = get_dynamodb_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let async_product_command_service =
        AsyncProductCommandServiceImpl::new(get_sqs_client().await, SQS.queue_url());

    let shop_record = make_partner_shop_record();
    let shop_id = shop_record.shop_id;
    shop_repository.put_shop_record(shop_record).await.unwrap();

    let (authenticator_service, user_service) = authorized_services(shop_id);

    let body_json = serde_json::json!({
        "id": 17,
        "name": "Woo Product",
        "permalink": "https://example.com/product/woo-product/",
        "description": "<p>Woo description</p>",
        "price": "42.00",
        "status": "publish",
        "stock_status": "instock",
        "images": []
    });
    let body = body_json.to_string();

    let mut request = ApiGatewayV2httpRequestProxy::builder()
        .http_method(http::Method::POST)
        .route_key("POST /api/v1/webhooks/woocommerce/{shopId}")
        .path_parameter("shopId", shop_id.to_string())
        .body_serde(&body_json)
        .build();
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-wc-webhook-topic",
        WOOCOMMERCE_TOPIC_PRODUCT_CREATED.parse().unwrap(),
    );
    headers.insert("x-wc-webhook-signature", signature(&body).parse().unwrap());
    request.headers = headers;

    let response = handle_woocommerce(
        LambdaEvent::new(request, Context::default()),
        &get_shop_service,
        &user_service,
        &authenticator_service,
        &async_product_command_service,
    )
    .await
    .unwrap();

    assert_eq!(202, response.status_code);
    assert!(response.body.is_none());

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
    let command: AsyncProductCommandData =
        serde_json::from_str(messages[0].body.as_deref().unwrap()).unwrap();
    assert!(matches!(command, AsyncProductCommandData::Upsert(_)));
    assert_eq!(command.key().shop_id, shop_id);
    assert_eq!(command.key().shops_product_id.to_string(), "17");
}
