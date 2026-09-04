use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};

use api_support::{
    assert_problem, json_response, seed_access_token_for, seed_listing_source_for_search,
    seed_party, seed_user,
};
use serde_json::json;
use test_api::{IntegrationTestService, aura_integration_test};

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_safe_listing_source_summary_for_admin_with_no_store_cache_control() {
    let (listing_source_id, operator_party_id, listing_source_slug_id) =
        seed_listing_source_for_search(
            "Referral Listing Source",
            "Referral Operator",
            "WOOCOMMERCE",
            Some(json!({"kind": "PARTNERIZE", "camref": "campaign123"})),
        )
        .await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/listing-sources",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .query(&[("listingSourceId", listing_source_id.to_string())])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to search listing sources API: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(Some(1), body["items"].as_array().map(Vec::len));
    assert_eq!(
        json!(listing_source_id.to_string()),
        body["items"][0]["listingSourceId"]
    );
    assert_eq!(
        json!(listing_source_slug_id),
        body["items"][0]["listingSourceSlugId"]
    );
    assert_eq!(json!("Referral Listing Source"), body["items"][0]["name"]);
    assert_eq!(
        json!(operator_party_id.to_string()),
        body["items"][0]["operator"]["partyId"]
    );
    assert_eq!(
        json!(format!("api-search-party-{operator_party_id}")),
        body["items"][0]["operator"]["partySlugId"]
    );
    assert_eq!(
        json!("Referral Operator"),
        body["items"][0]["operator"]["name"]
    );
    assert_eq!(json!(["WOOCOMMERCE"]), body["items"][0]["ingestionMethods"]);
    assert_eq!(
        json!("https://listing-source-search.example/"),
        body["items"][0]["presentation"]["url"]
    );
    assert_eq!(
        json!("https://listing-source-search.example/image.jpg"),
        body["items"][0]["presentation"]["image"]
    );
    assert_eq!(
        json!({"type": "PARTNERIZE", "camref": "campaign123"}),
        body["items"][0]["referralConfiguration"]
    );
    assert_eq!(json!(21), body["size"]);
    assert!(body.get("searchAfter").is_none());

    let serialized = body.to_string();
    assert!(!serialized.contains("webhookSecret"));
    assert!(!serialized.contains("crawler"));
    assert!(!serialized.contains("provider-secret"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_filter_listing_sources_by_text_name_operator_method_and_exact_identity() {
    let (matching_id, matching_operator_id, matching_slug) = seed_listing_source_for_search(
        "Filtered Listing Source",
        "Matching Operator",
        "SHOPIFY",
        None,
    )
    .await;
    let (other_id, other_operator_id, _) = seed_listing_source_for_search(
        "Different Listing Source",
        "Other Operator",
        "PARTNER_API",
        None,
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/admin/listing-sources", AURA_API.base_url());

    for (field, value) in [
        ("query", "Filtered".to_owned()),
        ("name", "Filtered Listing".to_owned()),
        ("operatorPartyId", matching_operator_id.to_string()),
        ("ingestionMethod", "SHOPIFY".to_owned()),
        ("listingSourceId", matching_id.to_string()),
        ("listingSourceSlugId", matching_slug.clone()),
    ] {
        let response = client
            .get(&url)
            .bearer_auth(token.clone())
            .query(&[(field, value)])
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to filter listing sources by {field}: {error}"));
        let (status, body) = json_response(response).await;

        assert_eq!(reqwest::StatusCode::OK, status, "filter {field}");
        assert_eq!(
            Some(1),
            body["items"].as_array().map(Vec::len),
            "filter {field}"
        );
        assert_eq!(
            json!(matching_id.to_string()),
            body["items"][0]["listingSourceId"],
            "filter {field}"
        );
    }

    let response = client
        .get(&url)
        .bearer_auth(token)
        .query(&[("operatorPartyId", other_operator_id.to_string())])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to check nonmatching operator filter: {error}"));
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some(1), body["items"].as_array().map(Vec::len));
    assert_eq!(
        json!(other_id.to_string()),
        body["items"][0]["listingSourceId"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_follow_listing_source_cursor_with_deterministic_sorting() {
    let (first_id, _, _) = seed_listing_source_for_search(
        "Cursor Listing Source A",
        "Cursor Operator A",
        "PARTNER_API",
        None,
    )
    .await;
    let (second_id, _, _) = seed_listing_source_for_search(
        "Cursor Listing Source B",
        "Cursor Operator B",
        "PARTNER_API",
        None,
    )
    .await;
    let (third_id, _, _) = seed_listing_source_for_search(
        "Cursor Listing Source C",
        "Cursor Operator C",
        "PARTNER_API",
        None,
    )
    .await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/admin/listing-sources", AURA_API.base_url());

    let first = client
        .get(&url)
        .bearer_auth(token.clone())
        .query(&[
            ("name", "Cursor Listing Source"),
            ("sort", "name"),
            ("order", "asc"),
            ("size", "2"),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get first listing-source page: {error}"));
    let (first_status, first_body) = json_response(first).await;

    assert_eq!(reqwest::StatusCode::OK, first_status);
    assert_eq!(Some(2), first_body["items"].as_array().map(Vec::len));
    assert_eq!(
        json!(first_id.to_string()),
        first_body["items"][0]["listingSourceId"]
    );
    assert_eq!(
        json!(second_id.to_string()),
        first_body["items"][1]["listingSourceId"]
    );
    let cursor = first_body["searchAfter"]
        .as_str()
        .unwrap_or_else(|| panic!("missing listing-source search cursor"))
        .to_owned();

    let second = client
        .get(&url)
        .bearer_auth(token)
        .query(&[
            ("name", "Cursor Listing Source"),
            ("sort", "name"),
            ("order", "asc"),
            ("size", "2"),
            ("searchAfter", cursor.as_str()),
        ])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get second listing-source page: {error}"));
    let (second_status, second_body) = json_response(second).await;

    assert_eq!(reqwest::StatusCode::OK, second_status);
    assert_eq!(Some(1), second_body["items"].as_array().map(Vec::len));
    assert_eq!(
        json!(third_id.to_string()),
        second_body["items"][0]["listingSourceId"]
    );
    assert!(second_body.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_empty_listing_source_collection_when_no_source_matches() {
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/listing-sources",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .query(&[("name", "missing-listing-source")])
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to search empty listing-source collection: {error}")
        });
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some(0), body["items"].as_array().map(Vec::len));
    assert!(body.get("searchAfter").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_listing_source_search_query_values() {
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/admin/listing-sources", AURA_API.base_url());

    let invalid_sort = client
        .get(&url)
        .bearer_auth(token.clone())
        .query(&[("sort", "invalid"), ("order", "asc")])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to validate listing-source sort: {error}"));
    let (status, body) = json_response(invalid_sort).await;
    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_SORT_VALUE",
    );

    for (field, value) in [
        ("size", "not-a-number"),
        ("searchAfter", "not-a-uuid"),
        ("listingSourceId", "not-a-uuid"),
        ("listingSourceSlugId", "Not-A-Slug"),
        ("operatorPartyId", "not-a-uuid"),
        ("ingestionMethod", "UNKNOWN"),
    ] {
        let response = client
            .get(&url)
            .bearer_auth(token.clone())
            .query(&[(field, value)])
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to validate listing-source {field}: {error}"));
        let (status, body) = json_response(response).await;
        assert_problem(
            status,
            &body,
            reqwest::StatusCode::BAD_REQUEST,
            "BAD_QUERY_PARAMETER_VALUE",
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_listing_source_collection_for_non_admin() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(user_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/listing-sources",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to reject non-admin listing-source search: {error}")
        });
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_listing_source_for_existing_party_at_admin_route() {
    let party_id = seed_party("Existing Listing Source Operator", None, None).await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/admin/listing-sources", AURA_API.base_url());

    let response = client
        .post(&url)
        .bearer_auth(token.clone())
        .json(&json!({
            "name": "Created Listing Source",
            "operator": {"type": "EXISTING", "partyId": party_id.to_string()},
            "ingestionConfiguration": [{"type": "PARTNER_API"}],
            "url": "https://created-listing-source.example/",
            "image": "https://created-listing-source.example/image.png",
            "referralConfiguration": {"type": "PARTNERIZE", "camref": "campaign123"}
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create listing source: {error}"));
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (status, body) = json_response(response).await;

    let listing_source_id = body["listingSourceId"]
        .as_str()
        .unwrap_or_else(|| panic!("created response has no listing source ID"))
        .to_owned();
    assert_eq!(reqwest::StatusCode::CREATED, status);
    assert_eq!(
        Some(format!("/api/v1/admin/listing-sources/{listing_source_id}")),
        location
    );
    assert_eq!(json!(listing_source_id.as_str()), body["listingSourceId"]);
    assert!(body["listingSourceSlugId"].is_string());

    let response = client
        .get(&url)
        .bearer_auth(token)
        .query(&[("listingSourceId", listing_source_id.as_str())])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to read created listing source: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some(1), body["items"].as_array().map(Vec::len));
    assert_eq!(
        json!(party_id.to_string()),
        body["items"][0]["operator"]["partyId"]
    );
    assert_eq!(json!("Created Listing Source"), body["items"][0]["name"]);
    assert_eq!(json!(["PARTNER_API"]), body["items"][0]["ingestionMethods"]);
    assert_eq!(
        json!({"type": "PARTNERIZE", "camref": "campaign123"}),
        body["items"][0]["referralConfiguration"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_listing_source_with_new_party_without_echoing_webhook_secret() {
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/admin/listing-sources", AURA_API.base_url());

    let response = client
        .post(&url)
        .bearer_auth(token.clone())
        .json(&json!({
            "name": "  WooCommerce Listing Source  ",
            "operator": {
                "type": "NEW",
                "name": "  New Listing Source Operator  ",
                "phone": "+49 30 123456",
                "email": "operator@example.test"
            },
            "ingestionConfiguration": [{
                "type": "WOOCOMMERCE",
                "currency": "EUR",
                "language": "en"
            }],
            "woocommerceWebhookSecret": "provider-secret"
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create listing source with new party: {error}"));
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (status, body) = json_response(response).await;

    let listing_source_id = body["listingSourceId"]
        .as_str()
        .unwrap_or_else(|| panic!("created response has no listing source ID"))
        .to_owned();
    assert_eq!(reqwest::StatusCode::CREATED, status);
    assert_eq!(
        Some(format!("/api/v1/admin/listing-sources/{listing_source_id}")),
        location
    );
    assert!(!body.to_string().contains("provider-secret"));

    let response = client
        .get(&url)
        .bearer_auth(token)
        .query(&[("listingSourceId", listing_source_id.as_str())])
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to read created WooCommerce source: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some(1), body["items"].as_array().map(Vec::len));
    assert_eq!(
        json!("New Listing Source Operator"),
        body["items"][0]["operator"]["name"]
    );
    assert_eq!(json!(["WOOCOMMERCE"]), body["items"][0]["ingestionMethods"]);
    assert!(!body.to_string().contains("provider-secret"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_listing_source_create_for_non_admin() {
    let party_id = seed_party("Unauthorized Listing Source Operator", None, None).await;
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(user_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/admin/listing-sources",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&json!({
            "name": "Unauthorized Listing Source",
            "operator": {"type": "EXISTING", "partyId": party_id.to_string()},
            "ingestionConfiguration": [{"type": "PARTNER_API"}]
        }))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to reject non-admin listing-source create: {error}")
        });
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_listing_source_create_with_secret_for_non_woocommerce_source() {
    let party_id = seed_party("Invalid Secret Listing Source Operator", None, None).await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/admin/listing-sources",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&json!({
            "name": "Invalid Secret Listing Source",
            "operator": {"type": "EXISTING", "partyId": party_id.to_string()},
            "ingestionConfiguration": [{"type": "PARTNER_API"}],
            "woocommerceWebhookSecret": "must-not-be-stored"
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to validate listing-source secret: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_BODY_VALUE",
    );
    assert!(!body.to_string().contains("must-not-be-stored"));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_map_duplicate_shopify_domain_to_conflict_on_admin_create() {
    let party_id = seed_party("Shopify Conflict Operator", None, None).await;
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let url = format!("{}/api/v1/admin/listing-sources", AURA_API.base_url());
    let client = reqwest::Client::new();

    let response = client
        .post(&url)
        .bearer_auth(token.clone())
        .json(&json!({
            "name": "First Shopify Listing Source",
            "operator": {"type": "EXISTING", "partyId": party_id.to_string()},
            "ingestionConfiguration": [{
                "type": "SHOPIFY",
                "domain": "duplicate-shop.example",
                "currency": "EUR",
                "language": "en"
            }]
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create first Shopify source: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, response.status());

    let response = client
        .post(&url)
        .bearer_auth(token)
        .json(&json!({
            "name": "Second Shopify Listing Source",
            "operator": {"type": "EXISTING", "partyId": party_id.to_string()},
            "ingestionConfiguration": [{
                "type": "SHOPIFY",
                "domain": "duplicate-shop.example",
                "currency": "EUR",
                "language": "en"
            }]
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to test duplicate Shopify source: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::CONFLICT, "CONFLICT");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_remove_legacy_listing_source_create_route() {
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/listing-sources", AURA_API.base_url()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call legacy listing-source route: {error}"));

    assert_eq!(reqwest::StatusCode::NOT_FOUND, response.status());
}
