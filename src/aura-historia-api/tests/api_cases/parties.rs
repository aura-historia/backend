use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};

use api_support::{assert_problem, json_response, seed_access_token_for, seed_party, seed_user};
use test_api::{IntegrationTestService, aura_integration_test};

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_party_summary_for_admin_with_no_store_cache_control() {
    let party_id = seed_party(
        "Admin Party",
        Some("+49 30 123456"),
        Some("admin-party@example.test"),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .query(&[("name", "Admin Party")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to search parties API: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(
        serde_json::json!(party_id.to_string()),
        body["items"][0]["partyId"]
    );
    assert_eq!(
        serde_json::json!(format!("api-acceptance-party-{party_id}")),
        body["items"][0]["partySlugId"]
    );
    assert_eq!(serde_json::json!("Admin Party"), body["items"][0]["name"]);
    assert_eq!(
        serde_json::json!("+49 30 123456"),
        body["items"][0]["contact"]["phone"]
    );
    assert_eq!(
        serde_json::json!("admin-party@example.test"),
        body["items"][0]["contact"]["email"]
    );
    assert!(body["items"][0]["created"].as_str().is_some());
    assert!(body["items"][0]["updated"].as_str().is_some());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_filter_parties_by_name_and_contact_query() {
    let matching_id = seed_party(
        "Selector Party",
        Some("+49 30 555000"),
        Some("selector@example.test"),
    )
    .await;
    let _other_id = seed_party(
        "Different Party",
        Some("+49 30 111000"),
        Some("other@example.test"),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .query(&[("query", "selector")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to filter parties API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some(1), body["items"].as_array().map(Vec::len));
    assert_eq!(
        serde_json::json!(matching_id.to_string()),
        body["items"][0]["partyId"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_follow_party_cursor_with_deterministic_sorting() {
    let first_id = seed_party("Cursor Party A", None, None).await;
    let second_id = seed_party("Cursor Party B", None, None).await;
    let third_id = seed_party("Cursor Party C", None, None).await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let client = reqwest::Client::new();

    let first = client
        .get(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .query(&[
            ("name", "Cursor Party"),
            ("sort", "name"),
            ("order", "asc"),
            ("size", "2"),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get first party page: {error}"));
    let (first_status, first_body) = json_response(first).await;

    assert_eq!(reqwest::StatusCode::OK, first_status);
    assert_eq!(Some(2), first_body["items"].as_array().map(Vec::len));
    assert_eq!(
        serde_json::json!(first_id.to_string()),
        first_body["items"][0]["partyId"]
    );
    assert_eq!(
        serde_json::json!(second_id.to_string()),
        first_body["items"][1]["partyId"]
    );
    let cursor = first_body["searchAfter"]
        .as_str()
        .unwrap_or_else(|| panic!("missing party search cursor"))
        .to_owned();

    let second = client
        .get(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .query(&[
            ("name", "Cursor Party"),
            ("sort", "name"),
            ("order", "asc"),
            ("size", "2"),
            ("searchAfter", cursor.as_str()),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get second party page: {error}"));
    let (second_status, second_body) = json_response(second).await;

    assert_eq!(reqwest::StatusCode::OK, second_status);
    assert_eq!(Some(1), second_body["items"].as_array().map(Vec::len));
    assert_eq!(
        serde_json::json!(third_id.to_string()),
        second_body["items"][0]["partyId"]
    );
    assert!(second_body.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_empty_party_collection_when_no_party_matches() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .query(&[("email", "missing-party@example.test")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to search empty party collection: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some(0), body["items"].as_array().map(Vec::len));
    assert!(body.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_party_search_query_values() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let client = reqwest::Client::new();

    let invalid_sort = client
        .get(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .query(&[("sort", "invalid"), ("order", "asc")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to validate party sort: {error}"));
    let (status, body) = json_response(invalid_sort).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_SORT_VALUE",
    );

    let invalid_size = client
        .get(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .query(&[("size", "not-a-number")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to validate party size: {error}"));
    let (status, body) = json_response(invalid_size).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_QUERY_PARAMETER_VALUE",
    );

    let invalid_cursor = client
        .get(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .query(&[("searchAfter", "not-a-uuid")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to validate party cursor: {error}"));
    let (status, body) = json_response(invalid_cursor).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_QUERY_PARAMETER_VALUE",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_party_collection_for_non_admin() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(user_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/admin/parties", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin party search: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}
