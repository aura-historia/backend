mod api_support;

use api_support::{json_response, seed_access_token_for, seed_partner_shop, seed_shop, seed_user};
use serde_json::json;

use test_api::{
    AuraHistoriaApi, DynamoDB, IntegrationTestService, OpenSearch, Postgres, aura_integration_test,
};
use user_core::access_token::Scope;

const BUSINESS_SCHEMA: Postgres = Postgres::new_schema_once("migrations");
const DYNAMODB: DynamoDB = DynamoDB();
const OPENSEARCH: OpenSearch = OpenSearch();
static AURA_API: AuraHistoriaApi = AuraHistoriaApi::new(api_support::aura_api_app);

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_synchronously_create_batch_and_return_partial_failure_keys() {
    let shop = seed_shop().await;
    let user_id = seed_user("USER").await;
    seed_partner_shop(user_id, shop.id()).await;
    let token = seed_access_token_for(
        user_id,
        std::collections::HashSet::from([Scope::ProductsWrite]),
    )
    .await;
    let body = json!([
        product("synchronous-product"),
        product("synchronous-product")
    ]);

    let response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/shops/{}/products",
            AURA_API.base_url(),
            shop.id()
        ))
        .bearer_auth(String::from(token))
        .json(&body)
        .send()
        .await;

    assert!(response.is_ok(), "failed to call partner products API");
    if let Ok(response) = response {
        let (status, body) = json_response(response).await;
        assert_eq!(reqwest::StatusCode::OK, status);
        assert_eq!(
            json!([{
                "shopId": shop.id().to_string(),
                "shopsProductId": "synchronous-product"
            }]),
            body
        );
    }
}

fn product(shops_product_id: &str) -> serde_json::Value {
    json!({
        "shopsProductId": shops_product_id,
        "title": { "text": "Synchronous Cabinet", "language": "en" },
        "description": { "text": "Created in the request transaction.", "language": "en" },
        "state": "LISTED",
        "url": "https://partner.example/products/synchronous-cabinet",
        "images": []
    })
}
