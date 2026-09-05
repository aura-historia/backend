use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};
use api_support::{
    assert_problem, json_response, seed_access_token_for, seed_listing_source_for_search,
    seed_partnership_for_search, seed_user,
};
use serde_json::{Value, json};
use test_api::{IntegrationTestService, aura_integration_test};
use time::macros::datetime;
use uuid::Uuid;

async fn get_partnerships(
    token: &str,
    query: &[(&str, &str)],
) -> (reqwest::StatusCode, Value, Option<String>) {
    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/admin/partnerships", AURA_API.base_url()))
        .bearer_auth(token)
        .query(query)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call admin partnerships API: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    (status, body, cache_control)
}

fn assert_no_store(cache_control: Option<String>) {
    assert_eq!(Some("no-store".to_owned()), cache_control);
}

fn item_ids(body: &Value) -> Vec<Uuid> {
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("partnership response did not contain an items array"))
        .iter()
        .map(|item| {
            item["partnershipId"]
                .as_str()
                .unwrap_or_else(|| panic!("partnership item did not contain partnershipId"))
                .parse::<Uuid>()
                .unwrap_or_else(|error| panic!("partnership ID was not a UUID: {error}"))
        })
        .collect()
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_safe_admin_partnership_summary_without_cache() {
    let member_one = seed_user("USER").await;
    let member_two = seed_user("USER").await;
    let (listing_source_one, _, _) = seed_listing_source_for_search(
        "Partnership Listing Source One",
        "Partnership Operator One",
        "PARTNER_API",
        None,
    )
    .await;
    let (listing_source_two, _, _) = seed_listing_source_for_search(
        "Partnership Listing Source Two",
        "Partnership Operator Two",
        "PARTNER_API",
        None,
    )
    .await;
    let created = datetime!(2026-07-01 12:00 UTC);
    let updated = datetime!(2026-07-02 12:00 UTC);
    let (partnership_id, party_id) = seed_partnership_for_search(
        "Safe Admin Partnership",
        created,
        updated,
        &[member_one, member_two],
        &[listing_source_one, listing_source_two],
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);

    let (status, body, cache_control) = get_partnerships(&token, &[("size", "1")]).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_no_store(cache_control);
    assert_eq!(
        json!({
            "items": [{
                "partnershipId": partnership_id.to_string(),
                "party": {
                    "partyId": party_id.to_string(),
                    "partySlugId": format!("api-partnership-party-{party_id}"),
                    "name": "Safe Admin Partnership"
                },
                "memberCount": 2,
                "listingSourceGrantCount": 2,
                "created": "2026-07-01T12:00:00Z",
                "updated": "2026-07-02T12:00:00Z"
            }],
            "size": 1
        }),
        body
    );
    assert!(body.to_string().find("secret").is_none());
    assert!(body.to_string().find("token").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_filter_admin_partnerships_by_party_member_and_listing_source() {
    let matching_member = seed_user("USER").await;
    let other_member = seed_user("USER").await;
    let (matching_source, _, _) = seed_listing_source_for_search(
        "Matching Partnership Source",
        "Matching Partnership Operator",
        "PARTNER_API",
        None,
    )
    .await;
    let (other_source, _, _) = seed_listing_source_for_search(
        "Other Partnership Source",
        "Other Partnership Operator",
        "PARTNER_API",
        None,
    )
    .await;
    let (matching_partnership, matching_party) = seed_partnership_for_search(
        "Matching Admin Partnership",
        datetime!(2026-07-10 12:00 UTC),
        datetime!(2026-07-10 12:00 UTC),
        &[matching_member],
        &[matching_source],
    )
    .await;
    let (other_partnership, other_party) = seed_partnership_for_search(
        "Other Admin Partnership",
        datetime!(2026-07-09 12:00 UTC),
        datetime!(2026-07-09 12:00 UTC),
        &[other_member],
        &[other_source],
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);

    for (field, value) in [
        ("partyId", matching_party.to_string()),
        ("memberUserId", matching_member.to_string()),
        ("listingSourceId", matching_source.to_string()),
    ] {
        let (status, body, cache_control) = get_partnerships(&token, &[(field, &value)]).await;
        assert_eq!(reqwest::StatusCode::OK, status, "filter {field}");
        assert_no_store(cache_control);
        assert_eq!(
            vec![Uuid::from(matching_partnership)],
            item_ids(&body),
            "filter {field}"
        );
    }

    let party = matching_party.to_string();
    let member = matching_member.to_string();
    let source = matching_source.to_string();
    let (status, body, cache_control) = get_partnerships(
        &token,
        &[
            ("partyId", &party),
            ("memberUserId", &member),
            ("listingSourceId", &source),
        ],
    )
    .await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_no_store(cache_control);
    assert_eq!(vec![Uuid::from(matching_partnership)], item_ids(&body));
    assert!(!item_ids(&body).contains(&Uuid::from(other_partnership)));
    assert_ne!(matching_party, other_party);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_follow_admin_partnership_cursor_with_uuid_tie_breaking() {
    let timestamp = datetime!(2026-07-20 12:00 UTC);
    let first =
        seed_partnership_for_search("Cursor Partnership One", timestamp, timestamp, &[], &[]).await;
    let second =
        seed_partnership_for_search("Cursor Partnership Two", timestamp, timestamp, &[], &[]).await;
    let third =
        seed_partnership_for_search("Cursor Partnership Three", timestamp, timestamp, &[], &[])
            .await;
    let mut expected = [
        Uuid::from(first.0),
        Uuid::from(second.0),
        Uuid::from(third.0),
    ];
    expected.sort_by(|left, right| right.cmp(left));

    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);

    let (status, first_body, first_cache_control) =
        get_partnerships(&token, &[("size", "2")]).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_no_store(first_cache_control);
    assert_eq!(json!(2), first_body["size"]);
    assert_eq!(expected[..2], item_ids(&first_body)[..]);
    let cursor = serde_json::to_string(&first_body["searchAfter"])
        .unwrap_or_else(|error| panic!("failed to serialize partnership cursor: {error}"));

    let (status, second_body, second_cache_control) =
        get_partnerships(&token, &[("size", "2"), ("searchAfter", &cursor)]).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_no_store(second_cache_control);
    assert_eq!(json!(2), second_body["size"]);
    assert_eq!(vec![expected[2]], item_ids(&second_body));
    assert!(second_body.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_empty_admin_partnership_collection_with_default_size() {
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let missing_party = Uuid::new_v4().to_string();

    let (status, body, cache_control) =
        get_partnerships(&token, &[("partyId", &missing_party)]).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_no_store(cache_control);
    assert_eq!(Some(0), body["items"].as_array().map(Vec::len));
    assert_eq!(json!(21), body["size"]);
    assert!(body.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_admin_partnership_query_values_with_field_errors() {
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let invalid_queries = [
        ("partyId", "not-a-uuid"),
        ("memberUserId", "not-a-uuid"),
        ("listingSourceId", "not-a-uuid"),
        ("size", "not-a-number"),
        ("searchAfter", "not-json"),
        ("searchAfter", r#"{"timestamp":"2026-07-20T12:00:00Z"}"#),
        (
            "searchAfter",
            r#"["not-a-timestamp","550e8400-e29b-41d4-a716-446655440000"]"#,
        ),
        ("searchAfter", r#"["2026-07-20T12:00:00Z","not-a-uuid"]"#),
    ];

    for (field, value) in invalid_queries {
        let (status, body, cache_control) = get_partnerships(&token, &[(field, value)]).await;
        assert_no_store(cache_control);
        assert_problem(
            status,
            &body,
            reqwest::StatusCode::BAD_REQUEST,
            "BAD_QUERY_PARAMETER_VALUE",
        );
        assert_eq!(json!({"field": field, "type": "QUERY"}), body["source"]);
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_non_admin_admin_partnership_collection_access() {
    let user_id = seed_user("USER").await;
    let token =
        String::from(seed_access_token_for(user_id, std::collections::HashSet::new()).await);

    let (status, body, cache_control) = get_partnerships(&token, &[]).await;

    assert_no_store(cache_control);
    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}
