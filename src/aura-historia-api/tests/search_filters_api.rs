mod api_support;

use api_support::{assert_problem, json_response, seed_access_token_for, seed_user};

use test_api::{
    AuraHistoriaApi, DynamoDB, IntegrationTestService, Postgres, aura_integration_test,
};
use user_core::access_token::Scope;

const BUSINESS_SCHEMA: Postgres = Postgres::new_schema_once("migrations");
const DYNAMODB: DynamoDB = DynamoDB();
static AURA_API: AuraHistoriaApi = AuraHistoriaApi::new(api_support::aura_api_app);

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_create_list_get_patch_and_delete_owned_search_filter() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::SearchFiltersWrite]),
    )
    .await;
    let client = reqwest::Client::new();

    let created = client
        .post(format!("{}/api/v1/me/search-filters", AURA_API.base_url()))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({
            "name": "Desk alerts",
            "search": {
                "language": "en",
                "currency": "EUR",
                "productQuery": ["desk"]
            }
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to create search filter: {error}"));
    let location = created
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (status, body) = json_response(created).await;

    assert_eq!(reqwest::StatusCode::CREATED, status);
    let filter_id = body["userSearchFilterId"]
        .as_str()
        .unwrap_or_else(|| panic!("created response is missing search filter ID"));
    assert_eq!(
        Some(format!("/api/v1/me/search-filters/{filter_id}")),
        location
    );
    assert_eq!(serde_json::json!(user_id.to_string()), body["userId"]);
    assert_eq!(serde_json::json!("desk"), body["search"]["productQuery"][0]);

    let listed = client
        .get(format!(
            "{}/api/v1/me/search-filters?sort=created&order=desc",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list search filters: {error}"));
    let (status, body) = json_response(listed).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(1), body["total"]);
    assert_eq!(
        serde_json::json!(filter_id),
        body["items"][0]["userSearchFilterId"]
    );

    let patched = client
        .patch(format!(
            "{}/api/v1/me/search-filters/{filter_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token.clone()))
        .json(&serde_json::json!({
            "notifications": false,
            "search": { "shopName": ["Desk Shop"] }
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to patch search filter: {error}"));
    let (status, body) = json_response(patched).await;
    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(serde_json::json!(false), body["notifications"]);
    assert_eq!(
        serde_json::json!("Desk Shop"),
        body["search"]["shopName"][0]
    );
    assert_eq!(serde_json::json!("desk"), body["search"]["productQuery"][0]);

    let fetched = client
        .get(format!(
            "{}/api/v1/me/search-filters/{filter_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token.clone()))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get search filter: {error}"));
    assert_eq!(reqwest::StatusCode::OK, fetched.status());
    assert_eq!(
        Some("no-store"),
        fetched
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
    );

    let deleted = client
        .delete(format!(
            "{}/api/v1/me/search-filters/{filter_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to delete search filter: {error}"));
    assert_eq!(reqwest::StatusCode::NO_CONTENT, deleted.status());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, &AURA_API])]
async fn should_reject_search_filter_routes_without_required_scope() {
    let user_id = seed_user("USER").await;
    let token = seed_access_token_for(user_id, std::collections::HashSet::new()).await;

    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/me/search-filters", AURA_API.base_url()))
        .bearer_auth(String::from(token))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to list search filters: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(status, &body, reqwest::StatusCode::FORBIDDEN, "FORBIDDEN");
}
