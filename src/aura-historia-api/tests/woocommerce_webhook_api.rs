mod api_support;

use api_support::{
    seed_access_token_for, seed_current_fx_snapshot, seed_partner_shop, seed_shop, seed_user,
};
use base64::Engine;
use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
use serde_json::json;
use std::collections::HashSet;
use test_api::{
    AuraHistoriaApi, IntegrationTestService, OpenSearch, Postgres, aura_integration_test,
    get_postgres_client,
};
use user_core::access_token::Scope;

const BUSINESS_SCHEMA: Postgres = Postgres::new_schema_once("migrations");

const OPENSEARCH: OpenSearch = OpenSearch();
static AURA_API: AuraHistoriaApi = AuraHistoriaApi::new(api_support::aura_api_app);
const SECRET: &str = "woocommerce-webhook-test-secret";

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn assert_test_result(result: TestResult) {
    assert!(result.is_ok(), "{result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_persist_woocommerce_created_webhook_after_signature_validation() {
    let result: TestResult = async {
    let (shop_id, token) = webhook_auth().await?;
    let body = json!({
        "id": 17,
        "name": "Woo Cabinet",
        "permalink": "https://partner.example/products/woo-cabinet",
        "description": "<p>Cabinet description</p>",
        "price": "42.699",
        "status": "publish",
        "stock_status": "instock",
        "images": []
    })
    .to_string();

    let response = send(&shop_id, &token, "product.created", &body).await?;
    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
    assert!(response.bytes().await?.is_empty());

    let pool = get_postgres_client().await;
    let row = sqlx::query_as::<_, (String, i64, String, String)>(
        "SELECT title_text, price_amount, state, shop_listing_id FROM products WHERE shop_id = $1 AND shop_listing_id = $2",
    )
    .bind(uuid::Uuid::parse_str(&shop_id)?)
    .bind("17")
    .fetch_one(&pool)
    .await?;
    assert_eq!("Woo Cabinet", row.0);
    assert_eq!(4_269, row.1);
    assert_eq!("AVAILABLE", row.2);
    assert_eq!("17", row.3);
    Ok::<(), Box<dyn std::error::Error>>(())
    }.await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_mark_existing_product_removed_when_woocommerce_deleted_webhook_arrives() {
    let result: TestResult = async {
        let (shop_id, token) = webhook_auth().await?;
        let created = json!({
            "id": 18,
            "name": "Woo Cabinet",
            "permalink": "https://partner.example/products/woo-cabinet",
            "status": "publish",
            "stock_status": "instock",
            "images": []
        })
        .to_string();
        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(&shop_id, &token, "product.created", &created)
                .await?
                .status()
        );

        let deleted = json!({ "id": 18 }).to_string();
        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(&shop_id, &token, "product.deleted", &deleted)
                .await?
                .status()
        );

        let pool = get_postgres_client().await;
        let state: (String,) = sqlx::query_as(
            "SELECT state FROM products WHERE shop_id = $1 AND shop_listing_id = $2",
        )
        .bind(uuid::Uuid::parse_str(&shop_id)?)
        .bind("18")
        .fetch_one(&pool)
        .await?;
        assert_eq!("REMOVED", state.0);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_update_existing_product_and_not_append_event_for_redelivery() {
    let result: TestResult = async {
        let (shop_id, token) = webhook_auth().await?;
        let created = product_body(20, "42.00", "publish", "instock");
        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(&shop_id, &token, "product.created", &created).await?.status()
        );

        let updated = product_body(20, "123.45", "publish", "outofstock");
        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(&shop_id, &token, "product.updated", &updated).await?.status()
        );

        let pool = get_postgres_client().await;
        let event_count_after_update: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM product_events WHERE product_id = (SELECT product_id FROM products WHERE shop_id = $1 AND shop_listing_id = $2)",
        )
        .bind(uuid::Uuid::parse_str(&shop_id)?)
        .bind("20")
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(&shop_id, &token, "product.updated", &updated).await?.status()
        );

        let product: (uuid::Uuid, i64, String) = sqlx::query_as(
            "SELECT product_id, price_amount, state FROM products WHERE shop_id = $1 AND shop_listing_id = $2",
        )
        .bind(uuid::Uuid::parse_str(&shop_id)?)
        .bind("20")
        .fetch_one(&pool)
        .await?;
        let event_count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM product_events WHERE product_id = $1",
        )
        .bind(product.0)
        .fetch_one(&pool)
        .await?;
        assert_eq!(12_345, product.1);
        assert_eq!("SOLD", product.2);
        assert_eq!(event_count_after_update.0, event_count.0);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_woocommerce_webhook_with_invalid_signature() {
    let result: TestResult = async {
        let (shop_id, token) = webhook_auth().await?;
        let body = json!({ "id": 19 }).to_string();
        let response = reqwest::Client::new()
            .post(format!(
                "{}/api/v1/webhooks/woocommerce/{shop_id}",
                AURA_API.base_url()
            ))
            .bearer_auth(token)
            .header("x-wc-webhook-topic", "product.deleted")
            .header("x-wc-webhook-signature", "invalid")
            .body(body)
            .send()
            .await?;
        assert_eq!(reqwest::StatusCode::UNAUTHORIZED, response.status());
        assert_eq!(
            "BAD_HEADER_VALUE",
            response.json::<serde_json::Value>().await?["error"]
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_woocommerce_webhook_without_product_write_capability() {
    let result: TestResult = async {
        let shop = seed_shop().await;
        configure_webhook_shop(shop.id()).await?;
        let user_id = seed_user("USER").await;
        seed_partner_shop(user_id, shop.id()).await;
        let token = String::from(seed_access_token_for(user_id, HashSet::new()).await);
        let body = json!({ "id": 21 }).to_string();

        let response = send(&shop.id().to_string(), &token, "product.deleted", &body).await?;
        assert_eq!(reqwest::StatusCode::FORBIDDEN, response.status());
        assert_eq!(
            "PARTNER_SHOP_NOT_PARTNERED",
            response.json::<serde_json::Value>().await?["error"]
        );
        assert_eq!(0, product_count(shop.id()).await?);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_woocommerce_webhook_from_user_not_linked_to_shop() {
    let result: TestResult = async {
        let shop = seed_shop().await;
        configure_webhook_shop(shop.id()).await?;
        let user_id = seed_user("USER").await;
        let token = String::from(
            seed_access_token_for(user_id, HashSet::from([Scope::ProductsWrite])).await,
        );
        let body = json!({ "id": 22 }).to_string();

        let response = send(&shop.id().to_string(), &token, "product.deleted", &body).await?;
        assert_eq!(reqwest::StatusCode::FORBIDDEN, response.status());
        assert_eq!(
            "PARTNER_SHOP_NOT_PARTNERED",
            response.json::<serde_json::Value>().await?["error"]
        );
        assert_eq!(0, product_count(shop.id()).await?);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_missing_woocommerce_auth_and_required_headers_without_persisting_product() {
    let result: TestResult = async {
        let (shop_id, token) = webhook_auth().await?;
        let body = json!({ "id": 24 }).to_string();
        let url = format!(
            "{}/api/v1/webhooks/woocommerce/{shop_id}",
            AURA_API.base_url()
        );

        let missing_authorization = reqwest::Client::new()
            .post(&url)
            .header("x-wc-webhook-topic", "product.deleted")
            .header("x-wc-webhook-signature", signature(&body))
            .body(body.clone())
            .send()
            .await?;
        assert_eq!(
            reqwest::StatusCode::UNAUTHORIZED,
            missing_authorization.status()
        );
        assert_eq!(
            "INVALID_CREDENTIALS",
            missing_authorization.json::<serde_json::Value>().await?["error"]
        );

        let missing_topic = reqwest::Client::new()
            .post(&url)
            .bearer_auth(&token)
            .header("x-wc-webhook-signature", signature(&body))
            .body(body.clone())
            .send()
            .await?;
        assert_eq!(reqwest::StatusCode::BAD_REQUEST, missing_topic.status());
        assert_eq!(
            "BAD_HEADER_VALUE",
            missing_topic.json::<serde_json::Value>().await?["error"]
        );

        let missing_signature = reqwest::Client::new()
            .post(&url)
            .bearer_auth(&token)
            .header("x-wc-webhook-topic", "product.deleted")
            .body(body)
            .send()
            .await?;
        assert_eq!(
            reqwest::StatusCode::UNAUTHORIZED,
            missing_signature.status()
        );
        assert_eq!(
            "BAD_HEADER_VALUE",
            missing_signature.json::<serde_json::Value>().await?["error"]
        );
        assert_eq!(
            0,
            product_count(uuid::Uuid::parse_str(&shop_id)?.into()).await?
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_woocommerce_request_shape_without_persisting_product() {
    let result: TestResult = async {
        let (shop_id, token) = webhook_auth().await?;
        for (topic, body, expected_error) in [
            (
                "orders.created",
                json!({ "id": 23 }).to_string(),
                "BAD_HEADER_VALUE",
            ),
            ("product.created", "not-json".to_owned(), "BAD_BODY_VALUE"),
            ("product.created", "".to_owned(), "BAD_BODY_VALUE"),
        ] {
            let response = send(&shop_id, &token, topic, &body).await?;
            assert_eq!(reqwest::StatusCode::BAD_REQUEST, response.status());
            assert_eq!(
                expected_error,
                response.json::<serde_json::Value>().await?["error"]
            );
        }
        assert_eq!(
            0,
            product_count(uuid::Uuid::parse_str(&shop_id)?.into()).await?
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

async fn webhook_auth() -> Result<(String, String), Box<dyn std::error::Error>> {
    let pool = get_postgres_client().await;
    seed_current_fx_snapshot(&pool).await;
    let shop = seed_shop().await;
    let shop_id = shop.id().to_string();
    configure_webhook_shop(shop.id()).await?;
    let user_id = seed_user("USER").await;
    seed_partner_shop(user_id, shop.id()).await;
    let token = seed_access_token_for(user_id, HashSet::from([Scope::ProductsWrite])).await;
    Ok((shop_id, String::from(token)))
}

async fn configure_webhook_shop(shop_id: shop_core::shop_id::ShopId) -> Result<(), sqlx::Error> {
    let pool = get_postgres_client().await;
    sqlx::query(
        "UPDATE shops SET woocommerce_webhook_secret = $1, woocommerce_currency = 'EUR', woocommerce_language = 'en' WHERE shop_id = $2",
    )
    .bind(SECRET)
    .bind(uuid::Uuid::from(shop_id))
    .execute(&pool)
    .await?;
    Ok(())
}

async fn product_count(shop_id: shop_core::shop_id::ShopId) -> Result<i64, sqlx::Error> {
    let pool = get_postgres_client().await;
    sqlx::query_scalar("SELECT COUNT(*) FROM products WHERE shop_id = $1")
        .bind(uuid::Uuid::from(shop_id))
        .fetch_one(&pool)
        .await
}

fn product_body(id: u64, price: &str, status: &str, stock_status: &str) -> String {
    json!({
        "id": id,
        "name": "Woo Cabinet",
        "permalink": format!("https://partner.example/products/woo-cabinet-{id}"),
        "description": "<p>Cabinet description</p>",
        "price": price,
        "status": status,
        "stock_status": stock_status,
        "images": []
    })
    .to_string()
}

async fn send(
    shop_id: &str,
    token: &str,
    topic: &str,
    body: &str,
) -> Result<reqwest::Response, reqwest::Error> {
    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/webhooks/woocommerce/{shop_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(token)
        .header("x-wc-webhook-topic", topic)
        .header("x-wc-webhook-signature", signature(body))
        .body(body.to_owned())
        .send()
        .await
}

fn signature(body: &str) -> String {
    let key = PKey::hmac(SECRET.as_bytes())
        .unwrap_or_else(|error| panic!("failed creating HMAC key: {error}"));
    let mut signer = Signer::new(MessageDigest::sha256(), &key)
        .unwrap_or_else(|error| panic!("failed creating HMAC signer: {error}"));
    signer
        .update(body.as_bytes())
        .unwrap_or_else(|error| panic!("failed signing webhook body: {error}"));
    base64::engine::general_purpose::STANDARD.encode(
        signer
            .sign_to_vec()
            .unwrap_or_else(|error| panic!("failed finalizing HMAC signature: {error}")),
    )
}
