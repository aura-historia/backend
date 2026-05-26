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
use shop::core::aura_historia_api_key::{HashedRawAccessToken, RawAccessToken};
use shop::core::woocommerce_webhook_secret::WoocommerceWebhookSecret;
use shop::dynamodb::repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl};
use shop::dynamodb::shop_record::ShopRecord;
use shop::service::get_service::GetShopServiceImpl;
use test_api::*;
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

fn make_partner_shop_record(api_key: &RawAccessToken) -> ShopRecord {
    let hashed: HashedRawAccessToken = api_key.clone().into();
    let mut record: ShopRecord = Faker.fake();
    record.partner_api_key_short = Some(hashed.short_token().to_string());
    record.partner_api_key_long_hash = Some(hashed.long_token_hash().to_string());
    record.partner_user_id = Some(Faker.fake());
    record.woocommerce_webhook_secret = Some(WoocommerceWebhookSecret::from(SECRET));
    record.woocommerce_currency = Some(common::currency::record::CurrencyRecord::Eur);
    record.woocommerce_language = Some(common::language::record::LanguageRecord::En);
    record
}

#[localstack_test(services = [DynamoDB(), SQS])]
async fn should_return_202_and_forward_woocommerce_create_webhook_to_sqs() {
    let ddb_client = get_dynamodb_client().await;
    let shop_repository = ShopDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let async_product_command_service =
        AsyncProductCommandServiceImpl::new(get_sqs_client().await, SQS.queue_url());

    let api_key = RawAccessToken::new();
    let shop_record = make_partner_shop_record(&api_key);
    let shop_id = shop_record.shop_id;
    shop_repository.put_shop_record(shop_record).await.unwrap();

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
    let api_key_str: String = api_key.into();
    let mut request = ApiGatewayV2httpRequestProxy::builder()
        .http_method(http::Method::POST)
        .route_key("POST /api/v1/webhooks/woocommerce/{shopId}")
        .path_parameter("shopId", shop_id.to_string())
        .header("x-aura-historia-access-token", api_key_str)
        .body_serde(&body_json)
        .build();
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-aura-historia-access-token",
        request
            .headers
            .get("x-aura-historia-access-token")
            .unwrap()
            .clone(),
    );
    headers.insert(
        "x-wc-webhook-topic",
        WOOCOMMERCE_TOPIC_PRODUCT_CREATED.parse().unwrap(),
    );
    headers.insert("x-wc-webhook-signature", signature(&body).parse().unwrap());
    request.headers = headers;

    let response = handle_woocommerce(
        LambdaEvent::new(request, Context::default()),
        &get_shop_service,
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
