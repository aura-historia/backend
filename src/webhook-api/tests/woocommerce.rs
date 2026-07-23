use base64::Engine;
use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
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
use user::core::access_token::{AccessToken, HashedRawAccessToken, RawAccessToken};
use user::core::user::User;
use user::dynamodb::access_token_record::AccessTokenRecord;
use user::dynamodb::repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl};
use user::dynamodb::user_record::UserRecord;
use user::service::authenticator_service::AuthenticatorServiceImpl;
use user::service::user_service::UserServiceImpl;
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

#[aura_integration_test(services = [DynamoDB(), SQS])]
async fn should_return_202_and_forward_woocommerce_create_webhook_to_sqs() {
    let ddb_client = get_dynamodb_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let async_product_command_service =
        AsyncProductCommandServiceImpl::new(get_sqs_client().await, SQS.queue_url());

    let shop_record = make_partner_shop_record();
    let shop_id = shop_record.shop_id;
    shop_repository.put_shop_record(shop_record).await.unwrap();

    let user_repo = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let (_, raw_token) = seed_partner_user_with_token(&user_repo, shop_id).await;
    let user_service_for_auth = UserServiceImpl::new(&user_repo);
    let user_service_for_handler = UserServiceImpl::new(&user_repo);
    let verifier = MockAccessTokenVerifierService::default();
    let authenticator = AuthenticatorServiceImpl::new(&verifier, &user_service_for_auth);

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
    headers.insert(
        "Authorization",
        format!("Bearer {}", String::from(raw_token))
            .parse()
            .unwrap(),
    );
    request.headers = headers;

    let response = handle_woocommerce(
        LambdaEvent::new(request, Context::default()),
        &get_shop_service,
        &user_service_for_handler,
        &authenticator,
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
