mod api_support;

use api_support::{assert_problem, seed_access_token_for, seed_user};
use test_api::{
    AuraHistoriaApi, DynamoDB, IntegrationTestService, Postgres, aura_integration_test,
};
use user_core::access_token::Scope;

const BUSINESS_SCHEMA: Postgres = Postgres::new_schema_once("migrations");
const DYNAMODB: DynamoDB = DynamoDB();
static AURA_API: AuraHistoriaApi = AuraHistoriaApi::new(api_support::aura_api_app);

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_subscribe_to_newsletter_anonymously() {
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
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_subscribe_to_newsletter_with_aura_access_token() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .put(format!(
            "{}/api/v1/newsletter-subscriptions",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({ "email": "member@example.com" }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call newsletter API: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
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

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
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
