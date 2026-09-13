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
use user_core::access_token::Scope;

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_get_and_update_auction_as_administrator() {
    let source_id = ListingSourceId::try_from(seed_listing_source().await)
        .unwrap_or_else(|error| panic!("invalid seeded ListingSource ID: {error}"));
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
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
    assert_eq!(json!([]), created_body["protectedFields"]);
    assert!(created_body["schedule"]["liveStarts"].is_null());

    let fetched = client
        .get(format!(
            "{}/api/v1/admin/auctions/{auction_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token.clone()))
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
                "lotsBeginClosing": {
                    "precision": "INSTANT",
                    "at": "2026-10-18T16:03:00Z",
                    "sourceTimezone": "Europe/Berlin"
                }
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
        json!({
            "precision": "INSTANT",
            "at": "2026-10-18T16:03:00Z",
            "sourceTimezone": "Europe/Berlin"
        }),
        updated_body["schedule"]["lotsBeginClosing"]
    );
    assert_eq!(
        json!(["NAME", "FORMAT", "LOTS_BEGIN_CLOSING"]),
        updated_body["protectedFields"]
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
            .bearer_auth(String::from(admin_token.clone()))
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
        .bearer_auth(String::from(admin_token.clone()))
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
    assert_eq!(Some("no-store".to_owned()), directory_cache_control);
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
    assert_eq!(Some("no-store".to_owned()), detail_cache_control);
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
    assert_eq!(Some("no-store".to_owned()), catalogue_cache_control);
    assert_eq!(json!(5), catalogue_body["pageSize"]);
    assert_eq!(json!([]), catalogue_body["items"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_resolve_typed_partner_membership_and_redact_hidden_catalogue_auction_data() {
    let source_id = ListingSourceId::try_from(seed_listing_source().await)
        .unwrap_or_else(|error| panic!("invalid seeded ListingSource ID: {error}"));
    let admin_id = seed_user("ADMIN").await;
    let admin_token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let client = reqwest::Client::new();
    let source_auction_id = "f12-typed-membership";

    let created_auction = client
        .post(format!("{}/api/v1/admin/auctions", AURA_API.base_url()))
        .bearer_auth(String::from(admin_token))
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
                "sourceAuctionId": source_auction_id,
                "lotNumber": "42",
                "cataloguePosition": 42,
                "timing": {
                    "biddingOpens": { "precision": "DATE", "on": "2026-10-18" },
                    "scheduledCloses": {
                        "precision": "INSTANT",
                        "at": "2026-10-18T16:03:00Z"
                    }
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
        anonymous_body["items"][0]["item"]["auctionSummary"]["auctionId"]
    );
    assert_eq!(
        json!("42"),
        anonymous_body["items"][0]["item"]["auction"]["lotNumber"]
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
    assert!(
        hidden_body["items"][0]["item"]
            .get("auctionSummary")
            .is_none()
    );
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

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_correct_and_release_product_listing_auction_context_without_public_domain_changes()
{
    let product_listing_id = seed_product().await;
    let admin_id = seed_user("ADMIN").await;
    let token = seed_access_token_for(admin_id, std::collections::HashSet::new()).await;
    let client = reqwest::Client::new();
    let context_url = format!(
        "{}/api/v1/admin/product-listings/{product_listing_id}/auction-context",
        AURA_API.base_url()
    );
    let correction_url = format!(
        "{}/api/v1/admin/product-listings/{product_listing_id}/auction-corrections",
        AURA_API.base_url()
    );
    let override_url = format!(
        "{}/api/v1/admin/product-listings/{product_listing_id}/auction-override",
        AURA_API.base_url()
    );
    let public_listing_url = format!(
        "{}/api/v1/product-listings/{product_listing_id}",
        AURA_API.base_url()
    );
    let history_url = format!(
        "{}/api/v1/product-listings/{product_listing_id}/history",
        AURA_API.base_url()
    );
    let correction_reason = "acceptance-only auction context barrier";

    let public_before = client
        .get(&public_listing_url)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to get public ProductListing before correction: {error}")
        });
    let (public_before_status, public_before_body) = json_response(public_before).await;
    assert_eq!(reqwest::StatusCode::OK, public_before_status);

    let history_before = client
        .get(&history_url)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to get ProductListing history before correction: {error}")
        });
    let (history_before_status, history_before_body) = json_response(history_before).await;
    assert_eq!(reqwest::StatusCode::OK, history_before_status);

    let context_before = client
        .get(&context_url)
        .bearer_auth(String::from(token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get ProductListing auction context: {error}"));
    let initial_etag = context_before
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| panic!("auction-context response has no ETag"));
    assert!(
        initial_etag.starts_with("\"plv-")
            && initial_etag.ends_with('"')
            && !initial_etag.starts_with("W/"),
        "auction-context ETag must be strong: {initial_etag}"
    );
    let (context_before_status, context_before_body) = json_response(context_before).await;
    assert_eq!(reqwest::StatusCode::OK, context_before_status);
    assert_eq!(
        json!(product_listing_id.to_string()),
        context_before_body["productListingId"]
    );
    assert!(context_before_body["auction"].is_null());
    assert_eq!(
        json!(false),
        context_before_body["auctionContextOverrideActive"]
    );

    let corrected = client
        .post(&correction_url)
        .bearer_auth(String::from(token.clone()))
        .header(reqwest::header::IF_MATCH, initial_etag.clone())
        .json(&json!({
            "expectedCurrentAuctionId": null,
            "replacement": null,
            "reason": correction_reason,
        }))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to correct ProductListing auction context: {error}")
        });
    let corrected_etag = corrected
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let corrected_status = corrected.status();
    let corrected_body = corrected
        .text()
        .await
        .unwrap_or_else(|error| panic!("failed to read correction response: {error}"));
    assert_eq!(
        reqwest::StatusCode::OK,
        corrected_status,
        "response body: {corrected_body}"
    );
    let corrected_body = serde_json::from_str::<serde_json::Value>(&corrected_body)
        .unwrap_or_else(|error| panic!("failed to decode correction response: {error}"));
    let corrected_etag =
        corrected_etag.unwrap_or_else(|| panic!("correction response has no ETag"));
    assert!(
        corrected_etag.starts_with("\"plv-")
            && corrected_etag.ends_with('"')
            && !corrected_etag.starts_with("W/"),
        "correction ETag must be strong: {corrected_etag}"
    );
    assert_eq!(
        context_before_body["expectedVersion"],
        corrected_body["expectedVersion"]
    );
    assert_ne!(
        context_before_body["expectedAuctionPolicyVersion"],
        corrected_body["expectedAuctionPolicyVersion"]
    );

    let active_context = client
        .get(&context_url)
        .bearer_auth(String::from(token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to get active ProductListing auction context: {error}")
        });
    let active_context_etag = active_context
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| panic!("active auction-context response has no ETag"));
    assert_eq!(corrected_etag, active_context_etag);
    let (active_context_status, active_context_body) = json_response(active_context).await;
    assert_eq!(reqwest::StatusCode::OK, active_context_status);
    assert_eq!(
        json!(true),
        active_context_body["auctionContextOverrideActive"]
    );

    let public_with_barrier =
        client
            .get(&public_listing_url)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("failed to get public ProductListing with barrier: {error}")
            });
    let (public_with_barrier_status, public_with_barrier_body) =
        json_response(public_with_barrier).await;
    assert_eq!(reqwest::StatusCode::OK, public_with_barrier_status);
    assert_eq!(public_before_body["item"], public_with_barrier_body["item"]);
    assert!(
        !public_with_barrier_body
            .to_string()
            .contains(correction_reason)
    );
    assert!(!public_with_barrier_body.to_string().contains("audit"));

    let history_with_barrier = client
        .get(&history_url)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to get ProductListing history with barrier: {error}")
        });
    let (history_with_barrier_status, history_with_barrier_body) =
        json_response(history_with_barrier).await;
    assert_eq!(reqwest::StatusCode::OK, history_with_barrier_status);
    assert_eq!(history_before_body, history_with_barrier_body);

    let stale_release = client
        .delete(&override_url)
        .bearer_auth(String::from(token.clone()))
        .header(reqwest::header::IF_MATCH, initial_etag)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to send stale auction-context release: {error}"));
    let (stale_release_status, stale_release_body) = json_response(stale_release).await;
    assert_problem(
        stale_release_status,
        &stale_release_body,
        reqwest::StatusCode::CONFLICT,
        "CONFLICT",
    );

    let malformed_release = client
        .delete(&override_url)
        .bearer_auth(String::from(token.clone()))
        .header(reqwest::header::IF_MATCH, format!("W/{corrected_etag}"))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to send malformed auction-context release: {error}")
        });
    let (malformed_release_status, malformed_release_body) = json_response(malformed_release).await;
    assert_problem(
        malformed_release_status,
        &malformed_release_body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_HEADER_VALUE",
    );

    let released = client
        .delete(&override_url)
        .bearer_auth(String::from(token.clone()))
        .header(reqwest::header::IF_MATCH, corrected_etag)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to release ProductListing auction context: {error}")
        });
    let released_etag = released
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (released_status, released_body) = json_response(released).await;
    assert_eq!(
        reqwest::StatusCode::OK,
        released_status,
        "response body: {released_body}"
    );
    let released_etag = released_etag.unwrap_or_else(|| panic!("release response has no ETag"));
    assert_eq!(
        context_before_body["expectedVersion"],
        released_body["expectedVersion"]
    );
    assert_ne!(
        corrected_body["expectedAuctionPolicyVersion"],
        released_body["expectedAuctionPolicyVersion"]
    );

    let released_context = client
        .get(&context_url)
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to get released ProductListing auction context: {error}")
        });
    let released_context_etag = released_context
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| panic!("released auction-context response has no ETag"));
    assert_eq!(released_etag, released_context_etag);
    let (released_context_status, released_context_body) = json_response(released_context).await;
    assert_eq!(reqwest::StatusCode::OK, released_context_status);
    assert_eq!(
        json!(false),
        released_context_body["auctionContextOverrideActive"]
    );
    assert_eq!(
        context_before_body["expectedVersion"],
        released_context_body["expectedVersion"]
    );

    let public_after_release =
        client
            .get(&public_listing_url)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("failed to get public ProductListing after release: {error}")
            });
    let (public_after_release_status, public_after_release_body) =
        json_response(public_after_release).await;
    assert_eq!(reqwest::StatusCode::OK, public_after_release_status);
    assert_eq!(
        public_before_body["item"],
        public_after_release_body["item"]
    );

    let history_after_release = client
        .get(&history_url)
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to get ProductListing history after release: {error}")
        });
    let (history_after_release_status, history_after_release_body) =
        json_response(history_after_release).await;
    assert_eq!(reqwest::StatusCode::OK, history_after_release_status);
    assert_eq!(history_before_body, history_after_release_body);
}
