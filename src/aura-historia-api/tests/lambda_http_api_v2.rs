#[allow(dead_code)]
mod api_support;

use aura_historia_api::{ApiConfig, lambda::handle_http_api_v2_request};
use axum::{Router, body::to_bytes, http::StatusCode};
use base64::Engine;
use lambda_http::{
    RequestExt,
    lambda_runtime::{Context, LambdaEvent},
};
use listing_source_core::ListingSourceId;
use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
use serial_test::serial;
use std::{
    collections::HashMap,
    ffi::OsString,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use test_api::{IntegrationTestService, OpenSearch, Postgres, get_postgres_client};
use tower::ServiceExt;
use user_core::access_token::Scope;

const BUSINESS_SCHEMA: Postgres = Postgres::new_schema_once("migrations");
const OPENSEARCH: OpenSearch = OpenSearch();
const WOOCOMMERCE_WEBHOOK_SECRET: &str = "lambda-http-api-v2-webhook-secret";

#[test_api::aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH])]
async fn should_traverse_the_composed_public_router_from_an_http_api_v2_event() {
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        let app = api_support::aura_api_app().await;
        let request = listing_sources_request()?;

        let response = handle_http_api_v2_request(app.clone(), request).await?;
        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(axum::http::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
        assert_eq!(
            Some("lambda-route-test"),
            response
                .headers()
                .get("x-correlation-id")
                .and_then(|value| value.to_str().ok())
        );
        assert_eq!(
            Some("*"),
            response
                .headers()
                .get(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|value| value.to_str().ok())
        );
        let request_id = response
            .headers()
            .get("x-request-id")
            .ok_or_else(|| std::io::Error::other("missing request ID"))?
            .to_str()?;
        assert!(uuid::Uuid::parse_str(request_id).is_ok());
        let response_body = to_bytes(response.into_body(), usize::MAX).await?;
        let response_data: serde_json::Value = serde_json::from_slice(&response_body)?;
        assert_eq!(response_data["size"], 1);
        assert!(response_data["items"].is_array());

        let mut invalid_auth_request = listing_sources_request()?;
        invalid_auth_request.headers_mut().insert(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer invalid"),
        );
        let invalid_auth_response = handle_http_api_v2_request(app, invalid_auth_request).await?;
        assert_eq!(StatusCode::UNAUTHORIZED, invalid_auth_response.status());
        assert_eq!(
            Some("no-store"),
            invalid_auth_response
                .headers()
                .get(axum::http::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
        Ok(())
    }
    .await;

    assert!(result.is_ok(), "{result:?}");
}

#[test_api::aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH])]
async fn should_enforce_admin_access_through_the_composed_http_api_v2_router() {
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        let app = api_support::aura_api_app().await;
        let admin_id = api_support::seed_user("ADMIN").await;
        let admin_token = String::from(
            api_support::seed_access_token_for(admin_id, std::collections::HashSet::new()).await,
        );
        let user_id = api_support::seed_user("USER").await;
        let user_token = String::from(
            api_support::seed_access_token_for(user_id, std::collections::HashSet::new()).await,
        );

        let allowed = handle_http_api_v2_request(
            app.clone(),
            http_api_v2_json_request("GET", "/api/v1/admin/overview", &admin_token, "")?,
        )
        .await?;
        assert_eq!(StatusCode::OK, allowed.status());
        assert_eq!(
            Some("no-store"),
            allowed
                .headers()
                .get(axum::http::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );

        let denied = handle_http_api_v2_request(
            app,
            http_api_v2_json_request("GET", "/api/v1/admin/overview", &user_token, "")?,
        )
        .await?;
        assert_eq!(StatusCode::FORBIDDEN, denied.status());
        let denied_body = to_bytes(denied.into_body(), usize::MAX).await?;
        assert_eq!(
            serde_json::json!("FORBIDDEN"),
            serde_json::from_slice::<serde_json::Value>(&denied_body)?["error"]
        );
        Ok(())
    }
    .await;

    assert!(result.is_ok(), "{result:?}");
}

#[test_api::aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH])]
async fn should_acknowledge_signed_raw_woocommerce_bytes_through_the_composed_http_api_v2_router() {
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        let app = api_support::aura_api_app().await;
        let listing_source_uuid = api_support::seed_listing_source().await;
        configure_woocommerce_listing_source(listing_source_uuid).await?;
        let user_id = api_support::seed_user("USER").await;
        api_support::seed_partnership_membership(user_id, listing_source_uuid).await;
        api_support::seed_operator_partnership_listing_source_grant(listing_source_uuid).await;
        let token = String::from(
            api_support::seed_access_token_for(
                user_id,
                std::collections::HashSet::from([Scope::ProductListingsWrite]),
            )
            .await,
        );
        let listing_source_id = ListingSourceId::try_from(listing_source_uuid)?.to_string();
        let raw_body = br#"{ "id": 901, "name": "Lambda \u00e9 cabinet", "permalink": "https://partner.example/lambda-http-api-v2-901", "price": "42.00", "status": "publish", "stock_status": "instock", "images": [], "futureWooKey": "\u00e9" }"#;
        let signature = woocommerce_signature(raw_body)?;
        let request = http_api_v2_binary_woocommerce_event(
            &format!("/api/v1/webhooks/woocommerce/{listing_source_id}"),
            &token,
            "product.updated",
            &signature,
            "lambda-http-api-v2-901",
            raw_body,
        )?;

        let response = serialize_http_api_v2_router_response(app, request).await?;
        assert_eq!(serde_json::json!(204), response["statusCode"]);
        assert_eq!(serde_json::json!(""), response["body"]);
        assert_eq!(serde_json::json!(false), response["isBase64Encoded"]);

        let pool = get_postgres_client().await;
        let source_payload: serde_json::Value = sqlx::query_scalar(
            "SELECT revision.source_payload \
             FROM product_listing_raw_revisions revision \
             JOIN product_listing_raw_streams stream \
               ON stream.product_listing_raw_stream_id = revision.product_listing_raw_stream_id \
             WHERE stream.listing_source_id = $1 \
               AND stream.ingestion_method = 'WOOCOMMERCE' \
               AND stream.source_record_key = '901'",
        )
        .bind(listing_source_uuid)
        .fetch_one(&pool)
        .await?;
        assert_eq!(serde_json::json!("é"), source_payload["futureWooKey"]);
        Ok(())
    }
    .await;

    assert!(result.is_ok(), "{result:?}");
}

#[test_api::aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH])]
async fn should_persist_an_authenticated_partner_write_through_the_http_api_v2_adapter() {
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        let app = api_support::aura_api_app().await;
        let pool = get_postgres_client().await;
        api_support::seed_current_fx_snapshot(&pool).await;
        let listing_source_id = api_support::seed_listing_source().await;
        let user_id = api_support::seed_user("USER").await;
        api_support::seed_partnership_membership(user_id, listing_source_id).await;
        api_support::seed_operator_partnership_listing_source_grant(listing_source_id).await;
        let token = String::from(
            api_support::seed_access_token_for(
                user_id,
                std::collections::HashSet::from([Scope::ProductListingsWrite]),
            )
            .await,
        );
        let listing_source_id = ListingSourceId::try_from(listing_source_id)?.to_string();
        let source_listing_id = "lambda-v2-partner-write";
        let body = serde_json::json!([{
            "sourceListingId": source_listing_id,
            "title": { "text": "Lambda adapter cabinet", "language": "en" },
            "description": { "text": "Persisted through HTTP API v2.", "language": "en" },
            "availability": "AVAILABLE",
            "url": "https://partner.example/lambda-v2-partner-write",
            "images": []
        }])
        .to_string();
        let request = http_api_v2_json_request(
            "POST",
            &format!("/api/v1/listing-sources/{listing_source_id}/product-listings"),
            &token,
            &body,
        )?;

        let response = handle_http_api_v2_request(app.clone(), request).await?;
        assert_eq!(StatusCode::OK, response.status());
        assert_eq!(
            serde_json::json!([]),
            serde_json::from_slice::<serde_json::Value>(
                &to_bytes(response.into_body(), usize::MAX).await?
            )?
        );
        let persisted: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = $2",
        )
        .bind(ListingSourceId::try_from(listing_source_id.as_str())?.as_uuid())
        .bind(source_listing_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(1, persisted);

        let denied = http_api_v2_json_request(
            "POST",
            &format!("/api/v1/listing-sources/{listing_source_id}/product-listings"),
            "invalid",
            &body,
        )?;
        let denied_response = handle_http_api_v2_request(app, denied).await?;
        assert_eq!(StatusCode::UNAUTHORIZED, denied_response.status());
        let after_denied: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = $2",
        )
        .bind(ListingSourceId::try_from(listing_source_id.as_str())?.as_uuid())
        .bind(source_listing_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(1, after_denied);
        Ok(())
    }
    .await;

    assert!(result.is_ok(), "{result:?}");
}

#[serial(lambda_composition_environment)]
#[test_api::aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_serve_health_through_full_lambda_composition_when_google_and_opensearch_are_unavailable()
 {
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        let _environment = EnvironmentGuard::set(&[
            ("OPENSEARCH_ENDPOINT_URL", "http://127.0.0.1:9"),
            ("STAGE", "ephemeral"),
            (
                "GOOGLE_APPLICATION_CREDENTIALS",
                "/tmp/aura-historia-unavailable-google-adc.json",
            ),
            ("AWS_EC2_METADATA_DISABLED", "true"),
            ("AWS_CONFIG_FILE", "/dev/null"),
            ("AWS_SHARED_CREDENTIALS_FILE", "/dev/null"),
        ]);
        let config = unavailable_dependency_api_config()?;
        let app = aura_historia_api::lambda_app_from_config_and_pool(
            &config,
            get_postgres_client().await,
        )
        .await?;
        let response = serialize_http_api_v2_router_response(
            app,
            http_api_v2_event(include_str!("fixtures/http_api_v2_health.json"))?,
        )
        .await?;

        assert_eq!(serde_json::json!(200), response["statusCode"]);
        assert_eq!(serde_json::json!("ok\n"), response["body"]);
        assert_eq!(serde_json::json!(false), response["isBase64Encoded"]);
        Ok(())
    }
    .await;

    assert!(result.is_ok(), "{result:?}");
}

fn listing_sources_request() -> Result<lambda_http::Request, lambda_http::Error> {
    Ok(
        lambda_http::request::from_str(include_str!("fixtures/http_api_v2_listing_sources.json"))?
            .with_lambda_context(context_with_remaining(Duration::from_secs(60))),
    )
}

fn http_api_v2_json_request(
    method: &str,
    path: &str,
    access_token: &str,
    body: &str,
) -> Result<lambda_http::Request, lambda_http::Error> {
    let event = serde_json::json!({
        "version": "2.0",
        "routeKey": "$default",
        "rawPath": path,
        "rawQueryString": "",
        "headers": {
            "authorization": format!("Bearer {access_token}"),
            "content-type": "application/json",
            "host": "api.example.test",
            "origin": "https://client.example.test"
        },
        "requestContext": {
            "stage": "$default",
            "http": {
                "method": method,
                "path": path,
                "protocol": "HTTP/1.1",
                "sourceIp": "127.0.0.1",
                "userAgent": "aura-historia-api-lambda-test"
            }
        },
        "body": body,
        "isBase64Encoded": false
    });
    Ok(lambda_http::request::from_str(&event.to_string())?
        .with_lambda_context(context_with_remaining(Duration::from_secs(60))))
}

async fn serialize_http_api_v2_router_response(
    app: Router,
    request: lambda_http::request::LambdaRequest,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let service = lambda_http::service_fn(move |request: lambda_http::Request| {
        let app = app.clone();
        async move { handle_http_api_v2_request(app, request).await }
    });
    let response = lambda_http::Adapter::from(service)
        .oneshot(LambdaEvent {
            payload: request,
            context: context_with_remaining(Duration::from_secs(60)),
        })
        .await?;
    Ok(serde_json::to_value(response)?)
}

fn http_api_v2_event(
    event: &str,
) -> Result<lambda_http::request::LambdaRequest, serde_json::Error> {
    serde_json::from_str(event)
}

fn http_api_v2_binary_woocommerce_event(
    path: &str,
    access_token: &str,
    topic: &str,
    signature: &str,
    delivery_id: &str,
    body: &[u8],
) -> Result<lambda_http::request::LambdaRequest, serde_json::Error> {
    let event = serde_json::json!({
        "version": "2.0",
        "routeKey": "$default",
        "rawPath": path,
        "rawQueryString": "",
        "headers": {
            "authorization": format!("Bearer {access_token}"),
            "content-type": "application/json",
            "host": "api.example.test",
            "x-wc-webhook-topic": topic,
            "x-wc-webhook-signature": signature,
            "x-wc-webhook-delivery-id": delivery_id
        },
        "requestContext": {
            "stage": "$default",
            "http": {
                "method": "POST",
                "path": path,
                "protocol": "HTTP/1.1",
                "sourceIp": "127.0.0.1",
                "userAgent": "aura-historia-api-lambda-test"
            }
        },
        "body": base64::engine::general_purpose::STANDARD.encode(body),
        "isBase64Encoded": true
    });
    serde_json::from_value(event)
}

async fn configure_woocommerce_listing_source(
    listing_source_id: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    let pool = get_postgres_client().await;
    sqlx::query(
        "INSERT INTO listing_source_ingestion_methods (listing_source_id, ingestion_method) VALUES ($1, 'WOOCOMMERCE')",
    )
    .bind(listing_source_id)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO listing_source_woocommerce_ingestion_configurations (listing_source_id, webhook_secret, currency, language) VALUES ($1, $2, 'EUR', 'en')",
    )
    .bind(listing_source_id)
    .bind(WOOCOMMERCE_WEBHOOK_SECRET)
    .execute(&pool)
    .await?;
    Ok(())
}

fn woocommerce_signature(body: &[u8]) -> Result<String, openssl::error::ErrorStack> {
    let key = PKey::hmac(WOOCOMMERCE_WEBHOOK_SECRET.as_bytes())?;
    let mut signer = Signer::new(MessageDigest::sha256(), &key)?;
    signer.update(body)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(signer.sign_to_vec()?))
}

fn unavailable_dependency_api_config() -> Result<ApiConfig, aura_historia_api::ApiConfigError> {
    let values = HashMap::from([
        (
            aura_historia_api::COGNITO_ISSUER_ENV,
            "https://issuer.lambda.test",
        ),
        (
            aura_historia_api::COGNITO_JWKS_URL_ENV,
            "https://issuer.lambda.test/.well-known/jwks.json",
        ),
        (
            aura_historia_api::COGNITO_APP_CLIENT_IDS_ENV,
            "lambda-test-client",
        ),
        (
            aura_historia_api::COGNITO_USER_POOL_ID_ENV,
            "lambda-test-pool",
        ),
        (aura_historia_api::STRIPE_API_KEY_ENV, "sk_test_lambda"),
        (
            aura_historia_api::STRIPE_CHECKOUT_SUCCESS_URL_ENV,
            "https://client.example.test/checkout/success",
        ),
        (
            aura_historia_api::STRIPE_CHECKOUT_CANCEL_URL_ENV,
            "https://client.example.test/checkout/cancel",
        ),
        (
            aura_historia_api::STRIPE_PORTAL_RETURN_URL_ENV,
            "https://client.example.test/billing",
        ),
        (
            aura_historia_api::STRIPE_PRO_MONTHLY_PRICE_ID_ENV,
            "price_pro_monthly",
        ),
        (
            aura_historia_api::STRIPE_PRO_YEARLY_PRICE_ID_ENV,
            "price_pro_yearly",
        ),
        (
            aura_historia_api::STRIPE_ULTIMATE_MONTHLY_PRICE_ID_ENV,
            "price_ultimate_monthly",
        ),
        (
            aura_historia_api::STRIPE_ULTIMATE_YEARLY_PRICE_ID_ENV,
            "price_ultimate_yearly",
        ),
        (aura_historia_api::ZOHO_LIST_KEY_ENV, "lambda-test-list"),
        (aura_historia_api::ZOHO_CLIENT_ID_ENV, "lambda-test-client"),
        (
            aura_historia_api::ZOHO_CLIENT_SECRET_ENV,
            "lambda-test-secret",
        ),
        (
            aura_historia_api::ZOHO_REFRESH_TOKEN_ENV,
            "lambda-test-refresh",
        ),
        (
            aura_historia_api::ZOHO_ACCOUNTS_URL_ENV,
            "https://accounts.example.test",
        ),
        (
            aura_historia_api::ZOHO_CAMPAIGNS_URL_ENV,
            "https://campaigns.example.test",
        ),
    ]);
    ApiConfig::from_getter(|name| values.get(name).map(ToString::to_string))
}

struct EnvironmentGuard {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl EnvironmentGuard {
    fn set(values: &[(&'static str, &'static str)]) -> Self {
        let previous = values
            .iter()
            .map(|(name, value)| {
                let previous = std::env::var_os(name);
                // This serial test owns these composition-only variables until its guard drops.
                unsafe { std::env::set_var(name, value) };
                (*name, previous)
            })
            .collect();
        Self { previous }
    }
}

impl Drop for EnvironmentGuard {
    fn drop(&mut self) {
        for (name, value) in self.previous.drain(..) {
            match value {
                // This runs before the serial test releases the composition environment.
                Some(value) => unsafe { std::env::set_var(name, value) },
                // This runs before the serial test releases the composition environment.
                None => unsafe { std::env::remove_var(name) },
            }
        }
    }
}

fn context_with_remaining(remaining: Duration) -> Context {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default();
    let mut context = Context::default();
    context.deadline = now.saturating_add(remaining.as_millis().min(u128::from(u64::MAX)) as u64);
    context
}
