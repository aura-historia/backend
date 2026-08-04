mod api_support;

use api_support::{assert_problem, json_response, seed_access_token_for, seed_product, seed_user};
use common::product_id::ProductId;
use test_api::{
    AuraHistoriaApi, DynamoDB, IntegrationTestService, Postgres, aura_integration_test,
};
use user_core::access_token::Scope;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const DYNAMODB: DynamoDB = DynamoDB();
static AURA_API: AuraHistoriaApi = AuraHistoriaApi::new(api_support::aura_api_app);

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_add_product_to_watchlist_when_authenticated() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_id = seed_product().await;

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({
            "productId": product_id.to_string(),
            "notifications": true
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create watchlist API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::CREATED, status);
    assert_eq!(serde_json::json!(product_id.to_string()), body["productId"]);
    assert_eq!(serde_json::json!(true), body["notifications"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_reject_watchlist_create_when_entry_exists() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_id = seed_product().await;
    let client = reqwest::Client::new();

    let created = client
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"productId": product_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create watchlist API: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, created.status());

    let duplicate = client
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"productId": product_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to duplicate watchlist API: {error}"));
    let (status, body) = json_response(duplicate).await;

    assert_problem(status, &body, reqwest::StatusCode::CONFLICT, "CONFLICT");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_list_current_user_watchlist() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistRead, Scope::WatchlistWrite]),
    )
    .await;
    let product_id = seed_product().await;
    let client = reqwest::Client::new();
    let created = client
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"productId": product_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create watchlist API: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, created.status());

    let response = client
        .get(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list watchlist API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(21), body["size"]);
    assert_eq!(
        serde_json::json!(product_id.to_string()),
        body["items"][0]["productId"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_update_watchlist_notifications() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_id = seed_product().await;
    let client = reqwest::Client::new();
    let created = client
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"productId": product_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create watchlist API: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, created.status());

    let response = client
        .patch(format!(
            "{}/api/v1/me/watchlist/{}",
            AURA_API.base_url(),
            product_id
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"notifications": false}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch watchlist API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(false), body["notifications"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_delete_watchlist_entry() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_id = seed_product().await;
    let client = reqwest::Client::new();
    let created = client
        .post(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"productId": product_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create watchlist API: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, created.status());

    let response = client
        .delete(format!(
            "{}/api/v1/me/watchlist/{}",
            AURA_API.base_url(),
            product_id
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete watchlist API: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_reject_watchlist_update_when_product_id_is_invalid() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/me/watchlist/not-a-uuid",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"notifications": true}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch invalid watchlist API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_UUID",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_return_not_found_when_updating_missing_watchlist_entry() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/api/v1/me/watchlist/{}",
            AURA_API.base_url(),
            ProductId::new()
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"notifications": true}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch missing watchlist API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "WATCHLIST_ENTRY_NOT_FOUND",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_require_auth_for_watchlist() {
    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/me/watchlist", AURA_API.base_url()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list watchlist missing auth API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );
}
