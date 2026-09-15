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
