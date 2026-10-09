use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};

use api_support::{
    assert_problem, json_response, seed_access_token_for, seed_listing_source,
    seed_operator_partnership_listing_source_grant, seed_partnership_membership, seed_product,
    seed_user,
};
use auction_core::AuctionId;
use listing_source_core::ListingSourceId;
use product_listing_core::product_listing_id::ProductListingId;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use serde_json::json;

use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use user_core::access_token::{AccessTokenId, Scope};

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_get_and_update_auction_as_administrator() {
    let source_id = ListingSourceId::try_from(seed_listing_source().await)
        .unwrap_or_else(|error| panic!("invalid seeded ListingSource ID: {error}"));
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let read_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::AuctionsRead]),
    )
    .await;
    let client = reqwest::Client::new();

    let created = client
        .post(format!("{}/api/v1/admin/auctions", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&json!({
            "listingSourceId": source_id,
            "sourceAuctionId": " catalogue / 2026-0042 "
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create Auction: {error}"));
    let location = created
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let cache_control = created
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (created_status, created_body) = json_response(created).await;

    let auction_id = created_body["auctionId"]
        .as_str()
        .unwrap_or_else(|| panic!("created Auction response has no auctionId"))
        .parse::<AuctionId>()
        .unwrap_or_else(|error| panic!("created response has invalid Auction ID: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, created_status);
    assert_eq!(
        Some(format!("/api/v1/admin/auctions/{auction_id}")),
        location
    );
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(
        json!(source_id.to_string()),
        created_body["listingSourceId"]
    );
    assert_eq!(
        json!("catalogue / 2026-0042"),
        created_body["sourceAuctionId"]
    );
    assert!(created_body["name"].is_null());
    assert!(created_body["description"].is_null());
    assert!(created_body["format"].is_null());
    assert_eq!(json!(1), created_body["expectedVersion"]);
    assert!(created_body["schedule"]["liveStarts"].is_null());

    let fetched = client
        .get(format!(
            "{}/api/v1/admin/auctions/{auction_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(read_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get Auction: {error}"));
    let fetched_cache_control = fetched
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (fetched_status, fetched_body) = json_response(fetched).await;
    assert_eq!(reqwest::StatusCode::OK, fetched_status);
    assert_eq!(Some("no-store".to_owned()), fetched_cache_control);
    assert_eq!(json!(auction_id.to_string()), fetched_body["auctionId"]);

    let updated = client
        .patch(format!(
            "{}/api/v1/admin/auctions/{auction_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token.clone()))
        .json(&json!({
            "expectedVersion": 1,
            "name": {"language": "en", "text": "Autumn Decorative Arts"},
            "format": "TIMED",
            "schedule": {
                "lotsBeginClosing": "2026-10-18T16:03:00Z"
            }
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to update Auction: {error}"));
    let updated_cache_control = updated
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (updated_status, updated_body) = json_response(updated).await;
    assert_eq!(reqwest::StatusCode::OK, updated_status);
    assert_eq!(Some("no-store".to_owned()), updated_cache_control);
    assert_eq!(
        json!("Autumn Decorative Arts"),
        updated_body["name"]["text"]
    );
    assert_eq!(json!("TIMED"), updated_body["format"]);
    assert_eq!(json!(2), updated_body["expectedVersion"]);
    assert_eq!(
        json!("2026-10-18T16:03:00Z"),
        updated_body["schedule"]["lotsBeginClosing"]
    );

    let stale = client
        .patch(format!(
            "{}/api/v1/admin/auctions/{auction_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&json!({"expectedVersion": 1, "name": null}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to send stale Auction update: {error}"));
    let (stale_status, stale_body) = json_response(stale).await;
    assert_problem(
        stale_status,
        &stale_body,
        reqwest::StatusCode::CONFLICT,
        "CONFLICT",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_duplicate_key_invalid_id_and_non_admin_auction_requests() {
    let source_id = ListingSourceId::try_from(seed_listing_source().await)
        .unwrap_or_else(|error| panic!("invalid seeded ListingSource ID: {error}"));
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let admin_read_token = seed_access_token_for(
        admin_id,
        std::collections::HashSet::from([Scope::AuctionsRead]),
    )
    .await;
    let client = reqwest::Client::new();
    let body = json!({
        "listingSourceId": source_id,
        "sourceAuctionId": "catalogue-42"
    });

    let first = client
        .post(format!("{}/api/v1/admin/auctions", AURA_API.base_url()))
        .bearer_auth(String::from(admin_token.clone()))
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create first Auction: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, first.status());

    let duplicate = client
        .post(format!("{}/api/v1/admin/auctions", AURA_API.base_url()))
        .bearer_auth(String::from(admin_token.clone()))
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create duplicate Auction: {error}"));
    let (duplicate_status, duplicate_body) = json_response(duplicate).await;
    assert_problem(
        duplicate_status,
        &duplicate_body,
        reqwest::StatusCode::CONFLICT,
        "CONFLICT",
    );

    for invalid_id in [
        source_id.to_string(),
        AuctionId::new().as_uuid().to_string(),
        "auc_not-a-typeid".to_owned(),
    ] {
        let response = client
            .get(format!(
                "{}/api/v1/admin/auctions/{invalid_id}",
                AURA_API.base_url()
            ))
            .bearer_auth(String::from(admin_read_token.clone()))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to get invalid Auction ID: {error}"));
        let (status, response_body) = json_response(response).await;
        assert_problem(
            status,
            &response_body,
            reqwest::StatusCode::BAD_REQUEST,
            "INVALID_OBJECT_ID",
        );
    }

    let missing = client
        .get(format!(
            "{}/api/v1/admin/auctions/{}",
            AURA_API.base_url(),
            AuctionId::new()
        ))
        .bearer_auth(String::from(admin_read_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get missing Auction: {error}"));
    let (missing_status, missing_body) = json_response(missing).await;
    assert_problem(
        missing_status,
        &missing_body,
        reqwest::StatusCode::NOT_FOUND,
        "AUCTION_NOT_FOUND",
    );

    let user_id = seed_user("USER").await;
    let user_token = seed_access_token_for(user_id, std::collections::HashSet::new()).await;
    let forbidden = client
        .post(format!("{}/api/v1/admin/auctions", AURA_API.base_url()))
        .bearer_auth(String::from(user_token))
        .json(&json!({
            "listingSourceId": source_id,
            "sourceAuctionId": "forbidden-catalogue"
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reject non-admin Auction create: {error}"));
    let (forbidden_status, forbidden_body) = json_response(forbidden).await;
    assert_problem(
        forbidden_status,
        &forbidden_body,
        reqwest::StatusCode::FORBIDDEN,
        "FORBIDDEN",
    );
}

async fn create_admin_search_auction(
    client: &reqwest::Client,
    token: &str,
    source_id: ListingSourceId,
    source_auction_id: &str,
    name: Option<&str>,
    format: Option<&str>,
    reported_status: Option<&str>,
) -> AuctionId {
    let response = client
        .post(format!("{}/api/v1/admin/auctions", AURA_API.base_url()))
        .bearer_auth(token)
        .json(&json!({
            "listingSourceId": source_id,
            "sourceAuctionId": source_auction_id,
            "name": name.map(|text| json!({ "language": "en", "text": text })),
            "format": format,
            "reportedStatus": reported_status
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create Auction search fixture: {error}"));
    let (status, body) = json_response(response).await;
    assert_eq!(
        reqwest::StatusCode::CREATED,
        status,
        "response body: {body}"
    );
    body["auctionId"]
        .as_str()
        .unwrap_or_else(|| panic!("Auction fixture has no auctionId: {body}"))
        .parse::<AuctionId>()
        .unwrap_or_else(|error| panic!("invalid created Auction ID: {error}"))
}

async fn admin_auction_search(
    client: &reqwest::Client,
    token: &str,
    parameters: &[(&str, String)],
) -> (reqwest::StatusCode, serde_json::Value, Option<String>) {
    let response = client
        .get(format!("{}/api/v1/admin/auctions", AURA_API.base_url()))
        .bearer_auth(token)
        .query(parameters)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to search admin Auctions: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (status, body) = json_response(response).await;
    (status, body, cache_control)
}

async fn assert_admin_auction_reads(
    client: &reqwest::Client,
    token: &str,
    source_id: ListingSourceId,
    auction_id: AuctionId,
    expected_status: reqwest::StatusCode,
) {
    let (status, body, cache_control) =
        admin_auction_search(client, token, &[("listingSourceId", source_id.to_string())]).await;
    assert_eq!(Some("no-store".to_owned()), cache_control);
    if expected_status == reqwest::StatusCode::OK {
        assert_eq!(expected_status, status, "response body: {body}");
        assert_eq!(json!(1), body["size"]);
        assert_eq!(json!(auction_id.to_string()), body["items"][0]["auctionId"]);
    } else {
        assert_problem(status, &body, expected_status, "FORBIDDEN");
    }

    let response = client
        .get(format!(
            "{}/api/v1/admin/auctions/{auction_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(token)
        .send()
        .await
        .expect("get admin Auction details for scope regression");
    assert_eq!(
        Some("no-store"),
        response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
    );
    let (status, body) = json_response(response).await;
    if expected_status == reqwest::StatusCode::OK {
        assert_eq!(expected_status, status, "response body: {body}");
        assert_eq!(json!(auction_id.to_string()), body["auctionId"]);
    } else {
        assert_problem(status, &body, expected_status, "FORBIDDEN");
    }
}

async fn assert_access_token_scope_readback(
    client: &reqwest::Client,
    owner_token: &str,
    access_token_id: AccessTokenId,
    scopes: &[&str],
) {
    let response = client
        .get(format!(
            "{}/api/v1/me/access-tokens/{access_token_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(owner_token)
        .send()
        .await
        .expect("read back Auction scope token metadata");
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::OK, status, "response body: {body}");
    assert_eq!(json!(access_token_id.to_string()), body["accessTokenId"]);
    assert_eq!(json!(scopes), body["scopes"]);
    assert!(body.get("accessToken").is_none());
}

async fn issue_auction_test_token(
    client: &reqwest::Client,
    owner_token: &str,
    scopes: &[&str],
) -> (AccessTokenId, String) {
    let response = client
        .post(format!("{}/api/v1/me/access-tokens", AURA_API.base_url()))
        .bearer_auth(owner_token)
        .json(&json!({"name": "Auction read scope regression", "scopes": scopes}))
        .send()
        .await
        .expect("issue Auction scope token through the API");
    assert_eq!(
        Some("no-store"),
        response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
    );
    let (status, body) = json_response(response).await;
    assert_eq!(reqwest::StatusCode::CREATED, status);
    let access_token_id = body["accessTokenId"]
        .as_str()
        .and_then(|value| AccessTokenId::try_from(value).ok())
        .expect("issued token has a canonical AccessToken ID");
    let token = body["accessToken"]
        .as_str()
        .expect("issued token has its one-time plaintext")
        .to_owned();
    assert_access_token_scope_readback(client, owner_token, access_token_id, scopes).await;
    (access_token_id, token)
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_enforce_auction_read_scope_after_api_issuance_and_updates() {
    let source_id = ListingSourceId::try_from(seed_listing_source().await)
        .unwrap_or_else(|error| panic!("invalid seeded ListingSource ID: {error}"));
    let admin_id = seed_user("ADMIN").await;
    let write_token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let owner_token = api_support::cognito_access_token_for_test_user(admin_id);
    let client = reqwest::Client::new();
    let auction_id = create_admin_search_auction(
        &client,
        &write_token,
        source_id,
        "auction-read-scope-regression",
        None,
        None,
        None,
    )
    .await;

    assert_admin_auction_reads(
        &client,
        &owner_token,
        source_id,
        auction_id,
        reqwest::StatusCode::OK,
    )
    .await;
    let (access_token_id, token) =
        issue_auction_test_token(&client, &owner_token, &["auctions:read"]).await;
    assert_admin_auction_reads(
        &client,
        &token,
        source_id,
        auction_id,
        reqwest::StatusCode::OK,
    )
    .await;

    for (scopes, expected_status) in [
        (&[][..], reqwest::StatusCode::FORBIDDEN),
        (&["users:read"][..], reqwest::StatusCode::FORBIDDEN),
        (&["auctions:read"][..], reqwest::StatusCode::OK),
    ] {
        let response = client
            .patch(format!("{}/api/v1/me/access-tokens", AURA_API.base_url()))
            .bearer_auth(&owner_token)
            .json(&json!({"accessTokenId": access_token_id, "scopes": scopes}))
            .send()
            .await
            .expect("update Auction scope token through the API");
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(reqwest::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
        let (status, body) = json_response(response).await;
        assert_eq!(reqwest::StatusCode::OK, status, "response body: {body}");
        assert_eq!(json!(scopes), body["scopes"]);
        assert_access_token_scope_readback(&client, &owner_token, access_token_id, scopes).await;
        assert_admin_auction_reads(&client, &token, source_id, auction_id, expected_status).await;
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_forbid_non_admin_auction_reads_even_with_scope_without_caching() {
    let source_id = ListingSourceId::try_from(seed_listing_source().await)
        .unwrap_or_else(|error| panic!("invalid seeded ListingSource ID: {error}"));
    let admin_id = seed_user("ADMIN").await;
    let admin_token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();
    let auction_id = create_admin_search_auction(
        &client,
        &admin_token,
        source_id,
        "auction-read-role-regression",
        None,
        None,
        None,
    )
    .await;
    let user_id = seed_user("USER").await;
    let owner_token = api_support::cognito_access_token_for_test_user(user_id);
    let (_, token) = issue_auction_test_token(&client, &owner_token, &["auctions:read"]).await;

    assert_admin_auction_reads(
        &client,
        &token,
        source_id,
        auction_id,
        reqwest::StatusCode::FORBIDDEN,
    )
    .await;
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_find_the_exact_source_scoped_conflict_with_admin_data_and_returned_size() {
    let source_id = ListingSourceId::try_from(seed_listing_source().await)
        .unwrap_or_else(|error| panic!("invalid seeded ListingSource ID: {error}"));
    let other_source_id = ListingSourceId::try_from(seed_listing_source().await)
        .unwrap_or_else(|error| panic!("invalid second ListingSource ID: {error}"));
    let admin_id = seed_user("ADMIN").await;
    let token = String::from(
        seed_access_token_for(
            admin_id,
            std::collections::HashSet::from([Scope::AuctionsRead]),
        )
        .await,
    );
    let client = reqwest::Client::new();
    let auction_id = create_admin_search_auction(
        &client,
        &token,
        source_id,
        " conflict / 42 ",
        Some("Auction to locate"),
        Some("LIVE"),
        None,
    )
    .await;
    let other_id = create_admin_search_auction(
        &client,
        &token,
        other_source_id,
        "conflict / 42",
        None,
        None,
        None,
    )
    .await;
    let _prefix_id = create_admin_search_auction(
        &client,
        &token,
        source_id,
        "conflict / 42-extra",
        None,
        None,
        None,
    )
    .await;

    let duplicate = client
        .post(format!("{}/api/v1/admin/auctions", AURA_API.base_url()))
        .bearer_auth(&token)
        .json(&json!({ "listingSourceId": source_id, "sourceAuctionId": " conflict / 42 " }))
        .send()
        .await
        .expect("duplicate Auction request");
    let (duplicate_status, duplicate_body) = json_response(duplicate).await;
    assert_problem(
        duplicate_status,
        &duplicate_body,
        reqwest::StatusCode::CONFLICT,
        "CONFLICT",
    );

    let update = client
        .patch(format!("{}/api/v1/admin/auctions/{auction_id}", AURA_API.base_url()))
        .bearer_auth(&token)
        .json(&json!({ "expectedVersion": 1, "name": { "language": "en", "text": "Renamed auction" } }))
        .send().await.expect("update Auction fixture");
    let (update_status, update_body) = json_response(update).await;
    assert_eq!(
        reqwest::StatusCode::OK,
        update_status,
        "response body: {update_body}"
    );
    assert_eq!(json!(2), update_body["expectedVersion"]);

    let (status, body, cache_control) = admin_auction_search(
        &client,
        &token,
        &[
            ("listingSourceId", source_id.to_string()),
            ("sourceAuctionId", " conflict / 42 ".to_owned()),
            ("size", "100".to_owned()),
        ],
    )
    .await;
    assert_eq!(reqwest::StatusCode::OK, status, "response body: {body}");
    assert_eq!(Some("no-store".to_owned()), cache_control);
    assert_eq!(
        json!(1),
        body["size"],
        "size is the returned item count, not the limit"
    );
    assert_eq!(Some(1), body["items"].as_array().map(Vec::len));
    assert_eq!(json!(auction_id.to_string()), body["items"][0]["auctionId"]);
    assert_eq!(
        json!(source_id.to_string()),
        body["items"][0]["listingSourceId"]
    );
    assert_eq!(json!("conflict / 42"), body["items"][0]["sourceAuctionId"]);
    assert_eq!(json!("Renamed auction"), body["items"][0]["name"]["text"]);
    assert_eq!(json!(2), body["items"][0]["expectedVersion"]);
    assert!(body.get("searchAfter").is_none());

    let (other_status, other_body, _) = admin_auction_search(
        &client,
        &token,
        &[
            ("listingSourceId", other_source_id.to_string()),
            ("sourceAuctionId", "conflict / 42".to_owned()),
        ],
    )
    .await;
    assert_eq!(
        reqwest::StatusCode::OK,
        other_status,
        "response body: {other_body}"
    );
    assert_eq!(
        json!(other_id.to_string()),
        other_body["items"][0]["auctionId"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_filter_and_page_admin_auctions_with_scoped_deterministic_sort() {
    let source_id = ListingSourceId::try_from(seed_listing_source().await)
        .unwrap_or_else(|error| panic!("invalid seeded ListingSource ID: {error}"));
    let admin_id = seed_user("ADMIN").await;
    let token = String::from(
        seed_access_token_for(
            admin_id,
            std::collections::HashSet::from([Scope::AuctionsRead]),
        )
        .await,
    );
    let client = reqwest::Client::new();
    let first = create_admin_search_auction(
        &client,
        &token,
        source_id,
        "f19-alpha-a",
        Some("Alpha"),
        Some("LIVE"),
        Some("ENDED"),
    )
    .await;
    let second = create_admin_search_auction(
        &client,
        &token,
        source_id,
        "f19-alpha-b",
        Some("alpha"),
        Some("LIVE"),
        Some("ENDED"),
    )
    .await;
    let zulu = create_admin_search_auction(
        &client,
        &token,
        source_id,
        "f19-zulu",
        Some("Zulu"),
        Some("TIMED"),
        Some("SCHEDULED"),
    )
    .await;
    let unnamed =
        create_admin_search_auction(&client, &token, source_id, "f19-unnamed", None, None, None)
            .await;

    // Fixed instants exercise time ordering and the Auction ID tie-breaker without timing assumptions.
    let pool = get_postgres_client().await;
    for (id, created, updated) in [
        (
            first,
            time::macros::datetime!(2026-01-01 00:00 UTC),
            time::macros::datetime!(2026-01-02 00:00 UTC),
        ),
        (
            second,
            time::macros::datetime!(2026-01-01 00:00 UTC),
            time::macros::datetime!(2026-01-02 00:00 UTC),
        ),
        (
            zulu,
            time::macros::datetime!(2026-01-02 00:00 UTC),
            time::macros::datetime!(2026-01-03 00:00 UTC),
        ),
        (
            unnamed,
            time::macros::datetime!(2026-01-03 00:00 UTC),
            time::macros::datetime!(2026-01-01 00:00 UTC),
        ),
    ] {
        sqlx::query("UPDATE auctions SET created = $1, updated = $2 WHERE auction_id = $3")
            .bind(created)
            .bind(updated)
            .bind(id.as_uuid())
            .execute(&pool)
            .await
            .expect("set deterministic Auction search instants");
    }

    let source = source_id.to_string();
    let (filtered_status, filtered, _) = admin_auction_search(
        &client,
        &token,
        &[
            ("listingSourceId", source.clone()),
            ("query", "ALPH".to_owned()),
            ("format", "LIVE".to_owned()),
            ("reportedStatus", "ENDED".to_owned()),
            ("size", "1000".to_owned()),
        ],
    )
    .await;
    assert_eq!(
        reqwest::StatusCode::OK,
        filtered_status,
        "response body: {filtered}"
    );
    assert_eq!(json!(2), filtered["size"]);
    let filtered_ids = filtered["items"]
        .as_array()
        .expect("filtered items")
        .iter()
        .map(|item| item["auctionId"].as_str().expect("Auction ID").to_owned())
        .collect::<Vec<_>>();
    assert!(
        filtered_ids.contains(&first.to_string()) && filtered_ids.contains(&second.to_string())
    );

    for (sort, order, expected) in [
        (
            "name",
            "asc",
            vec![first.min(second), first.max(second), zulu, unnamed],
        ),
        (
            "name",
            "desc",
            vec![zulu, first.max(second), first.min(second), unnamed],
        ),
        (
            "created",
            "asc",
            vec![first.min(second), first.max(second), zulu, unnamed],
        ),
        (
            "updated",
            "desc",
            vec![zulu, first.max(second), first.min(second), unnamed],
        ),
    ] {
        let mut after: Option<String> = None;
        let mut seen = Vec::new();
        loop {
            let mut parameters = vec![
                ("listingSourceId", source.clone()),
                ("sort", sort.to_owned()),
                ("order", order.to_owned()),
                ("size", "1".to_owned()),
            ];
            if let Some(cursor) = &after {
                parameters.push(("searchAfter", cursor.clone()));
            }
            let (status, body, cache_control) =
                admin_auction_search(&client, &token, &parameters).await;
            assert_eq!(reqwest::StatusCode::OK, status, "response body: {body}");
            assert_eq!(Some("no-store".to_owned()), cache_control);
            assert_eq!(json!(1), body["size"]);
            seen.push(
                body["items"][0]["auctionId"]
                    .as_str()
                    .expect("Auction ID")
                    .parse::<AuctionId>()
                    .expect("valid Auction ID"),
            );
            after = body["searchAfter"].as_str().map(str::to_owned);
            if after.is_none() {
                break;
            }
            assert!(seen.len() < 5, "cursor must terminate without duplicates");
        }
        assert_eq!(expected, seen, "sort={sort}, order={order}");
    }

    let (page_status, page, _) = admin_auction_search(
        &client,
        &token,
        &[
            ("listingSourceId", source.clone()),
            ("sort", "name".to_owned()),
            ("order", "asc".to_owned()),
            ("size", "1".to_owned()),
        ],
    )
    .await;
    assert_eq!(
        reqwest::StatusCode::OK,
        page_status,
        "response body: {page}"
    );
    let cursor = page["searchAfter"].as_str().expect("nonterminal cursor");
    for parameters in [
        vec![
            ("listingSourceId", source.clone()),
            ("sort", "name".to_owned()),
            ("order", "desc".to_owned()),
            ("searchAfter", cursor.to_owned()),
        ],
        vec![
            ("listingSourceId", source.clone()),
            ("format", "LIVE".to_owned()),
            ("sort", "name".to_owned()),
            ("order", "asc".to_owned()),
            ("searchAfter", cursor.to_owned()),
        ],
    ] {
        let (status, body, cache_control) =
            admin_auction_search(&client, &token, &parameters).await;
        assert_problem(
            status,
            &body,
            reqwest::StatusCode::BAD_REQUEST,
            "BAD_QUERY_PARAMETER_VALUE",
        );
        assert_eq!(Some("no-store".to_owned()), cache_control);
    }

    // A lone sort or order does not override the default updated/descending scope.
    for parameters in [
        vec![
            ("listingSourceId", source.clone()),
            ("sort", "name".to_owned()),
            ("size", "1".to_owned()),
        ],
        vec![
            ("listingSourceId", source.clone()),
            ("order", "asc".to_owned()),
            ("size", "1".to_owned()),
        ],
    ] {
        let (status, body, _) = admin_auction_search(&client, &token, &parameters).await;
        assert_eq!(reqwest::StatusCode::OK, status, "response body: {body}");
        assert_eq!(json!(zulu.to_string()), body["items"][0]["auctionId"]);
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_page_public_scheduled_auctions_with_scoped_json_cursors_and_anonymous_caching() {
    let source_id = ListingSourceId::try_from(seed_listing_source().await)
        .unwrap_or_else(|error| panic!("invalid seeded ListingSource ID: {error}"));
    let admin_id = seed_user("ADMIN").await;
    let token =
        String::from(seed_access_token_for(admin_id, std::collections::HashSet::new()).await);
    let client = reqwest::Client::new();
    let pool = get_postgres_client().await;
    let from = time::macros::datetime!(2026-10-18 16:00 UTC);
    let to = time::macros::datetime!(2026-10-18 17:00 UTC);
    let tied = time::macros::datetime!(2026-10-18 16:30 UTC);
    let mut ids = Vec::new();
    for (key, instant) in [
        (
            "scheduled-below",
            Some(time::macros::datetime!(2026-10-18 15:59:59 UTC)),
        ),
        ("scheduled-from", Some(from)),
        ("scheduled-tie-a", Some(tied)),
        ("scheduled-tie-b", Some(tied)),
        ("scheduled-to", Some(to)),
        ("scheduled-null-a", None),
        ("scheduled-null-b", None),
    ] {
        let id =
            create_admin_search_auction(&client, &token, source_id, key, None, None, None).await;
        sqlx::query("UPDATE auctions SET live_starts_at = $1 WHERE auction_id = $2")
            .bind(instant)
            .bind(id.as_uuid())
            .execute(&pool)
            .await
            .expect("set deterministic public Auction schedule");
        ids.push(id);
    }
    let [below, at_from, tie_a, tie_b, at_to, null_a, null_b]: [AuctionId; 7] =
        ids.try_into().expect("seven scheduled Auction fixtures");
    let url = format!("{}/api/v1/auctions", AURA_API.base_url());
    let source = source_id.to_string();
    let mut scope_cursor = None;
    for window in [false, true] {
        for order in ["asc", "desc"] {
            let mut expected = if window {
                vec![at_from, tie_a.min(tie_b), tie_a.max(tie_b)]
            } else {
                vec![below, at_from, tie_a.min(tie_b), tie_a.max(tie_b), at_to]
            };
            if order == "desc" {
                expected.reverse();
            }
            if !window {
                let mut nulls = [null_a.min(null_b), null_a.max(null_b)];
                if order == "desc" {
                    nulls.reverse();
                }
                expected.extend(nulls);
            }
            let mut after: Option<serde_json::Value> = None;
            for (index, expected_id) in expected.iter().enumerate() {
                let mut parameters = vec![
                    ("listingSourceId", source.clone()),
                    ("sort", "scheduled".to_owned()),
                    ("timeRole", "LIVE_STARTS".to_owned()),
                    ("order", order.to_owned()),
                    ("pageSize", "1".to_owned()),
                ];
                if window {
                    parameters.extend([
                        ("from", "2026-10-18T16:00:00Z".to_owned()),
                        ("to", "2026-10-18T17:00:00Z".to_owned()),
                    ]);
                }
                if let Some(cursor) = &after {
                    parameters.push((
                        "searchAfter",
                        serde_json::to_string(cursor).expect("serialize complete directory cursor"),
                    ));
                }
                let response = client
                    .get(&url)
                    .query(&parameters)
                    .send()
                    .await
                    .expect("page public scheduled Auctions");
                let cache_control = response
                    .headers()
                    .get(reqwest::header::CACHE_CONTROL)
                    .and_then(|value| value.to_str().ok())
                    .map(ToOwned::to_owned);
                let (status, body) = json_response(response).await;
                assert_eq!(reqwest::StatusCode::OK, status, "response body: {body}");
                assert_eq!(
                    Some("public, max-age=0, s-maxage=60, stale-if-error=0".to_owned()),
                    cache_control
                );
                assert_eq!(json!(1), body["pageSize"]);
                let items = body["items"].as_array().expect("directory items");
                assert_eq!(1, items.len(), "window={window}, order={order}: {body}");
                assert_eq!(
                    json!(expected_id.to_string()),
                    items[0]["auctionId"],
                    "window={window}, order={order}, page={index}"
                );
                after = body.get("searchAfter").cloned();
                assert_eq!(index + 1 < expected.len(), after.is_some());
                if let Some(cursor) = &after {
                    assert!(
                        cursor.is_object(),
                        "expected complete JSON cursor: {cursor}"
                    );
                    assert_eq!(items[0]["schedule"]["liveStarts"], cursor["scheduled"]);
                }
                if !window && order == "asc" && index == 0 {
                    scope_cursor = after.clone();
                }
            }
        }
    }

    let cursor = serde_json::to_string(&scope_cursor.expect("nonterminal ascending cursor"))
        .expect("serialize complete directory cursor");
    for (order, role) in [("desc", "LIVE_STARTS"), ("asc", "SCHEDULED_END")] {
        let response = client
            .get(&url)
            .query(&[
                ("listingSourceId", source.clone()),
                ("sort", "scheduled".to_owned()),
                ("timeRole", role.to_owned()),
                ("order", order.to_owned()),
                ("pageSize", "1".to_owned()),
                ("searchAfter", cursor.clone()),
            ])
            .send()
            .await
            .expect("reject public directory cursor scope mismatch");
        let cache_control = response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let (status, body) = json_response(response).await;
        assert_eq!(
            reqwest::StatusCode::BAD_REQUEST,
            status,
            "response body: {body}"
        );
        assert_eq!(Some("private, no-store".to_owned()), cache_control);
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_browse_public_auction_directory_detail_and_empty_catalogue_anonymously() {
    let source_id = ListingSourceId::try_from(seed_listing_source().await)
        .unwrap_or_else(|error| panic!("invalid seeded ListingSource ID: {error}"));
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let client = reqwest::Client::new();
    let created = client
        .post(format!("{}/api/v1/admin/auctions", AURA_API.base_url()))
        .bearer_auth(String::from(admin_token))
        .json(&json!({
            "listingSourceId": source_id,
            "sourceAuctionId": "public-catalogue"
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create public Auction fixture: {error}"));
    let (_, created_body) = json_response(created).await;
    let auction_id = created_body["auctionId"]
        .as_str()
        .unwrap_or_else(|| panic!("created Auction response has no auctionId"))
        .parse::<AuctionId>()
        .unwrap_or_else(|error| panic!("created response has invalid Auction ID: {error}"));

    let directory = client
        .get(format!(
            "{}/api/v1/auctions?pageSize=5",
            AURA_API.base_url()
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list public Auctions: {error}"));
    let directory_cache_control = directory
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (directory_status, directory_body) = json_response(directory).await;
    assert_eq!(reqwest::StatusCode::OK, directory_status);
    assert_eq!(
        Some("public, max-age=0, s-maxage=60, stale-if-error=0".to_owned()),
        directory_cache_control
    );
    assert_eq!(
        json!(auction_id.to_string()),
        directory_body["items"][0]["auctionId"]
    );

    let detail = client
        .get(format!(
            "{}/api/v1/auctions/{auction_id}",
            AURA_API.base_url()
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get public Auction: {error}"));
    let detail_cache_control = detail
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (detail_status, detail_body) = json_response(detail).await;
    assert_eq!(reqwest::StatusCode::OK, detail_status);
    assert_eq!(
        Some("public, max-age=0, s-maxage=60, stale-if-error=0".to_owned()),
        detail_cache_control
    );
    assert_eq!(json!(auction_id.to_string()), detail_body["auctionId"]);
    assert!(detail_body.get("sourceAuctionId").is_none());
    assert!(detail_body.get("expectedVersion").is_none());

    let catalogue = client
        .get(format!(
            "{}/api/v1/auctions/{auction_id}/product-listings?pageSize=5",
            AURA_API.base_url()
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get public Auction catalogue: {error}"));
    let catalogue_cache_control = catalogue
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (catalogue_status, catalogue_body) = json_response(catalogue).await;
    assert_eq!(reqwest::StatusCode::OK, catalogue_status);
    assert_eq!(
        Some("public, max-age=0, s-maxage=60, stale-if-error=0".to_owned()),
        catalogue_cache_control
    );
    assert_eq!(json!(5), catalogue_body["pageSize"]);
    assert_eq!(json!([]), catalogue_body["items"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_keep_member_lot_facts_when_shared_schedule_changes_and_redact_hidden_catalogue_data()
 {
    let source_id = ListingSourceId::try_from(seed_listing_source().await)
        .unwrap_or_else(|error| panic!("invalid seeded ListingSource ID: {error}"));
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let client = reqwest::Client::new();
    let source_auction_id = "f12-typed-membership";

    let created_auction = client
        .post(format!("{}/api/v1/admin/auctions", AURA_API.base_url()))
        .bearer_auth(String::from(admin_token.clone()))
        .json(&json!({
            "listingSourceId": source_id,
            "sourceAuctionId": source_auction_id,
            "name": { "language": "en", "text": "F12 catalogue" },
            "format": "TIMED"
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create Auction fixture: {error}"));
    let (created_auction_status, created_auction_body) = json_response(created_auction).await;
    assert_eq!(
        reqwest::StatusCode::CREATED,
        created_auction_status,
        "response body: {created_auction_body}"
    );
    let auction_id = created_auction_body["auctionId"]
        .as_str()
        .unwrap_or_else(|| panic!("created Auction response has no auction ID"))
        .parse::<AuctionId>()
        .unwrap_or_else(|error| panic!("invalid created Auction ID: {error}"));

    let partner_id = seed_user("USER").await;
    seed_partnership_membership(partner_id, source_id.into_uuid()).await;
    seed_operator_partnership_listing_source_grant(source_id.into_uuid()).await;
    let partner_token = seed_access_token_for(
        partner_id,
        std::collections::HashSet::from([Scope::ProductListingsWrite]),
    )
    .await;
    let source_listing_id = "f12-catalogue-member";
    let created_product = client
        .post(format!(
            "{}/api/v1/listing-sources/{source_id}/product-listings",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(partner_token.clone()))
        .json(&json!([{
            "sourceListingId": source_listing_id,
            "title": { "language": "en", "text": "F12 hidden catalogue cabinet" },
            "description": { "language": "en", "text": "Typed partner Auction membership." },
            "availability": "AVAILABLE",
            "url": "https://partner.example/f12-catalogue-member",
            "images": [],
            "auction": {
                "auctionId": auction_id.to_string(),
                "lotNumber": "42",
                "cataloguePosition": 42,
                "timing": {
                    "biddingOpens": "2026-10-18T08:00:00Z",
                    "scheduledCloses": "2026-10-18T16:03:00Z"
                }
            }
        }]))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create typed partner product: {error}"));
    let (created_product_status, created_product_body) = json_response(created_product).await;
    assert_eq!(
        reqwest::StatusCode::OK,
        created_product_status,
        "response body: {created_product_body}"
    );
    assert_eq!(json!([]), created_product_body);

    let pool = get_postgres_client().await;
    let (product_listing_uuid, event_id): (uuid::Uuid, uuid::Uuid) = sqlx::query_as(
        "SELECT product_listing_id, current_event_id FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = $2",
    )
    .bind(source_id.as_uuid())
    .bind(source_listing_id)
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to read typed partner product: {error}"));
    let product_listing_id = ProductListingId::try_from(product_listing_uuid)
        .unwrap_or_else(|error| panic!("invalid typed partner ProductListing ID: {error}"));

    let updated_schedule = client
        .patch(format!(
            "{}/api/v1/admin/auctions/{auction_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(admin_token.clone()))
        .json(&json!({
            "expectedVersion": 1,
            "schedule": {
                "lotsBeginClosing": "2026-10-18T18:00:00Z"
            }
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to update shared Auction schedule: {error}"));
    let (updated_schedule_status, updated_schedule_body) = json_response(updated_schedule).await;
    assert_eq!(
        reqwest::StatusCode::OK,
        updated_schedule_status,
        "response body: {updated_schedule_body}"
    );
    assert_eq!(
        json!("2026-10-18T18:00:00Z"),
        updated_schedule_body["schedule"]["lotsBeginClosing"]
    );

    let filter_id = UserSearchFilterId::new();
    sqlx::query(
        "INSERT INTO search_filters (user_search_filter_id, user_id, name, notifications, state, search, language, currency) VALUES ($1, $2, 'F12 hidden catalogue alerts', true, 'ACTIVE', '{}', 'en', 'EUR')",
    )
    .bind(filter_id.as_uuid())
    .bind(partner_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to seed hidden catalogue filter: {error}"));

    for position in 0_i64..10 {
        let earlier_product_id = seed_product().await;
        let earlier_event_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT current_event_id FROM product_listings WHERE product_listing_id = $1",
        )
        .bind(earlier_product_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to read earlier product event: {error}"));
        sqlx::query(
            "INSERT INTO search_filter_matches (user_id, user_search_filter_id, product_listing_id, origin_event_id, user_search_filter_name, created) VALUES ($1, $2, $3, $4, 'F12 hidden catalogue alerts', now() - ($5 * interval '1 minute'))",
        )
        .bind(partner_id.as_uuid())
        .bind(filter_id.as_uuid())
        .bind(earlier_product_id.as_uuid())
        .bind(earlier_event_id)
        .bind(10 - position)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed earlier search-filter match: {error}"));
    }
    sqlx::query(
        "INSERT INTO search_filter_matches (user_id, user_search_filter_id, product_listing_id, origin_event_id, user_search_filter_name, created) VALUES ($1, $2, $3, $4, 'F12 hidden catalogue alerts', now())",
    )
    .bind(partner_id.as_uuid())
    .bind(filter_id.as_uuid())
    .bind(product_listing_id.as_uuid())
    .bind(event_id)
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to seed hidden catalogue match: {error}"));

    let catalogue_url = format!(
        "{}/api/v1/auctions/{auction_id}/product-listings?language=en&currency=EUR&pageSize=5",
        AURA_API.base_url()
    );
    let anonymous_catalogue = client
        .get(&catalogue_url)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get anonymous Auction catalogue: {error}"));
    let (anonymous_status, anonymous_body) = json_response(anonymous_catalogue).await;
    assert_eq!(
        reqwest::StatusCode::OK,
        anonymous_status,
        "response body: {anonymous_body}"
    );
    assert_eq!(
        json!(product_listing_id.to_string()),
        anonymous_body["items"][0]["item"]["productListingId"]
    );
    assert_eq!(
        json!(auction_id.to_string()),
        anonymous_body["items"][0]["item"]["auction"]["auctionId"]
    );
    assert_eq!(
        json!(auction_id.to_string()),
        anonymous_body["items"][0]["item"]["auction"]["auctionId"]
    );
    assert_eq!(
        json!("42"),
        anonymous_body["items"][0]["item"]["lot"]["lotNumber"]
    );
    assert_eq!(
        json!("2026-10-18T16:03:00Z"),
        anonymous_body["items"][0]["item"]["lot"]["scheduledCloses"]
    );
    assert_eq!(
        json!("2026-10-18T18:00:00Z"),
        anonymous_body["items"][0]["item"]["auction"]["schedule"]["lotsBeginClosing"]
    );

    let hidden_catalogue = client
        .get(&catalogue_url)
        .bearer_auth(String::from(partner_token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get hidden Auction catalogue: {error}"));
    let (hidden_status, hidden_body) = json_response(hidden_catalogue).await;
    assert_eq!(
        reqwest::StatusCode::OK,
        hidden_status,
        "response body: {hidden_body}"
    );
    assert_eq!(
        json!(product_listing_id.to_string()),
        hidden_body["items"][0]["item"]["productListingId"]
    );
    assert_eq!(
        json!(true),
        hidden_body["items"][0]["userState"]["searchFilter"]["hidden"]
    );
    assert!(hidden_body["items"][0]["item"]["auction"].is_null());
    assert!(hidden_body["items"][0]["item"]["lot"].is_null());
    assert!(
        hidden_body["items"][0]["item"]
            .get("productListingTitleSlugId")
            .is_none()
    );
    assert!(
        !hidden_body.to_string().contains(&auction_id.to_string()),
        "hidden catalogue serialization must not disclose Auction identity: {hidden_body}"
    );
}
