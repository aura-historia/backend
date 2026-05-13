use base64::Engine;
use common::currency::domain::Currency;
use common::product_state::domain::ProductState;
use fake::{Fake, Faker};
use http::HeaderMap;
use lambda_runtime::{Context, LambdaEvent};
use openssl::hash::MessageDigest;
use openssl::pkey::PKey;
use openssl::sign::Signer;
use product::service::command_service::MockCommandProductService;
use rstest::rstest;
use shop::core::partner_shop::PartnerShop;
use shop::core::partner_shop_api_key::{HashedPartnerShopApiKey, PartnerShopApiKey};
use shop::core::woocommerce_webhook_secret::WoocommerceWebhookSecret;
use shop::service::get_service::MockGetShopService;
use webhook_api::{
    WOOCOMMERCE_TOPIC_PRODUCT_CREATED, WOOCOMMERCE_TOPIC_PRODUCT_DELETED,
    WOOCOMMERCE_TOPIC_PRODUCT_UPDATED, handle,
};

const SECRET: &str = "woocommerce-secret";
const CREATED_BODY: &str = r#"{"id":17,"name":"Test Produkt Titel","slug":"test-produkt-titel","permalink":"http://aura-historia-test.local/product/test-produkt-titel/","date_created":"2026-05-13T19:22:31","date_modified":"2026-05-13T19:23:23","type":"simple","status":"publish","description":"<p>Hayde yallah test beschreibung</p>\n","short_description":"<p>Hayde yallah kurze test beschreibung</p>\n","price":"42.69","regular_price":"42.69","stock_status":"instock","categories":[{"id":15,"name":"Uncategorized","slug":"uncategorized"}],"images":[]}"#;
const UPDATED_BODY: &str = r#"{"id":17,"name":"Test Produkt Titel","slug":"test-produkt-titel","permalink":"http://aura-historia-test.local/product/test-produkt-titel/","date_created":"2026-05-13T19:22:31","date_modified":"2026-05-13T19:24:54","type":"simple","status":"publish","description":"<p>Hayde yallah test beschreibung</p>\n","short_description":"<p>Hayde yallah kurze test beschreibung</p>\n","price":"123.45","regular_price":"123.45","stock_status":"instock","categories":[{"id":15,"name":"Uncategorized","slug":"uncategorized"}],"images":[]}"#;
const DELETED_BODY: &str = r#"{"id":17}"#;

fn signature(body: &str) -> String {
    let key = PKey::hmac(SECRET.as_bytes()).unwrap();
    let mut signer = Signer::new(MessageDigest::sha256(), &key).unwrap();
    signer.update(body.as_bytes()).unwrap();
    base64::engine::general_purpose::STANDARD.encode(signer.sign_to_vec().unwrap())
}

fn partner_shop(api_key: &PartnerShopApiKey) -> PartnerShop {
    let mut shop: PartnerShop = Faker.fake();
    shop.hashed_api_key = Some(HashedPartnerShopApiKey::from(api_key.clone()));
    shop.woocommerce_webhook_secret = Some(WoocommerceWebhookSecret::from(SECRET));
    shop.shopify_currency = Some(Currency::Eur);
    shop
}

#[rstest]
#[case::created(
    WOOCOMMERCE_TOPIC_PRODUCT_CREATED,
    CREATED_BODY,
    ProductState::Available
)]
#[case::updated(
    WOOCOMMERCE_TOPIC_PRODUCT_UPDATED,
    UPDATED_BODY,
    ProductState::Available
)]
#[case::deleted(WOOCOMMERCE_TOPIC_PRODUCT_DELETED, DELETED_BODY, ProductState::Removed)]
#[tokio::test]
async fn should_ingest_product_when_valid_woocommerce_webhook_for_event_type(
    #[case] topic: &str,
    #[case] body: &str,
    #[case] expected_state: ProductState,
) {
    let api_key = PartnerShopApiKey::new();
    let shop = partner_shop(&api_key);
    let api_key_header: String = api_key.clone().into();

    let mut request = aws_lambda_events::apigw::ApiGatewayV2httpRequest::default();
    request.route_key = Some("POST /api/v1/webhooks/woocommerce/{shopId}".to_owned());
    request
        .path_parameters
        .insert("shopId".to_owned(), shop.shop_id.to_string());
    let mut headers = HeaderMap::new();
    headers.insert("x-api-key", api_key_header.parse().unwrap());
    headers.insert("x-wc-webhook-topic", topic.parse().unwrap());
    headers.insert("x-wc-webhook-signature", signature(body).parse().unwrap());
    request.headers = headers;
    request.body = Some(body.to_owned());
    let event = LambdaEvent::new(request, Context::default());

    let expected_shop = shop.clone();
    let mut get_shop_service = MockGetShopService::default();
    get_shop_service
        .expect_verify_partner_shop()
        .return_once(move |_, _| Box::pin(async move { Ok(expected_shop) }));

    let mut product_service = MockCommandProductService::default();
    product_service
        .expect_upsert()
        .return_once(move |commands| {
            Box::pin(async move {
                assert_eq!(1, commands.len());
                assert_eq!(Some(expected_state), commands[0].state);
                assert_eq!("17", commands[0].shops_product_id.to_string());
                vec![]
            })
        });

    let response = handle(event, &get_shop_service, &product_service)
        .await
        .unwrap();
    assert_eq!(200, response.status_code);
}
