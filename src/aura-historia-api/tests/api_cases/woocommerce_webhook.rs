use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};

use api_support::{
    seed_access_token_for, seed_current_fx_snapshot, seed_partner_shop, seed_shop, seed_user,
};
use base64::Engine;
use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
use serde_json::json;
use std::collections::HashSet;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use user_core::access_token::Scope;

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
        "permalink": "https://partner.example/product-listings/woo-cabinet",
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
        "SELECT title_text, price_amount, availability, shop_listing_id FROM product_listings WHERE shop_id = $1 AND shop_listing_id = $2",
    )
    .bind(uuid::Uuid::parse_str(&shop_id)?)
    .bind("17")
    .fetch_one(&pool)
    .await?;
    assert_eq!("Woo Cabinet", row.0);
    assert_eq!(4_269, row.1);
    assert_eq!("IN_STOCK", row.2);
    assert_eq!("17", row.3);
    Ok::<(), Box<dyn std::error::Error>>(())
    }.await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_withdraw_existing_product_listing_when_woocommerce_deleted_webhook_arrives() {
    let result: TestResult = async {
        let (shop_id, token) = webhook_auth().await?;
        let created = json!({
            "id": 18,
            "name": "Woo Cabinet",
            "permalink": "https://partner.example/product-listings/woo-cabinet",
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
        let event_count_after_withdrawal: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM product_listing_events WHERE product_listing_id = (SELECT product_listing_id FROM product_listings WHERE shop_id = $1 AND shop_listing_id = $2)",
        )
        .bind(uuid::Uuid::parse_str(&shop_id)?)
        .bind("18")
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(&shop_id, &token, "product.deleted", &deleted)
                .await?
                .status()
        );
        let event_count_after_redelivery: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM product_listing_events WHERE product_listing_id = (SELECT product_listing_id FROM product_listings WHERE shop_id = $1 AND shop_listing_id = $2)",
        )
        .bind(uuid::Uuid::parse_str(&shop_id)?)
        .bind("18")
        .fetch_one(&pool)
        .await?;
        assert_eq!(event_count_after_withdrawal, event_count_after_redelivery);

        let lifecycle: (String,) = sqlx::query_as(
            "SELECT lifecycle FROM product_listings WHERE shop_id = $1 AND shop_listing_id = $2",
        )
        .bind(uuid::Uuid::parse_str(&shop_id)?)
        .bind("18")
        .fetch_one(&pool)
        .await?;
        assert_eq!("WITHDRAWN", lifecycle.0);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_ignore_missing_product_listing_when_woocommerce_deleted_webhook_arrives() {
    let result: TestResult = async {
        let (shop_id, token) = webhook_auth().await?;
        let deleted = json!({ "id": 999 }).to_string();

        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(&shop_id, &token, "product.deleted", &deleted)
                .await?
                .status()
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
            "SELECT COUNT(*) FROM product_listing_events WHERE product_listing_id = (SELECT product_listing_id FROM product_listings WHERE shop_id = $1 AND shop_listing_id = $2)",
        )
        .bind(uuid::Uuid::parse_str(&shop_id)?)
        .bind("20")
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(&shop_id, &token, "product.updated", &updated).await?.status()
        );

        let product_listing: (uuid::Uuid, i64, String) = sqlx::query_as(
            "SELECT product_listing_id, price_amount, availability FROM product_listings WHERE shop_id = $1 AND shop_listing_id = $2",
        )
        .bind(uuid::Uuid::parse_str(&shop_id)?)
        .bind("20")
        .fetch_one(&pool)
        .await?;
        let event_count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM product_listing_events WHERE product_listing_id = $1",
        )
        .bind(product_listing.0)
        .fetch_one(&pool)
        .await?;
        assert_eq!(12_345, product_listing.1);
        assert_eq!("OUT_OF_STOCK", product_listing.2);
        assert_eq!(event_count_after_update.0, event_count.0);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_preserve_availability_for_woocommerce_updates_without_supported_stock_status() {
    let result: TestResult = async {
        let (shop_id, token) = webhook_auth().await?;
        let created = product_body(25, "42.00", "publish", "instock");
        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(&shop_id, &token, "product.created", &created)
                .await?
                .status()
        );

        let pool = get_postgres_client().await;
        let availability_event_count_before_updates: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM product_listing_events WHERE product_listing_id = (SELECT product_listing_id FROM product_listings WHERE shop_id = $1 AND shop_listing_id = $2) AND event_type = 'PRODUCT_LISTING_AVAILABILITY_CHANGED'",
        )
        .bind(uuid::Uuid::parse_str(&shop_id)?)
        .bind("25")
        .fetch_one(&pool)
        .await?;
        assert_eq!(0, availability_event_count_before_updates.0);

        let missing_stock_status = json!({
            "id": 25,
            "name": "Woo Cabinet",
            "permalink": "https://partner.example/product-listings/woo-cabinet-25",
            "description": "<p>Cabinet description</p>",
            "price": "42.00",
            "status": "publish",
            "images": []
        })
        .to_string();
        let unsupported_stock_status = product_body(25, "42.00", "publish", "unsupported");
        for updated in [&missing_stock_status, &unsupported_stock_status] {
            assert_eq!(
                reqwest::StatusCode::NO_CONTENT,
                send(&shop_id, &token, "product.updated", updated)
                    .await?
                    .status()
            );
        }

        let existing_listing: (String, String) = sqlx::query_as(
            "SELECT availability, lifecycle FROM product_listings WHERE shop_id = $1 AND shop_listing_id = $2",
        )
        .bind(uuid::Uuid::parse_str(&shop_id)?)
        .bind("25")
        .fetch_one(&pool)
        .await?;
        assert_eq!("IN_STOCK", existing_listing.0);
        assert_eq!("ACTIVE", existing_listing.1);
        let availability_event_count_after_updates: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM product_listing_events WHERE product_listing_id = (SELECT product_listing_id FROM product_listings WHERE shop_id = $1 AND shop_listing_id = $2) AND event_type = 'PRODUCT_LISTING_AVAILABILITY_CHANGED'",
        )
        .bind(uuid::Uuid::parse_str(&shop_id)?)
        .bind("25")
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            availability_event_count_before_updates,
            availability_event_count_after_updates
        );

        let missing_status_new_listing = json!({
            "id": 26,
            "name": "Woo Cabinet",
            "permalink": "https://partner.example/product-listings/woo-cabinet-26",
            "description": "<p>Cabinet description</p>",
            "price": "42.00",
            "status": "publish",
            "images": []
        })
        .to_string();
        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(
                &shop_id,
                &token,
                "product.updated",
                &missing_status_new_listing,
            )
            .await?
            .status()
        );
        let new_listing_availability: Option<String> = sqlx::query_scalar(
            "SELECT availability FROM product_listings WHERE shop_id = $1 AND shop_listing_id = $2",
        )
        .bind(uuid::Uuid::parse_str(&shop_id)?)
        .bind("26")
        .fetch_one(&pool)
        .await?;
        assert_eq!(None, new_listing_availability);
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
            seed_access_token_for(user_id, HashSet::from([Scope::ProductListingsWrite])).await,
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
    let token = seed_access_token_for(user_id, HashSet::from([Scope::ProductListingsWrite])).await;
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
    sqlx::query_scalar("SELECT COUNT(*) FROM product_listings WHERE shop_id = $1")
        .bind(uuid::Uuid::from(shop_id))
        .fetch_one(&pool)
        .await
}

fn product_body(id: u64, price: &str, status: &str, stock_status: &str) -> String {
    json!({
        "id": id,
        "name": "Woo Cabinet",
        "permalink": format!("https://partner.example/product-listings/woo-cabinet-{id}"),
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
