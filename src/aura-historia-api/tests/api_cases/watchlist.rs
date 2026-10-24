use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};

use api_support::{
    assert_problem, json_response, seed_access_token_for, seed_active_watchlist_entries,
    seed_inactive_watchlist_entry, seed_product, seed_user, seed_user_with_tier,
};

async fn withdraw_product(product_listing_id: ProductListingId) {
    let pool = get_postgres_client().await;
    sqlx::query(
        "UPDATE product_listings SET lifecycle = 'WITHDRAWN', availability = NULL WHERE product_listing_id = $1",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to withdraw product fixture: {error}"));
}
use product_listing_core::product_listing_id::ProductListingId;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use user_core::{access_token::Scope, tier::UserTier, user_id::UserId};

static SINGLE_CONNECTION_WATCHLIST_API: test_api::AuraHistoriaApi =
    test_api::AuraHistoriaApi::new(api_support::watchlist_api_app_with_single_connection_pool);

fn assert_watchlist_product(
    body: &serde_json::Value,
    product_listing_id: ProductListingId,
    watching: bool,
    notifications: bool,
) {
    assert_eq!(Some(2), body.as_object().map(|object| object.len()));
    assert_eq!(
        serde_json::json!(product_listing_id.to_string()),
        body["item"]["productListingId"]
    );
    assert_eq!(
        serde_json::json!({"watching": watching, "notifications": notifications}),
        body["userState"]["watchlist"]
    );
    // Watchlist PATCH tokens hold no notification or saved-search read scope.
    for field in ["notification", "searchFilter"] {
        assert!(
            body["userState"].get(field).is_none(),
            "undisclosed {field}: {body}"
        );
    }
}

async fn watchlist_entry_snapshot(
    user_id: UserId,
    product_listing_id: ProductListingId,
) -> serde_json::Value {
    sqlx::query_scalar(
        "SELECT to_jsonb(entry) FROM product_listing_watchlist entry WHERE user_id = $1 AND product_listing_id = $2",
    )
    .bind(user_id.as_uuid())
    .bind(product_listing_id.as_uuid())
    .fetch_one(&get_postgres_client().await)
    .await
    .unwrap_or_else(|error| panic!("failed to read watchlist fixture: {error}"))
}

async fn seed_product_presentation(product_listing_id: ProductListingId) {
    let pool = get_postgres_client().await;
    sqlx::query(
        "UPDATE product_listings SET title_text = 'Watchlist cabinet', title_language = 'en', description_text = 'English cabinet description', description_language = 'en', price_kind = 'MONETARY', price_amount = 10000, price_currency = 'EUR', product_images = $2 WHERE product_listing_id = $1",
    )
    .bind(product_listing_id.as_uuid())
    .bind(serde_json::json!([{ "url": "https://api-acceptance.example/cabinet.jpg" }]))
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to seed watchlist product presentation: {error}"));
    sqlx::query(
        "INSERT INTO product_listing_translations (product_listing_id, source_event_id, language, title, description) SELECT product_listing_id, content_source_event_id, 'de', 'Watchlist-Schrank', 'Deutsche Schrankbeschreibung' FROM product_listings WHERE product_listing_id = $1",
    )
    .bind(product_listing_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to seed watchlist product translation: {error}"));
    sqlx::query(
        "INSERT INTO product_listing_content_assessments (product_listing_id, source_event_id, decision, category) SELECT product_listing_id, content_source_event_id, 'ALLOWED', NULL FROM product_listings WHERE product_listing_id = $1",
    )
    .bind(product_listing_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to seed watchlist product assessment: {error}"));
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_add_product_to_watchlist_when_authenticated() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_listing_id = seed_product().await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({
            "productListingId": product_listing_id.to_string(),
            "notifications": true
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create watchlist API: {error}"));
    let location = response.headers().get(reqwest::header::LOCATION).cloned();
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::CREATED, status);
    assert!(location.is_none());
    assert_eq!(serde_json::json!(user_id.to_string()), body["userId"]);
    assert_eq!(
        serde_json::json!(product_listing_id.to_string()),
        body["productListingId"]
    );
    assert!(
        body["productListingId"]
            .as_str()
            .is_some_and(|value| value.starts_with("pl_"))
    );
    assert_eq!(serde_json::json!(true), body["notifications"]);
    assert_eq!(serde_json::json!("ACTIVE"), body["state"]);
    assert!(body.get("item").is_none());
    assert!(body.get("userState").is_none());
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_return_product_listing_not_found_when_watching_missing_product() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"productListingId": ProductListingId::new().to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to watch missing product API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::NOT_FOUND,
        "PRODUCT_LISTING_NOT_FOUND",
    );
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_return_product_listing_unavailable_when_watching_withdrawn_product() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_listing_id = seed_product().await;
    withdraw_product(product_listing_id).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to watch withdrawn product API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::CONFLICT,
        "PRODUCT_LISTING_UNAVAILABLE",
    );
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_leave_existing_watchlist_entry_unchanged_when_watching_duplicate() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_listing_id = seed_product().await;
    let client = reqwest::Client::new();

    let created = client
        .post(format!("{}/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string(), "notifications": false}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create watchlist API: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, created.status());

    let duplicate = client
        .post(format!("{}/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string(), "notifications": true}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to duplicate watchlist API: {error}"));
    let (status, body) = json_response(duplicate).await;
    assert_problem(status, &body, reqwest::StatusCode::CONFLICT, "CONFLICT");

    let pool = get_postgres_client().await;
    let (notifications, state): (bool, String) = sqlx::query_as(
        "SELECT notifications, state FROM product_listing_watchlist WHERE user_id = $1 AND product_listing_id = $2",
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(uuid::Uuid::from(product_listing_id))
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to read duplicate watchlist entry: {error}"));
    assert!(!notifications);
    assert_eq!("ACTIVE", state);
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_list_current_user_watchlist() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistRead, Scope::WatchlistWrite]),
    )
    .await;
    let product_listing_id = seed_product().await;
    let client = reqwest::Client::new();
    let created = client
        .post(format!("{}/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create watchlist API: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, created.status());

    let response = client
        .get(format!("{}/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list watchlist API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(1), body["size"]);
    assert_eq!(
        serde_json::json!(product_listing_id.to_string()),
        body["items"][0]["item"]["productListingId"]
    );
    assert_eq!(
        "CURRENT",
        body["items"][0]["item"]["pricing"]["valuation"]["type"]
    );

    withdraw_product(product_listing_id).await;

    let response = client
        .get(format!("{}/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list withdrawn watchlist API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(
        serde_json::json!(product_listing_id.to_string()),
        body["items"][0]["item"]["productListingId"]
    );
    assert_eq!(
        serde_json::json!("WITHDRAWN"),
        body["items"][0]["item"]["lifecycle"]
    );
    assert_eq!(
        serde_json::json!(true),
        body["items"][0]["userState"]["watchlist"]["watching"]
    );
    assert_eq!(
        serde_json::json!(true),
        body["items"][0]["userState"]["watchlist"]["notifications"]
    );
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_update_watchlist_notifications() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite, Scope::WatchlistRead]),
    )
    .await;
    let product_listing_id = seed_product().await;
    let client = reqwest::Client::new();
    let created = client
        .post(format!("{}/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create watchlist API: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, created.status());

    let response = client
        .patch(format!(
            "{}/v1/me/watchlist/{}",
            AURA_API.base_url(),
            product_listing_id
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"notifications": false}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch watchlist API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_watchlist_product(&body, product_listing_id, true, false);
    assert_eq!(serde_json::json!("ACTIVE"), body["item"]["lifecycle"]);
    assert_eq!("CURRENT", body["item"]["pricing"]["valuation"]["type"]);
    assert_eq!(
        serde_json::json!(false),
        watchlist_entry_snapshot(user_id, product_listing_id).await["notifications"]
    );
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_deactivate_and_reactivate_watchlist_entry_for_active_product() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite, Scope::WatchlistRead]),
    )
    .await;
    let product_listing_id = seed_inactive_watchlist_entry(user_id).await;
    let client = reqwest::Client::new();

    for (patch, state, watching, notifications) in [
        (serde_json::json!({"state": "ACTIVE"}), "ACTIVE", true, true),
        (
            serde_json::json!({"state": "INACTIVE_BY_USER", "notifications": false}),
            "INACTIVE_BY_USER",
            false,
            false,
        ),
        (
            serde_json::json!({"state": "ACTIVE"}),
            "ACTIVE",
            true,
            false,
        ),
    ] {
        let response = client
            .patch(format!(
                "{}/v1/me/watchlist/{product_listing_id}",
                AURA_API.base_url()
            ))
            .bearer_auth(String::from(token.clone()))
            .json(&patch)
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to change watchlist state API: {error}"));
        let (status, body) = json_response(response).await;

        assert_eq!(reqwest::StatusCode::OK, status, "response body: {body}");
        assert_watchlist_product(&body, product_listing_id, watching, notifications);
        let entry = watchlist_entry_snapshot(user_id, product_listing_id).await;
        assert_eq!(serde_json::json!(state), entry["state"]);
        assert_eq!(serde_json::json!(notifications), entry["notifications"]);
    }
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, &SINGLE_CONNECTION_WATCHLIST_API], isolated = true)]
async fn should_reactivate_watchlist_with_shipping_on_single_connection_pool_without_deadlock() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([
            Scope::WatchlistWrite,
            Scope::WatchlistRead,
            Scope::UsersRead,
        ]),
    )
    .await;
    let product_listing_id = seed_inactive_watchlist_entry(user_id).await;
    seed_product_presentation(product_listing_id).await;
    sqlx::query("INSERT INTO user_browsing_destinations (user_id, country) VALUES ($1, 'DE')")
        .bind(user_id.as_uuid())
        .execute(&get_postgres_client().await)
        .await
        .unwrap_or_else(|error| panic!("failed to seed shipping destination: {error}"));
    let before = watchlist_entry_snapshot(user_id, product_listing_id).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/v1/me/watchlist/{product_listing_id}",
            SINGLE_CONNECTION_WATCHLIST_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .timeout(std::time::Duration::from_secs(10))
        .json(&serde_json::json!({"state": "ACTIVE", "notifications": false}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("single-connection watchlist reactivation failed: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status, "response body: {body}");
    assert_watchlist_product(&body, product_listing_id, true, false);
    // The single-listing write response uses detail shipping. The listing asserts no
    // terms, so only the private destination assessment is disclosed.
    assert!(body["item"].get("shipping").is_none());
    assert_eq!(
        "DE",
        body["userState"]["shipping"]["destination"]["country"]
    );
    assert!(body["userState"]["contentVisibility"].is_object());
    let entry = watchlist_entry_snapshot(user_id, product_listing_id).await;
    assert_eq!("ACTIVE", entry["state"]);
    assert_eq!(serde_json::json!(false), entry["notifications"]);
    assert_eq!(serde_json::json!(2), entry["version"]);
    assert!(entry["active_since"].is_string());
    assert!(entry["notifications_enabled_since"].is_null());
    assert_eq!(before["created"], entry["created"]);
    assert_ne!(before["updated"], entry["updated"]);
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, &SINGLE_CONNECTION_WATCHLIST_API], isolated = true)]
async fn should_not_commit_watchlist_patch_when_shipping_fails_with_single_connection_pool() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite, Scope::UsersRead]),
    )
    .await;
    let product_listing_id = seed_inactive_watchlist_entry(user_id).await;
    seed_product_presentation(product_listing_id).await;
    // Details/pricing remain valid; only the real shipping decoder rejects this stored object.
    sqlx::query("UPDATE product_listings SET delivery_configuration = '{}'::jsonb WHERE product_listing_id = $1")
        .bind(product_listing_id.as_uuid())
        .execute(&get_postgres_client().await)
        .await
        .unwrap_or_else(|error| panic!("failed to seed invalid shipping configuration: {error}"));
    let before = watchlist_entry_snapshot(user_id, product_listing_id).await;
    let client = reqwest::Client::new();

    for patch in [
        serde_json::json!({"state": "ACTIVE", "notifications": false}),
        serde_json::json!({"notifications": false}),
        serde_json::json!({}),
    ] {
        let response = client
            .patch(format!(
                "{}/v1/me/watchlist/{product_listing_id}",
                SINGLE_CONNECTION_WATCHLIST_API.base_url()
            ))
            .bearer_auth(String::from(token.clone()))
            .timeout(std::time::Duration::from_secs(10))
            .json(&patch)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("failed to patch watchlist with invalid shipping: {error}")
            });
        let (status, body) = json_response(response).await;

        assert_problem(
            status,
            &body,
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "SHIPPING_STORAGE_ERROR",
        );
        assert_eq!(
            before,
            watchlist_entry_snapshot(user_id, product_listing_id).await,
            "shipping failure must preserve all watchlist fields, version and timestamps for {patch}"
        );
    }
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_return_current_product_for_empty_watchlist_patch_without_changing_entry() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite, Scope::WatchlistRead]),
    )
    .await;
    let product_listing_id = seed_inactive_watchlist_entry(user_id).await;
    let before = watchlist_entry_snapshot(user_id, product_listing_id).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/v1/me/watchlist/{product_listing_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to send empty watchlist patch API: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status, "response body: {body}");
    assert_watchlist_product(&body, product_listing_id, false, true);
    assert_eq!(
        before,
        watchlist_entry_snapshot(user_id, product_listing_id).await
    );
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_present_watchlist_patch_product_in_requested_language_and_currency() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite, Scope::WatchlistRead]),
    )
    .await;
    let product_listing_id = seed_inactive_watchlist_entry(user_id).await;
    seed_product_presentation(product_listing_id).await;
    let client = reqwest::Client::new();

    for (query, language, title, description, currency, amount) in [
        (
            "",
            "en",
            "Watchlist cabinet",
            "English cabinet description",
            "EUR",
            10000,
        ),
        (
            "?language=de&currency=USD",
            "de",
            "Watchlist-Schrank",
            "Deutsche Schrankbeschreibung",
            "USD",
            12500,
        ),
    ] {
        let response = client
            .patch(format!(
                "{}/v1/me/watchlist/{product_listing_id}{query}",
                AURA_API.base_url()
            ))
            .bearer_auth(String::from(token.clone()))
            .json(&serde_json::json!({"notifications": false}))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to present watchlist patch API: {error}"));
        let (status, body) = json_response(response).await;

        assert_eq!(reqwest::StatusCode::OK, status, "response body: {body}");
        assert_watchlist_product(&body, product_listing_id, false, false);
        assert_eq!(
            serde_json::json!({"text": title, "language": language}),
            body["item"]["title"]
        );
        assert_eq!(
            serde_json::json!({"text": description, "language": language}),
            body["item"]["description"]
        );
        assert_eq!(
            serde_json::json!({"text": "Watchlist cabinet", "language": "en"}),
            body["item"]["productTitle"]
        );
        assert_eq!(
            serde_json::json!({"type": "MONETARY", "currency": "EUR", "amount": 10000}),
            body["item"]["pricing"]["source"]["price"]
        );
        assert_eq!(
            serde_json::json!({"type": "MONETARY", "currency": currency, "amount": amount}),
            body["item"]["pricing"]["display"]["price"]
        );
        assert_eq!("CURRENT", body["item"]["pricing"]["valuation"]["type"]);
        assert!(body["item"].get("shipping").is_none());
    }
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_return_product_listing_unavailable_when_reactivating_withdrawn_product() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_listing_id = seed_inactive_watchlist_entry(user_id).await;
    withdraw_product(product_listing_id).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/v1/me/watchlist/{product_listing_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"state": "ACTIVE"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reactivate withdrawn product API: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::CONFLICT,
        "PRODUCT_LISTING_UNAVAILABLE",
    );
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_allow_notification_only_and_deactivation_updates_for_withdrawn_product() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite, Scope::WatchlistRead]),
    )
    .await;
    let product_listing_id = seed_product().await;
    let client = reqwest::Client::new();
    let created = client
        .post(format!("{}/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create watchlist API: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, created.status());
    withdraw_product(product_listing_id).await;

    let notifications = client
        .patch(format!(
            "{}/v1/me/watchlist/{product_listing_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"notifications": false}))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to update notifications for withdrawn product API: {error}")
        });
    let (status, body) = json_response(notifications).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_watchlist_product(&body, product_listing_id, true, false);
    assert_eq!(serde_json::json!("WITHDRAWN"), body["item"]["lifecycle"]);
    assert!(body["item"]["availability"].is_null());
    assert!(body["item"].get("shipping").is_none());
    assert!(body["userState"].get("shipping").is_none());

    let deactivated = client
        .patch(format!(
            "{}/v1/me/watchlist/{product_listing_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"state": "INACTIVE_BY_USER"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to deactivate withdrawn product API: {error}"));
    let (status, body) = json_response(deactivated).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_watchlist_product(&body, product_listing_id, false, false);
    assert_eq!(serde_json::json!("WITHDRAWN"), body["item"]["lifecycle"]);
    assert!(body["item"]["availability"].is_null());
    assert!(body["item"].get("shipping").is_none());
    assert!(body["userState"].get("shipping").is_none());
    let entry = watchlist_entry_snapshot(user_id, product_listing_id).await;
    assert_eq!(serde_json::json!("INACTIVE_BY_USER"), entry["state"]);
    assert_eq!(serde_json::json!(false), entry["notifications"]);
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_preserve_hidden_product_redaction_when_updating_watchlist() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite, Scope::WatchlistRead]),
    )
    .await;
    let product_listing_id = seed_inactive_watchlist_entry(user_id).await;
    seed_product_presentation(product_listing_id).await;
    let pool = get_postgres_client().await;
    let filter_id = search_filter_core::user_search_filter_id::UserSearchFilterId::new();
    sqlx::query(
        "INSERT INTO search_filters (user_search_filter_id, user_id, name, notifications, state, search, language, currency, matching_evidence_policy) VALUES ($1, $2, 'Hidden watchlist alerts', true, 'ACTIVE', '{}', 'en', 'EUR', 'TEXT_ONLY')",
    )
    .bind(filter_id.as_uuid())
    .bind(user_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to seed hidden watchlist search filter: {error}"));

    for position in 0_i64..11 {
        let matched_product_listing_id = if position == 10 {
            product_listing_id
        } else {
            seed_product().await
        };
        sqlx::query(
            "INSERT INTO search_filter_matches (user_id, user_search_filter_id, product_listing_id, origin_event_id, user_search_filter_name, created) SELECT $1, $2, product_listing_id, current_event_id, 'Hidden watchlist alerts', (date_trunc('month', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC') + ($4 * interval '1 second') FROM product_listings WHERE product_listing_id = $3",
        )
        .bind(user_id.as_uuid())
        .bind(filter_id.as_uuid())
        .bind(matched_product_listing_id.as_uuid())
        .bind(position)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed hidden watchlist search match: {error}"));
    }
    let client = reqwest::Client::new();

    for (patch, watching) in [
        (
            serde_json::json!({"state": "ACTIVE", "notifications": false}),
            true,
        ),
        (serde_json::json!({"state": "INACTIVE_BY_USER"}), false),
    ] {
        let response = client
            .patch(format!(
                "{}/v1/me/watchlist/{product_listing_id}?language=de&currency=USD",
                AURA_API.base_url()
            ))
            .bearer_auth(String::from(token.clone()))
            .json(&patch)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("failed to patch hidden watchlist product API: {error}")
            });
        let (status, body) = json_response(response).await;

        assert_eq!(reqwest::StatusCode::OK, status, "response body: {body}");
        // Redaction still applies although the saved-search state stays undisclosed.
        assert_watchlist_product(&body, product_listing_id, watching, false);
        assert!(body["item"].get("productListingTitleSlugId").is_none());
        assert!(body["item"].get("productTitle").is_none());
        assert!(body["item"].get("productDescription").is_none());
        assert!(body["item"].get("description").is_none());
        assert_eq!(
            serde_json::json!({"text": "Versteckter Produkttitel", "language": "de"}),
            body["item"]["title"]
        );
        assert_eq!(serde_json::json!("Hidden"), body["item"]["source"]["name"]);
        for field in ["url", "viewUrl"] {
            assert_eq!(
                serde_json::json!("https://aura-historia.com/pricing"),
                body["item"][field]
            );
        }
        assert_eq!(serde_json::json!({}), body["item"]["pricing"]["source"]);
        assert_eq!(serde_json::json!({}), body["item"]["pricing"]["display"]);
        assert_eq!(serde_json::json!([]), body["item"]["images"]);
        assert!(body["item"]["availability"].is_null());
        assert!(body["item"].get("shipping").is_none());
        assert!(body["userState"].get("shipping").is_none());
    }
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_delete_watchlist_entry() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_listing_id = seed_product().await;
    let client = reqwest::Client::new();
    let created = client
        .post(format!("{}/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create watchlist API: {error}"));
    assert_eq!(reqwest::StatusCode::CREATED, created.status());

    let response = client
        .delete(format!(
            "{}/v1/me/watchlist/{}",
            AURA_API.base_url(),
            product_listing_id
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete watchlist API: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_reject_noncanonical_product_listing_id_in_watchlist_path() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_listing_id = ProductListingId::new();

    for invalid_id in [
        listing_source_core::ListingSourceId::new().to_string(),
        product_listing_id.as_uuid().to_string(),
        "pl_not-a-typeid".to_owned(),
    ] {
        let response = reqwest::Client::new()
            .patch(format!(
                "{}/v1/me/watchlist/{invalid_id}",
                AURA_API.base_url()
            ))
            .bearer_auth(String::from(token.clone()))
            .json(&serde_json::json!({"notifications": true}))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to patch invalid watchlist API: {error}"));
        let (status, body) = json_response(response).await;

        assert_problem(
            status,
            &body,
            reqwest::StatusCode::BAD_REQUEST,
            "INVALID_OBJECT_ID",
        );
    }
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_reject_noncanonical_product_listing_id_in_watchlist_body() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    let product_listing_id = ProductListingId::new();

    for invalid_id in [
        listing_source_core::ListingSourceId::new().to_string(),
        product_listing_id.as_uuid().to_string(),
        "pl_not-a-typeid".to_owned(),
    ] {
        let response = reqwest::Client::new()
            .post(format!("{}/v1/me/watchlist", AURA_API.base_url()))
            .bearer_auth(String::from(token.clone()))
            .json(&serde_json::json!({ "productListingId": invalid_id }))
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to post invalid watchlist API: {error}"));
        let (status, body) = json_response(response).await;

        assert_problem(
            status,
            &body,
            reqwest::StatusCode::BAD_REQUEST,
            "INVALID_OBJECT_ID",
        );
    }
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_return_not_found_when_updating_missing_watchlist_entry() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/v1/me/watchlist/{}",
            AURA_API.base_url(),
            ProductListingId::new()
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

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_reject_watchlist_create_at_free_tier_quota() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    seed_active_watchlist_entries(user_id, 20).await;
    let product_listing_id = seed_product().await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create over quota: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNPROCESSABLE_ENTITY,
        "WATCHLIST_QUOTA_EXCEEDED",
    );
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_serialize_concurrent_watchlist_creates_at_free_quota() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    seed_active_watchlist_entries(user_id, 19).await;
    let first_product_listing_id = seed_product().await;
    let second_product_listing_id = seed_product().await;
    let client = reqwest::Client::new();

    let first_request = client
        .post(format!("{}/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({ "productListingId": first_product_listing_id }))
        .send();
    let second_request = client
        .post(format!("{}/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({ "productListingId": second_product_listing_id }))
        .send();

    let (first, second) = tokio::join!(first_request, second_request);
    let mut statuses = [
        first
            .unwrap_or_else(|error| panic!("first concurrent create failed: {error}"))
            .status(),
        second
            .unwrap_or_else(|error| panic!("second concurrent create failed: {error}"))
            .status(),
    ];
    statuses.sort();

    assert_eq!(
        [
            reqwest::StatusCode::CREATED,
            reqwest::StatusCode::UNPROCESSABLE_ENTITY,
        ],
        statuses
    );
    let pool = get_postgres_client().await;
    let active_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_watchlist WHERE user_id = $1 AND state = 'ACTIVE'",
    )
    .bind(uuid::Uuid::from(user_id))
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to count active watchlist entries: {error}"));
    assert_eq!(20, active_count);
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_allow_ultimate_tier_to_create_beyond_free_quota() {
    let user_id = seed_user_with_tier("USER", UserTier::Ultimate).await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    seed_active_watchlist_entries(user_id, 20).await;
    let product_listing_id = seed_product().await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/me/watchlist", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"productListingId": product_listing_id.to_string()}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create unlimited watchlist entry: {error}"));

    assert_eq!(reqwest::StatusCode::CREATED, response.status());
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_reject_free_tier_watchlist_reactivation_at_quota() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    seed_active_watchlist_entries(user_id, 20).await;
    let product_listing_id = seed_inactive_watchlist_entry(user_id).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/v1/me/watchlist/{}",
            AURA_API.base_url(),
            product_listing_id
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"state": "ACTIVE"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reactivate over quota: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNPROCESSABLE_ENTITY,
        "WATCHLIST_QUOTA_EXCEEDED",
    );
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_serialize_concurrent_watchlist_reactivations_at_free_quota() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite]),
    )
    .await;
    seed_active_watchlist_entries(user_id, 19).await;
    let first_product_listing_id = seed_inactive_watchlist_entry(user_id).await;
    let second_product_listing_id = seed_inactive_watchlist_entry(user_id).await;
    let client = reqwest::Client::new();

    let first_request = client
        .patch(format!(
            "{}/v1/me/watchlist/{first_product_listing_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({ "state": "ACTIVE" }))
        .send();
    let second_request = client
        .patch(format!(
            "{}/v1/me/watchlist/{second_product_listing_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({ "state": "ACTIVE" }))
        .send();

    let (first, second) = tokio::join!(first_request, second_request);
    let mut statuses = [
        first
            .unwrap_or_else(|error| panic!("first concurrent reactivation failed: {error}"))
            .status(),
        second
            .unwrap_or_else(|error| panic!("second concurrent reactivation failed: {error}"))
            .status(),
    ];
    statuses.sort();

    assert_eq!(
        [
            reqwest::StatusCode::OK,
            reqwest::StatusCode::UNPROCESSABLE_ENTITY
        ],
        statuses
    );
    let pool = get_postgres_client().await;
    let active_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_watchlist WHERE user_id = $1 AND state = 'ACTIVE'",
    )
    .bind(uuid::Uuid::from(user_id))
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to count active watchlist entries: {error}"));
    assert_eq!(20, active_count);
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_allow_ultimate_tier_watchlist_reactivation_beyond_free_quota() {
    let user_id = seed_user_with_tier("USER", UserTier::Ultimate).await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::WatchlistWrite, Scope::WatchlistRead]),
    )
    .await;
    seed_active_watchlist_entries(user_id, 20).await;
    let product_listing_id = seed_inactive_watchlist_entry(user_id).await;

    let response = reqwest::Client::new()
        .patch(format!(
            "{}/v1/me/watchlist/{}",
            AURA_API.base_url(),
            product_listing_id
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({"state": "ACTIVE"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to reactivate unlimited watchlist entry: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_watchlist_product(&body, product_listing_id, true, true);
}

#[aura_integration_test(services = [crate::api_support::API_TEST_STATE, BUSINESS_SCHEMA, OPENSEARCH, &AURA_API], isolated = true)]
async fn should_require_auth_for_watchlist() {
    let response = reqwest::Client::new()
        .get(format!("{}/v1/me/watchlist", AURA_API.base_url()))
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
