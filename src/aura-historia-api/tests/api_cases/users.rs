use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};

use api_support::{assert_problem, json_response, seed_access_token_for, seed_user};

use test_api::{IntegrationTestService, aura_integration_test};
use user_core::access_token::Scope;

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_current_user_account_when_authenticated() {
    let user_id = seed_user("USER").await;
    let token =
        seed_access_token_for(user_id, std::collections::HashSet::from([Scope::UsersRead])).await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/me/account", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call user API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(user_id.to_string()), body["userId"]);
    assert_eq!(serde_json::json!("USER"), body["role"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_update_current_user_profile_when_body_is_valid() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .patch(format!("{}/api/v1/me/account", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({
            "firstName": "Ada",
            "lastName": "Lovelace",
            "language": "de",
            "currency": "EUR",
            "measurementUnit": "METRIC",
            "showUnassessedOrSensitiveContent": true
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch user API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!("Ada"), body["firstName"]);
    assert_eq!(
        serde_json::json!(true),
        body["showUnassessedOrSensitiveContent"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_delete_current_user_when_authenticated() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .delete(format!("{}/api/v1/me", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete user API: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_user_when_admin_reads_user() {
    let user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersRead]),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/users/{}", AURA_API.base_url(), user_id))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get admin user API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(user_id.to_string()), body["userId"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_search_users_when_actor_is_admin() {
    seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersRead]),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/users", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to search users API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert!(
        body["items"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_update_user_tier_when_actor_is_admin() {
    let user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .patch(format!("{}/api/v1/users/{}", AURA_API.base_url(), user_id))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"tier": "PRO"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch admin user API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(user_id.to_string()), body["userId"]);
    assert_eq!(serde_json::json!("PRO"), body["tier"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_delete_user_when_actor_is_admin() {
    let user_id = seed_user("USER").await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .delete(format!("{}/api/v1/users/{}", AURA_API.base_url(), user_id))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete admin user API: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_admin_user_read_when_actor_is_not_admin() {
    let user_id = seed_user("USER").await;
    let token =
        seed_access_token_for(user_id, std::collections::HashSet::from([Scope::UsersRead])).await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/users/{}", AURA_API.base_url(), user_id))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get forbidden user API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_admin_user_read_when_user_id_is_invalid() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::UsersRead]),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/users/not-a-uuid", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get invalid user API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_UUID",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_access_token_for_current_user() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::AccessTokensWrite, Scope::AccessTokensRead]),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/me/access-tokens", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({
            "name": "acceptance token",
            "scopes": ["product-listings:write", "watchlist:write"]
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create access token API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::CREATED, status);
    assert_eq!(serde_json::json!(user_id.to_string()), body["userId"]);
    assert!(
        body["accessToken"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_list_access_tokens_for_current_user() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::AccessTokensRead]),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/me/access-tokens", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list access token API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert!(body.as_array().is_some_and(|items| !items.is_empty()));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_get_access_token_for_current_user() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::AccessTokensRead, Scope::AccessTokensWrite]),
    )
    .await;
    let access_token_id = create_access_token(&token).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/me/access-tokens/{}",
            AURA_API.base_url(),
            access_token_id
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get access token API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!("editable token"), body["name"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_update_access_token_for_current_user() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::AccessTokensRead, Scope::AccessTokensWrite]),
    )
    .await;
    let access_token_id = create_access_token(&token).await;

    let response = reqwest::Client::new()
        .patch(format!("{}/api/v1/me/access-tokens", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({
            "accessTokenId": access_token_id,
            "name": "renamed token",
            "scopes": ["product-listings:write"]
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch access token API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!("renamed token"), body["name"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_delete_access_token_for_current_user() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::AccessTokensRead, Scope::AccessTokensWrite]),
    )
    .await;
    let access_token_id = create_access_token(&token).await;

    let response = reqwest::Client::new()
        .delete(format!(
            "{}/api/v1/me/access-tokens/{}",
            AURA_API.base_url(),
            access_token_id
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete access token API: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_access_token_read_when_id_is_invalid() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::AccessTokensRead]),
    )
    .await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/me/access-tokens/not-a-uuid",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get invalid access token API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_UUID",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_require_auth_for_access_tokens() {
    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/me/access-tokens", AURA_API.base_url()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list missing auth token API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );
}

async fn create_access_token(token: &user_core::access_token::RawAccessToken) -> String {
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/me/access-tokens", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"name": "editable token", "scopes": ["product-listings:write"]}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create access token API: {error}"));
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::CREATED, status, "{body}");
    body["accessTokenId"]
        .as_str()
        .unwrap_or_else(|| panic!("missing accessTokenId"))
        .to_owned()
}
