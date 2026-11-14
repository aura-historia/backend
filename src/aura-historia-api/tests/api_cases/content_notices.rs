use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};
use api_support::{
    assert_problem, json_response, seed_access_token_for, seed_listing_source, seed_user,
};
use listing_source_core::ListingSourceId;
use serde_json::json;
use std::collections::HashSet;
use test_api::{IntegrationTestService, aura_integration_test};
use user_core::access_token::Scope;

async fn list(token: &str) -> (reqwest::StatusCode, serde_json::Value, Option<String>) {
    admin_get(token, "?sort=created&order=desc&size=5").await
}

async fn admin_get(
    token: &str,
    path: &str,
) -> (reqwest::StatusCode, serde_json::Value, Option<String>) {
    let response = reqwest::Client::new()
        .get(format!(
            "{}/v1/admin/content-notices{path}",
            AURA_API.base_url()
        ))
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call admin content notices API: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    (status, body, cache_control)
}

async fn token(role: &'static str, scopes: HashSet<Scope>) -> String {
    String::from(seed_access_token_for(seed_user(role).await, scopes).await)
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_list_content_notices_only_for_admins_with_the_read_scope() {
    let admin = token("ADMIN", HashSet::from([Scope::ContentNoticesRead])).await;
    let (status, body, cache_control) = list(&admin).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(json!({"items": [], "size": 0}), body);

    for forbidden in [
        token("USER", HashSet::from([Scope::ContentNoticesRead])).await,
        token("ADMIN", HashSet::from([Scope::AdminOverviewRead])).await,
    ] {
        let (status, body, cache_control) = list(&forbidden).await;
        assert_eq!(Some("no-store".to_owned()), cache_control);
        assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
    }
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_return_a_submitted_notice_with_its_current_target_only_to_admins() {
    let listing_source_id = ListingSourceId::try_from(seed_listing_source().await).unwrap();
    let response = reqwest::Client::new()
        .post(format!("{}/v1/content-notices", AURA_API.base_url()))
        .json(&json!({
            "target": {"type": "LISTING_SOURCE", "listingSourceId": listing_source_id.to_string()},
            "submitterRole": "PROVIDER",
            "reasonCategory": "PROVIDER_OPT_OUT",
            "name": "Max Mustermann",
            "email": "max@antik.example",
            "language": "de",
            "goodFaithConfirmed": true
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to submit content notice: {error}"));
    assert_eq!(reqwest::StatusCode::ACCEPTED, response.status());
    let admin = token("ADMIN", HashSet::from([Scope::ContentNoticesRead])).await;
    let (_, page, _) = list(&admin).await;
    let id = page["items"][0]["contentNoticeId"]
        .as_str()
        .expect("submitted notice is listed")
        .to_owned();

    let (status, body, cache_control) = admin_get(&admin, &format!("/{id}")).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(json!(id), body["contentNoticeId"]);
    assert_eq!(
        json!(true),
        body["currentTarget"]["listingSource"]["exists"]
    );
    assert!(body.get("explanation").is_none());

    let unknown = content_notice_core::content_notice_id::ContentNoticeId::new();
    let (status, body, _) = admin_get(&admin, &format!("/{unknown}")).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "CONTENT_NOTICE_NOT_FOUND",
    );
    let user = token("USER", HashSet::from([Scope::ContentNoticesRead])).await;
    let (status, body, cache_control) = admin_get(&user, &format!("/{id}")).await;
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}
