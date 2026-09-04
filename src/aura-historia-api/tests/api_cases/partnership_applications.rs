use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};
use api_support::{
    assert_problem, json_response, seed_access_token_for, seed_partnership_application, seed_user,
};
use serde_json::json;
use test_api::{IntegrationTestService, aura_integration_test};
use time::macros::datetime;
use uuid::Uuid;

fn existing_proposal(listing_source_id: Uuid) -> serde_json::Value {
    json!({
        "type": "EXISTING_LISTING_SOURCE",
        "listing_source_id": listing_source_id,
    })
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_list_filtered_admin_partnership_application_summaries_without_secrets() {
    let applicant_user_id = seed_user("USER").await;
    let other_applicant_user_id = seed_user("USER").await;
    let listing_source_id = Uuid::new_v4();
    let matching_id = seed_partnership_application(
        applicant_user_id,
        "SUBMITTED",
        existing_proposal(listing_source_id),
        datetime!(2026-01-02 12:00 UTC),
        datetime!(2026-02-03 12:00 UTC),
    )
    .await;
    let _other_id = seed_partnership_application(
        other_applicant_user_id,
        "IN_REVIEW",
        existing_proposal(Uuid::new_v4()),
        datetime!(2026-03-02 12:00 UTC),
        datetime!(2026-04-03 12:00 UTC),
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let applicant_user_id = applicant_user_id.to_string();
    let listing_source_id = listing_source_id.to_string();

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/partnership-applications",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .query(&[
            ("state", "SUBMITTED"),
            ("proposalType", "EXISTING_LISTING_SOURCE"),
            ("applicantUserId", applicant_user_id.as_str()),
            ("listingSourceId", listing_source_id.as_str()),
            ("created[min]", "2026-01-01T00:00:00Z"),
            ("created[max]", "2026-01-31T23:59:59Z"),
            ("updated[min]", "2026-02-01T00:00:00Z"),
            ("updated[max]", "2026-02-28T23:59:59Z"),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list partnership applications: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(Some(1), body["items"].as_array().map(Vec::len));
    assert_eq!(json!(matching_id.to_string()), body["items"][0]["id"]);
    assert_eq!(
        json!(applicant_user_id),
        body["items"][0]["applicantUserId"]
    );
    assert_eq!(json!("SUBMITTED"), body["items"][0]["state"]);
    assert_eq!(
        json!("EXISTING_LISTING_SOURCE"),
        body["items"][0]["proposal"]["type"]
    );
    assert_eq!(
        json!(listing_source_id),
        body["items"][0]["proposal"]["listingSourceId"]
    );
    assert!(body["items"][0].get("version").is_none());
    assert!(body["items"][0]["created"].as_str().is_some());
    assert!(body["items"][0]["updated"].as_str().is_some());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_follow_admin_partnership_application_cursor_with_tied_timestamps() {
    let applicant_user_id = seed_user("USER").await;
    let created = datetime!(2026-05-05 12:00 UTC);
    let ids = vec![
        seed_partnership_application(
            applicant_user_id,
            "SUBMITTED",
            existing_proposal(Uuid::new_v4()),
            created,
            created,
        )
        .await,
        seed_partnership_application(
            applicant_user_id,
            "SUBMITTED",
            existing_proposal(Uuid::new_v4()),
            created,
            created,
        )
        .await,
        seed_partnership_application(
            applicant_user_id,
            "SUBMITTED",
            existing_proposal(Uuid::new_v4()),
            created,
            created,
        )
        .await,
    ];
    let mut expected_ids = ids.clone();
    expected_ids.sort_by(|left, right| right.cmp(left));
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let client = reqwest::Client::new();
    let path = format!(
        "{}/api/v1/admin/partnership-applications",
        AURA_API.base_url()
    );

    let first = client
        .get(&path)
        .bearer_auth(String::from(token.clone()))
        .query(&[("size", "2"), ("sort", "created"), ("order", "desc")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get first application page: {error}"));
    let (first_status, first_body) = json_response(first).await;

    assert_eq!(reqwest::StatusCode::OK, first_status);
    assert_eq!(Some(2), first_body["items"].as_array().map(Vec::len));
    assert_eq!(
        json!(expected_ids[0].to_string()),
        first_body["items"][0]["id"]
    );
    assert_eq!(
        json!(expected_ids[1].to_string()),
        first_body["items"][1]["id"]
    );
    assert!(first_body["searchAfter"].is_array());
    let cursor = first_body["searchAfter"].to_string();

    let second = client
        .get(&path)
        .bearer_auth(String::from(token))
        .query(&[
            ("size", "2"),
            ("sort", "created"),
            ("order", "desc"),
            ("searchAfter", cursor.as_str()),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get second application page: {error}"));
    let (second_status, second_body) = json_response(second).await;

    assert_eq!(reqwest::StatusCode::OK, second_status);
    assert_eq!(Some(1), second_body["items"].as_array().map(Vec::len));
    assert_eq!(
        json!(expected_ids[2].to_string()),
        second_body["items"][0]["id"]
    );
    assert!(second_body.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_empty_admin_partnership_application_collection() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/partnership-applications",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .query(&[("applicantUserId", Uuid::new_v4().to_string())])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get empty application collection: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(Some(0), body["items"].as_array().map(Vec::len));
    assert!(body.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_queries_and_non_admin_collection_access() {
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let client = reqwest::Client::new();
    let path = format!(
        "{}/api/v1/admin/partnership-applications",
        AURA_API.base_url()
    );

    let invalid = client
        .get(&path)
        .bearer_auth(String::from(admin_token))
        .query(&[("state", "invalid")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to validate application query: {error}"));
    let (status, body) = json_response(invalid).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_QUERY_PARAMETER_VALUE",
    );

    let user_id = seed_user("USER").await;
    let user_token = seed_access_token_for(user_id, std::collections::HashSet::new()).await;
    let response = client
        .get(&path)
        .bearer_auth(String::from(user_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin application access: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}
