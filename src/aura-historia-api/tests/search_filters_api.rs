mod api_support;

use api_support::{assert_problem, json_response, seed_access_token_for, seed_product, seed_user};
use product_listing_core::product_id::ProductId;
use test_api::{
    AuraHistoriaApi, IntegrationTestService, OpenSearch, Postgres, aura_integration_test,
    get_postgres_client,
};
use user_core::access_token::{RawAccessToken, Scope};

const BUSINESS_SCHEMA: Postgres = Postgres::new_schema_once("migrations");
const OPENSEARCH: OpenSearch = OpenSearch();

static AURA_API: AuraHistoriaApi = AuraHistoriaApi::new(api_support::aura_api_app);

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_owned_search_filter() {
    let user_id = seed_user("USER").await;
    let token = search_filters_token(user_id).await;
    let response = reqwest::Client::new()
        .post(format!("{}/api/v1/me/search-filters", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&search_filter_body())
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create search filter: {error}"));
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let content_language = response
        .headers()
        .get(reqwest::header::CONTENT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let has_last_modified = response
        .headers()
        .contains_key(reqwest::header::LAST_MODIFIED);
    let (status, body) = json_response(response).await;
    let filter_id = search_filter_id(&body);

    assert_eq!(reqwest::StatusCode::CREATED, status);
    assert_eq!(
        Some(format!("/api/v1/me/search-filters/{filter_id}")),
        location
    );
    assert_eq!(Some("en"), content_language.as_deref());
    assert!(has_last_modified);
    assert_eq!(serde_json::json!(user_id.to_string()), body["userId"]);
    assert_eq!(serde_json::json!("desk"), body["search"]["productQuery"][0]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_list_owned_search_filters() {
    let user_id = seed_user("USER").await;
    let token = search_filters_token(user_id).await;
    let client = reqwest::Client::new();
    let filter_id = create_search_filter(&client, &token).await;

    let response = client
        .get(format!("{}/api/v1/me/search-filters", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list search filters: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store"), cache_control.as_deref());
    assert_eq!(serde_json::json!(1), body["total"]);
    assert_eq!(
        serde_json::json!(filter_id),
        body["items"][0]["userSearchFilterId"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_get_owned_search_filter_with_representation_headers() {
    let user_id = seed_user("USER").await;
    let token = search_filters_token(user_id).await;
    let client = reqwest::Client::new();
    let filter_id = create_search_filter(&client, &token).await;

    let response = client
        .get(format!(
            "{}/api/v1/me/search-filters/{filter_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get search filter: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let content_language = response
        .headers()
        .get(reqwest::header::CONTENT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let has_last_modified = response
        .headers()
        .contains_key(reqwest::header::LAST_MODIFIED);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store"), cache_control.as_deref());
    assert_eq!(Some("en"), content_language.as_deref());
    assert!(has_last_modified);
    assert_eq!(serde_json::json!(filter_id), body["userSearchFilterId"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_update_owned_search_filter_without_replacing_unsupplied_search_fields() {
    let user_id = seed_user("USER").await;
    let token = search_filters_token(user_id).await;
    let client = reqwest::Client::new();
    let filter_id = create_search_filter(&client, &token).await;

    let response = client
        .patch(format!(
            "{}/api/v1/me/search-filters/{filter_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({
            "notifications": false,
            "search": { "productQuery": ["desk chair"] }
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to update search filter: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store"), cache_control.as_deref());
    assert_eq!(serde_json::json!(false), body["notifications"]);
    assert_eq!(
        serde_json::json!("desk chair"),
        body["search"]["productQuery"][0]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_delete_owned_search_filter() {
    let user_id = seed_user("USER").await;
    let token = search_filters_token(user_id).await;
    let client = reqwest::Client::new();
    let filter_id = create_search_filter(&client, &token).await;

    let response = client
        .delete(format!(
            "{}/api/v1/me/search-filters/{filter_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete search filter: {error}"));

    assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_list_search_filter_matches() {
    let user_id = seed_user("USER").await;
    let token = search_filters_token(user_id).await;
    let client = reqwest::Client::new();
    let filter_id = create_search_filter(&client, &token).await;
    let (product_id, _, _) = seed_search_filter_match(user_id, &filter_id).await;

    let response = client
        .get(format!(
            "{}/api/v1/me/search-filters/{filter_id}/matches?size=200",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list search filter matches: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store"), cache_control.as_deref());
    assert_eq!(serde_json::json!(100), body["size"]);
    assert_eq!(
        serde_json::json!(product_id.to_string()),
        body["items"][0]["item"]["productId"]
    );
    assert_eq!(
        "CURRENT",
        body["items"][0]["item"]["pricing"]["valuation"]["type"]
    );
    assert_eq!(
        serde_json::json!(false),
        body["items"][0]["userState"]["searchFilter"]["matchFeedback"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_accept_json_string_search_after_for_search_filter_matches() {
    let user_id = seed_user("USER").await;
    let token = search_filters_token(user_id).await;
    let client = reqwest::Client::new();
    let filter_id = create_search_filter(&client, &token).await;
    let (product_id, _, _) = seed_search_filter_match(user_id, &filter_id).await;
    let search_after =
        serde_json::json!(["1970-01-01T00:00:00Z", ProductId::new().to_string(),]).to_string();

    let response = client
        .get(format!(
            "{}/api/v1/me/search-filters/{filter_id}/matches",
            AURA_API.base_url()
        ))
        .query(&[("searchAfter", search_after)])
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| {
            panic!("failed to list search filter matches with cursor: {error}")
        });
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(
        serde_json::json!(product_id.to_string()),
        body["items"][0]["item"]["productId"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_serialize_concurrent_search_filter_creates_at_free_quota() {
    let user_id = seed_user("USER").await;
    let token = search_filters_token(user_id).await;
    let client = reqwest::Client::new();
    let first_request = client
        .post(format!("{}/api/v1/me/search-filters", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&search_filter_body())
        .send();
    let second_request = client
        .post(format!("{}/api/v1/me/search-filters", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .json(&search_filter_body())
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
        "SELECT count(*) FROM search_filters WHERE user_id = $1 AND state = 'ACTIVE'",
    )
    .bind(uuid::Uuid::from(user_id))
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to count active search filters: {error}"));
    assert_eq!(1, active_count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_serialize_concurrent_search_filter_reactivations_at_free_quota() {
    let user_id = seed_user("USER").await;
    let token = search_filters_token(user_id).await;
    let client = reqwest::Client::new();
    let first_filter_id = create_search_filter(&client, &token).await;
    let pool = get_postgres_client().await;
    let second_filter_id = uuid::Uuid::new_v4();

    sqlx::query(
        "UPDATE search_filters SET state = 'INACTIVE_BY_USER' WHERE user_search_filter_id = $1",
    )
    .bind(
        uuid::Uuid::parse_str(&first_filter_id)
            .unwrap_or_else(|error| panic!("invalid first search filter identifier: {error}")),
    )
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to inactivate first search filter: {error}"));
    sqlx::query(
        "INSERT INTO search_filters (user_search_filter_id, user_id, name, notifications, state, search, enhanced_search_description, embedding, language, currency, version, created, updated) SELECT $1, user_id, 'Second alerts', notifications, 'INACTIVE_BY_USER', search, enhanced_search_description, embedding, language, currency, version, created, updated FROM search_filters WHERE user_search_filter_id = $2",
    )
    .bind(second_filter_id)
    .bind(uuid::Uuid::parse_str(&first_filter_id).unwrap_or_else(|error| {
        panic!("invalid first search filter identifier: {error}")
    }))
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to seed second inactive search filter: {error}"));

    let first_request = client
        .patch(format!(
            "{}/api/v1/me/search-filters/{first_filter_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({ "state": "ACTIVE" }))
        .send();
    let second_request = client
        .patch(format!(
            "{}/api/v1/me/search-filters/{second_filter_id}",
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
    let active_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM search_filters WHERE user_id = $1 AND state = 'ACTIVE'",
    )
    .bind(uuid::Uuid::from(user_id))
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to count active search filters: {error}"));
    assert_eq!(1, active_count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_update_search_filter_match_feedback() {
    let user_id = seed_user("USER").await;
    let token = search_filters_token(user_id).await;
    let client = reqwest::Client::new();
    let filter_id = create_search_filter(&client, &token).await;
    let (product_id, _, _) = seed_search_filter_match(user_id, &filter_id).await;

    let response = client
        .patch(format!(
            "{}/api/v1/me/search-filters/{filter_id}/matches/{product_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .json(&serde_json::json!({ "feedback": true }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to update search filter match feedback: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let has_last_modified = response
        .headers()
        .contains_key(reqwest::header::LAST_MODIFIED);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("no-store"), cache_control.as_deref());
    assert!(has_last_modified);
    assert_eq!(serde_json::json!(product_id.to_string()), body["productId"]);
    assert_eq!(serde_json::json!(true), body["feedback"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_require_authentication_for_search_filter_routes() {
    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/me/search-filters", AURA_API.base_url()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list search filters without auth: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_search_filter_routes_without_required_scope() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(user_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/me/search-filters", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list search filters without scope: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_search_filter_identifier() {
    let user_id = seed_user("USER").await;
    let token = search_filters_token(user_id).await;

    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/me/search-filters/not-a-uuid",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get invalid search filter: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_UUID",
    );
}

async fn search_filters_token(user_id: user_core::user_id::UserId) -> RawAccessToken {
    seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::SearchFiltersWrite]),
    )
    .await
}

async fn create_search_filter(client: &reqwest::Client, token: &RawAccessToken) -> String {
    let response = client
        .post(format!("{}/api/v1/me/search-filters", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&search_filter_body())
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create search filter fixture: {error}"));
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::CREATED, status);
    search_filter_id(&body)
}

async fn seed_search_filter_match(
    user_id: user_core::user_id::UserId,
    filter_id: &str,
) -> (ProductId, String, String) {
    let product_id = seed_product().await;
    let pool = get_postgres_client().await;
    let (shop_id, shops_product_id, origin_event_id) =
        sqlx::query_as::<_, (uuid::Uuid, String, uuid::Uuid)>(
            "SELECT shop_id, shops_product_id, event_id FROM products WHERE product_id = $1",
        )
        .bind(uuid::Uuid::from(product_id))
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to read product match fixture: {error}"));
    let filter_id = uuid::Uuid::parse_str(filter_id)
        .unwrap_or_else(|error| panic!("invalid search filter fixture ID: {error}"));

    sqlx::query(
        "INSERT INTO search_filter_matches (user_id, user_search_filter_id, product_id, origin_event_id, user_search_filter_name, feedback) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(filter_id)
    .bind(uuid::Uuid::from(product_id))
    .bind(origin_event_id)
    .bind("Desk alerts")
    .bind(false)
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to seed search filter match: {error}"));

    (product_id, shop_id.to_string(), shops_product_id)
}

fn search_filter_body() -> serde_json::Value {
    serde_json::json!({
        "name": "Desk alerts",
        "search": {
            "language": "en",
            "currency": "EUR",
            "productQuery": ["desk"]
        }
    })
}

fn search_filter_id(body: &serde_json::Value) -> String {
    body["userSearchFilterId"]
        .as_str()
        .unwrap_or_else(|| panic!("search filter response is missing ID"))
        .to_owned()
}
