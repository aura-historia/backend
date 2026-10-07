use crate::BUSINESS_SCHEMA;

use application::transaction::{Transaction, UnitOfWork};
use aura_historia_api::{
    app,
    state::{AppState, LoopsWebhooksState},
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, Response, StatusCode, header},
};
use base64::Engine as _;
use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
use platform_postgres::SqlxUnitOfWork;
use serde_email::Email;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use tower::ServiceExt;
use user_core::user_id::UserId;
use user_loops::LoopsNewsletterWebhookVerifier;
use user_postgres::{SqlxLoopsWebhookReceiptRepository, SqlxMarketingConsentIntentRepository};
use user_service::use_cases::commands::coordinate_marketing_consent::MarketingConsentCoordinator;
use user_service::{
    ports::{
        ConsentIntent, MarketingEmailConsentError, MarketingEmailConsentOutcome,
        MarketingEmailConsentWriter, MarketingEmailSubscriptionState,
        NewsletterWebhookMailingListId,
    },
    use_cases::ApplyLoopsPreferenceEventHandler,
};

const SECRET: &str = "whsec_bG9vcHMta2V5LWN1cnJlbnQ=";
const SIGNING_KEY: &[u8] = b"loops-key-current";
const TARGET_LIST: &str = "marketing-list-id";
const PATH: &str = "/api/v1/webhooks/loops";

struct Provider {
    state: Result<MarketingEmailSubscriptionState, MarketingEmailConsentError>,
}

#[async_trait::async_trait]
impl MarketingEmailConsentWriter for Provider {
    async fn current_state(
        &self,
        _email: &Email,
    ) -> Result<MarketingEmailSubscriptionState, MarketingEmailConsentError> {
        self.state.clone()
    }

    async fn grant(
        &self,
        _intent: &ConsentIntent,
    ) -> Result<MarketingEmailConsentOutcome, MarketingEmailConsentError> {
        panic!("a webhook must not write to Loops")
    }

    async fn revoke(
        &self,
        _intent: &ConsentIntent,
    ) -> Result<MarketingEmailConsentOutcome, MarketingEmailConsentError> {
        panic!("a webhook must not write to Loops")
    }
}

async fn webhook_app(
    provider_state: Result<MarketingEmailSubscriptionState, MarketingEmailConsentError>,
) -> Router {
    let pool = get_postgres_client().await;
    let handler = ApplyLoopsPreferenceEventHandler::new(
        SqlxUnitOfWork::new(pool),
        SqlxMarketingConsentIntentRepository::new(),
        SqlxLoopsWebhookReceiptRepository::new(),
        Provider {
            state: provider_state,
        },
        NewsletterWebhookMailingListId::new(TARGET_LIST.to_owned()).expect("target list"),
    );
    app(AppState::new().with_loops_webhooks(LoopsWebhooksState::new(
        SECRET,
        Arc::new(LoopsNewsletterWebhookVerifier),
        Arc::new(handler),
    )))
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

fn signed_request(delivery_id: &str, timestamp: i64, body: Vec<u8>) -> Request<Body> {
    let key = PKey::hmac(SIGNING_KEY).expect("HMAC key");
    let mut signer = Signer::new(MessageDigest::sha256(), &key).expect("HMAC signer");
    signer.update(delivery_id.as_bytes()).expect("delivery ID");
    signer.update(b".").expect("separator");
    signer
        .update(timestamp.to_string().as_bytes())
        .expect("timestamp");
    signer.update(b".").expect("separator");
    signer.update(&body).expect("body");
    let signature = format!(
        "v1,{}",
        base64::engine::general_purpose::STANDARD.encode(signer.sign_to_vec().expect("signature"))
    );
    Request::builder()
        .method("POST")
        .uri(PATH)
        .header(header::CONTENT_TYPE, "application/json")
        .header("webhook-id", delivery_id)
        .header("webhook-timestamp", timestamp.to_string())
        .header("webhook-signature", signature)
        .body(Body::from(body))
        .expect("request")
}

fn event(event_name: &str, event_time: i64, email: &str, list: Option<&str>) -> Vec<u8> {
    let mut body = json!({
        "webhookSchemaVersion": "1.0.0",
        "eventName": event_name,
        "eventTime": event_time,
        "contactIdentity": { "id": "loops-contact-1", "email": email },
    });
    if let Some(list) = list {
        body["mailingList"] = json!({ "id": list });
    }
    body.to_string().into_bytes()
}

async fn assert_response(response: Response<Body>, status: StatusCode, code: Option<&str>) {
    assert_eq!(status, response.status());
    assert_eq!(
        "private, no-store",
        response.headers()[header::CACHE_CONTROL]
    );
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    match code {
        Some(code) => {
            let body: Value = serde_json::from_slice(&bytes).expect("problem JSON");
            assert_eq!(code, body["error"]);
            assert_eq!(u16::from(status), body["status"]);
        }
        None => assert!(bytes.is_empty()),
    }
}

async fn receipt(delivery_id: &str) -> Option<(Vec<u8>, String, String, Option<String>)> {
    sqlx::query_as(
        "SELECT raw_body_sha256, provider_event_name, disposition, email FROM loops_webhook_receipts WHERE delivery_id = $1",
    )
    .bind(delivery_id)
    .fetch_optional(&get_postgres_client().await)
    .await
    .expect("receipt query")
}

async fn seed_consented_user(pool: &sqlx::PgPool, id: uuid::Uuid, address: &str) {
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(id)
        .bind(address)
        .execute(pool)
        .await
        .expect("seed user");
    let email = Email::try_from(address).expect("email");
    let mut tx = SqlxUnitOfWork::new(pool.clone())
        .begin()
        .await
        .expect("consent transaction");
    MarketingConsentCoordinator::new(&mut tx, &SqlxMarketingConsentIntentRepository::new())
        .accepted_double_opt_in(
            format!("test-{}", uuid::Uuid::new_v4()),
            email,
            time::OffsetDateTime::now_utc() - time::Duration::hours(1),
        )
        .await
        .expect("seed accepted consent");
    tx.commit().await.expect("commit consent");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn signed_unknown_event_commits_exact_byte_receipt_and_retries_idempotently() {
    let app = webhook_app(Ok(MarketingEmailSubscriptionState::Missing)).await;
    let timestamp = now();
    let delivery_id = format!("msg_{}", uuid::Uuid::new_v4());
    let body = format!(
        " {{\n \"webhookSchemaVersion\":\"1.0.0\", \"eventName\":\"testing.testEvent\", \"eventTime\":{timestamp}, \"unused\":\"élève\" }}"
    )
    .into_bytes();
    let digest: [u8; 32] = Sha256::digest(&body).into();

    assert_response(
        app.clone()
            .oneshot(signed_request(&delivery_id, timestamp, body.clone()))
            .await
            .expect("route"),
        StatusCode::NO_CONTENT,
        None,
    )
    .await;
    let stored = receipt(&delivery_id).await.expect("committed receipt");
    assert_eq!(digest.to_vec(), stored.0);
    assert_eq!("testing.testEvent", stored.1);
    assert_eq!("IGNORED_UNSUPPORTED_EVENT", stored.2);
    assert_eq!(None, stored.3);

    assert_response(
        app.clone()
            .oneshot(signed_request(&delivery_id, timestamp, body.clone()))
            .await
            .expect("retry"),
        StatusCode::NO_CONTENT,
        None,
    )
    .await;
    assert_response(
        app.oneshot(signed_request(
            &delivery_id,
            timestamp,
            [body, b" ".to_vec()].concat(),
        ))
        .await
        .expect("conflict"),
        StatusCode::CONFLICT,
        Some("LOOPS_WEBHOOK_CONFLICT"),
    )
    .await;
    assert_eq!(
        digest.to_vec(),
        receipt(&delivery_id).await.expect("original receipt").0
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn invalid_proof_and_invalid_payload_never_commit_receipts() {
    let app = webhook_app(Ok(MarketingEmailSubscriptionState::Missing)).await;
    let timestamp = now();
    let body = event("email.unsubscribed", timestamp, "person@example.test", None);
    let mut invalid_signature = signed_request("bad-proof", timestamp, body.clone());
    invalid_signature
        .headers_mut()
        .insert("webhook-signature", "v1,AAAA".parse().unwrap());
    let cases = [
        (invalid_signature, StatusCode::UNAUTHORIZED, "LOOPS_WEBHOOK_UNAUTHORIZED", "bad-proof"),
        (signed_request("old-proof", timestamp - 301, body), StatusCode::UNAUTHORIZED, "LOOPS_WEBHOOK_UNAUTHORIZED", "old-proof"),
        (signed_request("bad-json", timestamp, b"{".to_vec()), StatusCode::BAD_REQUEST, "LOOPS_WEBHOOK_INVALID_BODY", "bad-json"),
        (signed_request("bad-schema", timestamp, br#"{"webhookSchemaVersion":"2.0.0","eventName":"testing.testEvent","eventTime":1}"#.to_vec()), StatusCode::BAD_REQUEST, "LOOPS_WEBHOOK_INVALID_BODY", "bad-schema"),
    ];
    for (request, status, code, id) in cases {
        assert_response(
            app.clone().oneshot(request).await.expect("route"),
            status,
            Some(code),
        )
        .await;
        assert!(receipt(id).await.is_none());
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn signed_withdrawal_changes_only_the_exact_users_consent_and_commits_receipt() {
    let app = webhook_app(Ok(MarketingEmailSubscriptionState::Missing)).await;
    let pool = get_postgres_client().await;
    let user_id = UserId::new().into_uuid();
    let other_id = UserId::new().into_uuid();
    let email = format!("loops-{}@example.test", user_id);
    let other_email = format!("loops-{}@example.test", other_id);
    for (id, address) in [(user_id, &email), (other_id, &other_email)] {
        seed_consented_user(&pool, id, address).await;
    }
    let timestamp = now();
    let delivery_id = format!("msg_{}", uuid::Uuid::new_v4());
    assert_response(
        app.oneshot(signed_request(
            &delivery_id,
            timestamp,
            event("email.unsubscribed", timestamp, &email, None),
        ))
        .await
        .expect("route"),
        StatusCode::NO_CONTENT,
        None,
    )
    .await;
    let (consent, revision): (bool, i64) = sqlx::query_as("SELECT marketing_email_consent, marketing_email_consent_revision FROM users WHERE user_id = $1")
        .bind(user_id).fetch_one(&pool).await.expect("user state");
    assert!(!consent);
    assert_eq!(2, revision);
    let other_consent: bool =
        sqlx::query_scalar("SELECT marketing_email_consent FROM users WHERE user_id = $1")
            .bind(other_id)
            .fetch_one(&pool)
            .await
            .expect("other user state");
    assert!(other_consent);
    let stored = receipt(&delivery_id).await.expect("committed receipt");
    assert_eq!("email.unsubscribed", stored.1);
    assert_eq!("APPLIED_WITHDRAWAL", stored.2);
    assert_eq!(Some(email), stored.3);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn unrelated_list_and_hard_bounce_are_receipted_without_withdrawing_consent() {
    let app = webhook_app(Ok(MarketingEmailSubscriptionState::Missing)).await;
    let pool = get_postgres_client().await;
    let user_id = UserId::new().into_uuid();
    let email = format!("loops-{}@example.test", user_id);
    seed_consented_user(&pool, user_id, &email).await;
    let timestamp = now();
    for (name, list, disposition) in [
        (
            "contact.mailingList.unsubscribed",
            Some("another-list"),
            "IGNORED_UNRELATED_LIST",
        ),
        ("email.hardBounced", None, "IGNORED_HARD_BOUNCE"),
    ] {
        let id = format!("msg_{}", uuid::Uuid::new_v4());
        assert_response(
            app.clone()
                .oneshot(signed_request(
                    &id,
                    timestamp,
                    event(name, timestamp, &email, list),
                ))
                .await
                .expect("route"),
            StatusCode::NO_CONTENT,
            None,
        )
        .await;
        assert_eq!(disposition, receipt(&id).await.expect("receipt").2);
    }
    let (consent, revision): (bool, i64) = sqlx::query_as("SELECT marketing_email_consent, marketing_email_consent_revision FROM users WHERE user_id = $1")
        .bind(user_id).fetch_one(&pool).await.expect("user state");
    assert!(consent);
    assert_eq!(1, revision);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn lambda_base64_event_preserves_signed_bytes_and_commits_the_receipt() {
    let app = webhook_app(Ok(MarketingEmailSubscriptionState::Missing)).await;
    let timestamp = now();
    let delivery_id = format!("msg_{}", uuid::Uuid::new_v4());
    let body = format!(
        " {{\n \"webhookSchemaVersion\":\"1.0.0\", \"eventName\":\"testing.testEvent\", \"eventTime\":{timestamp}, \"unused\":\"élève\" }}"
    )
    .into_bytes();
    let signed = signed_request(&delivery_id, timestamp, body.clone());
    let signature = signed.headers()["webhook-signature"]
        .to_str()
        .expect("signature");
    let mut event: Value =
        serde_json::from_str(include_str!("../fixtures/http_api_v2_health.json"))
            .expect("HTTP API v2 fixture");
    event["routeKey"] = "POST /api/v1/webhooks/loops".into();
    event["rawPath"] = PATH.into();
    event["requestContext"]["http"]["method"] = "POST".into();
    event["requestContext"]["http"]["path"] = PATH.into();
    event["headers"] = json!({
        "host": "api.example.test",
        "webhook-id": delivery_id,
        "webhook-timestamp": timestamp.to_string(),
        "webhook-signature": signature,
    });
    event["body"] = base64::engine::general_purpose::STANDARD
        .encode(&body)
        .into();
    event["isBase64Encoded"] = true.into();
    let request = lambda_http::request::from_str(&event.to_string()).expect("Lambda request");

    assert_response(
        aura_historia_api::lambda::handle_http_api_v2_request(app, request)
            .await
            .expect("Lambda adapter"),
        StatusCode::NO_CONTENT,
        None,
    )
    .await;
    let digest: [u8; 32] = Sha256::digest(&body).into();
    assert_eq!(
        digest.to_vec(),
        receipt(&delivery_id).await.expect("receipt").0
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn oversized_body_and_provider_read_failure_return_retryable_errors_without_receipts() {
    let timestamp = now();
    let app = webhook_app(Err(MarketingEmailConsentError::ReadUnavailable)).await;
    let oversized_id = format!("msg_{}", uuid::Uuid::new_v4());
    let oversized = signed_request(&oversized_id, timestamp, vec![b'x'; 64 * 1024 + 1]);
    assert_response(
        app.clone().oneshot(oversized).await.expect("route"),
        StatusCode::PAYLOAD_TOO_LARGE,
        Some("LOOPS_WEBHOOK_PAYLOAD_TOO_LARGE"),
    )
    .await;
    assert!(receipt(&oversized_id).await.is_none());

    let retry_id = format!("msg_{}", uuid::Uuid::new_v4());
    let email = format!("reader-{}@example.test", uuid::Uuid::new_v4());
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(UserId::new().into_uuid())
        .bind(&email)
        .execute(&get_postgres_client().await)
        .await
        .expect("seed user for provider read");
    let body = event("email.resubscribed", timestamp - 5, &email, None);
    assert_response(
        app.oneshot(signed_request(&retry_id, timestamp, body))
            .await
            .expect("route"),
        StatusCode::SERVICE_UNAVAILABLE,
        Some("LOOPS_WEBHOOK_TEMPORARILY_UNAVAILABLE"),
    )
    .await;
    assert!(receipt(&retry_id).await.is_none());
}
