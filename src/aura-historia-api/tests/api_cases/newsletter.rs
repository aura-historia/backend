use crate::{AURA_API, BUSINESS_SCHEMA, NEWSLETTER_C09_API, OPENSEARCH, api_support};

use api_support::{assert_problem, seed_access_token_for, seed_user};
use test_api::{IntegrationTestService, aura_integration_test};
use user_core::access_token::Scope;
use user_core::newsletter_confirmation::RawNewsletterConfirmationToken;

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_request_newsletter_confirmation_anonymously() {
    let response = reqwest::Client::new()
        .put(format!(
            "{}/api/v1/newsletter-subscriptions",
            AURA_API.base_url()
        ))
        .json(&serde_json::json!({
            "email": "collector@example.com",
            "firstName": "Ada",
            "lastName": "Lovelace",
            "language": "en",
            "currency": "EUR"
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call newsletter API: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
    assert_eq!(
        Some("no-store"),
        response
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok())
    );
    assert!(response.bytes().await.unwrap_or_default().is_empty());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_request_newsletter_confirmation_with_an_optional_bearer() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let account_email = format!("{}@example.test", user_id.as_uuid());
    for email in [account_email.as_str(), "alternate@example.com"] {
        let response = reqwest::Client::new()
            .put(format!(
                "{}/api/v1/newsletter-subscriptions",
                AURA_API.base_url()
            ))
            .bearer_auth(String::from(token.clone()))
            .json(&serde_json::json!({ "email": email }))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to call newsletter API: {error}"));

        assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get("cache-control")
                .and_then(|value| value.to_str().ok())
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_route_anonymous_newsletter_confirmation_without_authentication() {
    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/newsletter-subscriptions/confirm",
            AURA_API.base_url()
        ))
        .json(&serde_json::json!({ "token": "opaque-confirmation-token" }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call newsletter API: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
    assert_eq!(
        Some("no-store"),
        response
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok())
    );
    assert!(response.bytes().await.unwrap_or_default().is_empty());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_newsletter_request_body() {
    let response = reqwest::Client::new()
        .put(format!(
            "{}/api/v1/newsletter-subscriptions",
            AURA_API.base_url()
        ))
        .body("not-json")
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call newsletter API: {error}"));
    let status = response.status();
    let body = response
        .json::<serde_json::Value>()
        .await
        .unwrap_or_else(|error| panic!("failed to decode newsletter error: {error}"));

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_BODY_VALUE",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_map_transport_oversized_newsletter_body_to_bad_body_value() {
    for (method, path) in [
        (reqwest::Method::PUT, "/api/v1/newsletter-subscriptions"),
        (
            reqwest::Method::POST,
            "/api/v1/newsletter-subscriptions/confirm",
        ),
    ] {
        let response = reqwest::Client::new()
            .request(method, format!("{}{}", AURA_API.base_url(), path))
            .header("content-type", "application/json")
            .body("x".repeat(1_048_577))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to call newsletter API: {error}"));
        let status = response.status();
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get("cache-control")
                .and_then(|value| value.to_str().ok())
        );
        let body = response
            .json::<serde_json::Value>()
            .await
            .unwrap_or_else(|error| panic!("failed to decode newsletter error: {error}"));

        assert_problem(
            status,
            &body,
            reqwest::StatusCode::BAD_REQUEST,
            "BAD_BODY_VALUE",
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, &NEWSLETTER_C09_API])]
async fn should_commit_and_replay_real_newsletter_confirmation_and_reject_invalid_proofs() {
    use sqlx::Row;

    let user_id = seed_user("USER").await;
    let email = format!("{}@example.test", user_id.as_uuid());
    let token = request_real_confirmation(&email).await;

    let first = post_real_confirmation(&token).await;
    assert_eq!(reqwest::StatusCode::NO_CONTENT, first.status());
    assert!(first.bytes().await.unwrap_or_default().is_empty());

    let replay = post_real_confirmation(&token).await;
    assert_eq!(reqwest::StatusCode::NO_CONTENT, replay.status());
    assert!(replay.bytes().await.unwrap_or_default().is_empty());

    let pool = test_api::get_postgres_client().await;
    let user = sqlx::query(
        "SELECT marketing_email_consent, marketing_email_consent_revision FROM users WHERE user_id = $1",
    )
    .bind(user_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to read confirmed newsletter user: {error}"));
    assert!(user.get::<bool, _>("marketing_email_consent"));
    assert_eq!(
        1_i64,
        user.get::<i64, _>("marketing_email_consent_revision")
    );

    let proof = sqlx::query(
        "SELECT c.confirmed_at, c.resulting_intent_id, i.user_id, i.desired, i.source FROM newsletter_subscription_confirmations c JOIN marketing_email_consent_sync_intents i ON i.intent_id = c.resulting_intent_id WHERE c.email = $1",
    )
    .bind(&email)
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to read confirmed newsletter proof and intent: {error}"));
    assert!(
        proof
            .get::<Option<time::OffsetDateTime>, _>("confirmed_at")
            .is_some()
    );
    assert_eq!(
        Some(*user_id.as_uuid()),
        proof.get::<Option<uuid::Uuid>, _>("user_id")
    );
    assert!(proof.get::<bool, _>("desired"));
    assert_eq!("AURA_DOUBLE_OPT_IN", proof.get::<String, _>("source"));
    let intent_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE email = $1 AND source = 'AURA_DOUBLE_OPT_IN'",
    )
    .bind(&email)
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to count confirmation intents: {error}"));
    assert_eq!(1, intent_count);

    let unknown = RawNewsletterConfirmationToken::from_entropy([0xA7; 32]);
    let invalid = post_real_confirmation(unknown.as_str()).await;
    assert_eq!(reqwest::StatusCode::BAD_REQUEST, invalid.status());
    let invalid_body = invalid
        .json::<serde_json::Value>()
        .await
        .unwrap_or_else(|error| panic!("failed to decode invalid proof error: {error}"));
    assert_eq!("NEWSLETTER_CONFIRMATION_INVALID", invalid_body["error"]);
    assert!(!invalid_body.to_string().contains(unknown.as_str()));

    for (suffix, invalidate) in [
        (
            "expired",
            "UPDATE newsletter_subscription_confirmations SET created_at = now() - interval '26 hours', expires_at = now() - interval '2 hours' WHERE email = $1",
        ),
        (
            "revoked",
            "UPDATE newsletter_subscription_confirmations SET invalidated_at = now() WHERE email = $1",
        ),
    ] {
        let expired_or_revoked_email =
            format!("newsletter-{suffix}-{}@example.test", uuid::Uuid::new_v4());
        let expired_or_revoked_token = request_real_confirmation(&expired_or_revoked_email).await;
        sqlx::query(invalidate)
            .bind(&expired_or_revoked_email)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("failed to prepare {suffix} proof: {error}"));
        let response = post_real_confirmation(&expired_or_revoked_token).await;
        assert_eq!(reqwest::StatusCode::BAD_REQUEST, response.status());
        let body = response
            .json::<serde_json::Value>()
            .await
            .unwrap_or_else(|error| panic!("failed to decode {suffix} proof error: {error}"));
        assert_eq!("NEWSLETTER_CONFIRMATION_INVALID", body["error"]);
        assert!(!body.to_string().contains(&expired_or_revoked_token));
        let intents: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE email = $1",
        )
        .bind(&expired_or_revoked_email)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to count {suffix} proof intents: {error}"));
        assert_eq!(0, intents);
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, &NEWSLETTER_C09_API])]
async fn should_roll_back_newsletter_consent_and_intent_when_proof_completion_fails() {
    use sqlx::Row;

    let user_id = seed_user("USER").await;
    let email = format!("{}@example.test", user_id.as_uuid());
    let token = request_real_confirmation(&email).await;
    let pool = test_api::get_postgres_client().await;
    let confirmation_id: uuid::Uuid = sqlx::query_scalar(
        "SELECT confirmation_id FROM newsletter_subscription_confirmations WHERE email = $1",
    )
    .bind(&email)
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to load newsletter proof: {error}"));

    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE OR REPLACE FUNCTION test_fail_newsletter_confirmation_completion() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.confirmation_id = '{confirmation_id}'::uuid AND NEW.confirmed_at IS NOT NULL THEN RAISE EXCEPTION 'injected confirmation completion failure'; END IF; RETURN NEW; END $$"
    )))
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to install newsletter rollback trigger function: {error}"));
    sqlx::query(
        "CREATE TRIGGER test_fail_newsletter_confirmation_completion BEFORE UPDATE ON newsletter_subscription_confirmations FOR EACH ROW EXECUTE FUNCTION test_fail_newsletter_confirmation_completion()",
    )
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to install newsletter rollback trigger: {error}"));

    let response = post_real_confirmation(&token).await;
    sqlx::query("DROP TRIGGER test_fail_newsletter_confirmation_completion ON newsletter_subscription_confirmations")
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to remove newsletter rollback trigger: {error}"));
    sqlx::query("DROP FUNCTION test_fail_newsletter_confirmation_completion()")
        .execute(&pool)
        .await
        .unwrap_or_else(|error| {
            panic!("failed to remove newsletter rollback trigger function: {error}")
        });

    assert_eq!(reqwest::StatusCode::SERVICE_UNAVAILABLE, response.status());
    let body = response
        .json::<serde_json::Value>()
        .await
        .unwrap_or_else(|error| panic!("failed to decode rollback error: {error}"));
    assert_eq!("NEWSLETTER_TEMPORARILY_UNAVAILABLE", body["error"]);
    assert!(!body.to_string().contains(&token));

    let user = sqlx::query(
        "SELECT marketing_email_consent, marketing_email_consent_revision FROM users WHERE user_id = $1",
    )
    .bind(user_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to verify rolled back newsletter user: {error}"));
    assert!(!user.get::<bool, _>("marketing_email_consent"));
    assert_eq!(
        0_i64,
        user.get::<i64, _>("marketing_email_consent_revision")
    );
    let proof = sqlx::query(
        "SELECT confirmed_at, resulting_intent_id FROM newsletter_subscription_confirmations WHERE confirmation_id = $1",
    )
    .bind(confirmation_id)
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to verify rolled back proof: {error}"));
    assert!(
        proof
            .get::<Option<time::OffsetDateTime>, _>("confirmed_at")
            .is_none()
    );
    assert!(
        proof
            .get::<Option<uuid::Uuid>, _>("resulting_intent_id")
            .is_none()
    );
    let intent_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM marketing_email_consent_sync_intents WHERE email = $1 AND source = 'AURA_DOUBLE_OPT_IN'",
    )
    .bind(&email)
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to verify rolled back intent: {error}"));
    assert_eq!(0, intent_count);
}

async fn request_real_confirmation(email: &str) -> String {
    let response = reqwest::Client::new()
        .put(format!(
            "{}/api/v1/newsletter-subscriptions",
            NEWSLETTER_C09_API.base_url()
        ))
        .json(&serde_json::json!({ "email": email }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to request real newsletter confirmation: {error}"));
    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
    api_support::take_newsletter_confirmation_token(email)
}

async fn post_real_confirmation(token: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!(
            "{}/api/v1/newsletter-subscriptions/confirm",
            NEWSLETTER_C09_API.base_url()
        ))
        .json(&serde_json::json!({ "token": token }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to confirm newsletter proof through HTTP: {error}"))
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_supplied_newsletter_bearer_token() {
    let response = reqwest::Client::new()
        .put(format!(
            "{}/api/v1/newsletter-subscriptions",
            AURA_API.base_url()
        ))
        .bearer_auth("invalid-token")
        .json(&serde_json::json!({ "email": "collector@example.com" }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call newsletter API: {error}"));
    let status = response.status();
    let body = response
        .json::<serde_json::Value>()
        .await
        .unwrap_or_else(|error| panic!("failed to decode newsletter error: {error}"));

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );
}
