mod api_support;

use api_support::{
    json_response, seed_access_token_for, seed_current_fx_snapshot, seed_partner_shop, seed_shop,
    seed_user,
};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::convert::Infallible;
use test_api::{
    AuraHistoriaApi, IntegrationTestService, OpenSearch, Postgres, aura_integration_test,
    get_postgres_client,
};
use user_core::access_token::Scope;

const BUSINESS_SCHEMA: Postgres = Postgres::new_schema_once("migrations");

const OPENSEARCH: OpenSearch = OpenSearch();
static AURA_API: AuraHistoriaApi = AuraHistoriaApi::new(api_support::aura_api_app);

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn assert_test_result(result: TestResult) {
    assert!(result.is_ok(), "{result:?}");
}

struct PartnerAuth {
    shop_id: String,
    token: String,
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_partner_product_batch_when_all_products_are_new() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;

        let response = send_json(
            reqwest::Method::POST,
            products_path(&auth.shop_id),
            Some(&auth.token),
            &json!([product("post-full-first"), product("post-full-second")]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::OK, status);
        assert_eq!(json!([]), body);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_duplicate_product_as_partial_create_failure() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;
        let duplicate_id = "post-duplicate";

        let response = send_json(
            reqwest::Method::POST,
            products_path(&auth.shop_id),
            Some(&auth.token),
            &json!([product(duplicate_id), product(duplicate_id)]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::OK, status);
        assert_eq!(
            json!([failure(&auth.shop_id, duplicate_id, "CONFLICT")]),
            body
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_update_existing_partner_product_batch() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;
        let product_listing_id = "patch-success";
        create_product(&auth, product_listing_id).await?;

        let response = send_json(
            reqwest::Method::PATCH,
            products_path(&auth.shop_id),
            Some(&auth.token),
            &json!([patch_product(product_listing_id, "SOLD_OUT")]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::OK, status);
        assert_eq!(json!([]), body);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_unrelated_partner_from_existing_product_update() -> TestResult {
    let result: TestResult = async {
        let owner = partner_auth(product_listings_write_scope()).await?;
        let product_listing_id = "patch-unrelated-partner";
        create_product(&owner, product_listing_id).await?;

        let unrelated_user_id = seed_user("USER").await;
        let unrelated_token = String::from(
            seed_access_token_for(unrelated_user_id, product_listings_write_scope()).await,
        );
        let response = send_json(
            reqwest::Method::PATCH,
            products_path(&owner.shop_id),
            Some(&unrelated_token),
            &json!([patch_product(product_listing_id, "SOLD_OUT")]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::FORBIDDEN, status);
        assert_eq!(json!("FORBIDDEN"), body["error"]);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_missing_product_as_partial_update_failure() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;
        let existing_id = "patch-partial-existing";
        let missing_id = "patch-partial-missing";
        create_product(&auth, existing_id).await?;

        let response = send_json(
            reqwest::Method::PATCH,
            products_path(&auth.shop_id),
            Some(&auth.token),
            &json!([
                patch_product(existing_id, "AVAILABLE"),
                patch_product(missing_id, "SOLD_OUT")
            ]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::OK, status);
        assert_eq!(
            json!([failure(
                &auth.shop_id,
                missing_id,
                "PRODUCT_LISTING_NOT_FOUND"
            )]),
            body
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_when_every_product_update_is_missing() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;

        let response = send_json(
            reqwest::Method::PATCH,
            products_path(&auth.shop_id),
            Some(&auth.token),
            &json!([patch_product("patch-all-missing", "SOLD_OUT")]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::NOT_FOUND, status);
        assert_eq!(json!("PRODUCT_LISTING_NOT_FOUND"), body["error"]);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_create_then_update_partner_product_with_upsert() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;
        let product_listing_id = "put-create-update";

        let created = send_json(
            reqwest::Method::PUT,
            products_path(&auth.shop_id),
            Some(&auth.token),
            &json!([product(product_listing_id)]),
        )
        .await?;
        let (created_status, created_body) = response_json(created).await?;
        assert_eq!(reqwest::StatusCode::OK, created_status);
        assert_eq!(json!([]), created_body);

        let updated = send_json(
            reqwest::Method::PUT,
            products_path(&auth.shop_id),
            Some(&auth.token),
            &json!([patch_product(product_listing_id, "SOLD_OUT")]),
        )
        .await?;
        let (updated_status, updated_body) = response_json(updated).await?;

        assert_eq!(reqwest::StatusCode::OK, updated_status);
        assert_eq!(json!([]), updated_body);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_unrelated_partner_from_existing_product_upsert() -> TestResult {
    let result: TestResult = async {
        let owner = partner_auth(product_listings_write_scope()).await?;
        let product_listing_id = "put-unrelated-partner";
        create_product(&owner, product_listing_id).await?;

        let unrelated_user_id = seed_user("USER").await;
        let unrelated_token = String::from(
            seed_access_token_for(unrelated_user_id, product_listings_write_scope()).await,
        );
        let response = send_json(
            reqwest::Method::PUT,
            products_path(&owner.shop_id),
            Some(&unrelated_token),
            &json!([patch_product(product_listing_id, "SOLD_OUT")]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::FORBIDDEN, status);
        assert_eq!(json!("FORBIDDEN"), body["error"]);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_delete_existing_partner_product_batch() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;
        let first_id = "delete-full-first";
        let second_id = "delete-full-second";
        create_products(&auth, &[first_id, second_id]).await?;

        let response = send_json(
            reqwest::Method::DELETE,
            products_path(&auth.shop_id),
            Some(&auth.token),
            &json!([delete_product(first_id), delete_product(second_id)]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::OK, status);
        assert_eq!(json!([]), body);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_unrelated_partner_from_existing_product_delete() -> TestResult {
    let result: TestResult = async {
        let owner = partner_auth(product_listings_write_scope()).await?;
        let product_listing_id = "delete-unrelated-partner";
        create_product(&owner, product_listing_id).await?;

        let unrelated_user_id = seed_user("USER").await;
        let unrelated_token = String::from(
            seed_access_token_for(unrelated_user_id, product_listings_write_scope()).await,
        );
        let response = send_json(
            reqwest::Method::DELETE,
            products_path(&owner.shop_id),
            Some(&unrelated_token),
            &json!([delete_product(product_listing_id)]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::FORBIDDEN, status);
        assert_eq!(json!("FORBIDDEN"), body["error"]);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_missing_product_as_partial_delete_failure() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;
        let existing_id = "delete-partial-existing";
        let missing_id = "delete-partial-missing";
        create_product(&auth, existing_id).await?;

        let response = send_json(
            reqwest::Method::DELETE,
            products_path(&auth.shop_id),
            Some(&auth.token),
            &json!([delete_product(existing_id), delete_product(missing_id)]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::OK, status);
        assert_eq!(
            json!([failure(
                &auth.shop_id,
                missing_id,
                "PRODUCT_LISTING_NOT_FOUND"
            )]),
            body
        );
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_not_found_when_every_product_delete_is_missing() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;

        let response = send_json(
            reqwest::Method::DELETE,
            products_path(&auth.shop_id),
            Some(&auth.token),
            &json!([delete_product("delete-all-missing")]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::NOT_FOUND, status);
        assert_eq!(json!("PRODUCT_LISTING_NOT_FOUND"), body["error"]);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_partner_product_batch_when_access_token_lacks_scope() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(HashSet::new()).await?;

        let response = send_json(
            reqwest::Method::POST,
            products_path(&auth.shop_id),
            Some(&auth.token),
            &json!([product("scope-less")]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::FORBIDDEN, status);
        assert_eq!(json!("FORBIDDEN"), body["error"]);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_unrelated_partner_from_product_batch() -> TestResult {
    let result: TestResult = async {
        let shop = seed_shop().await;
        let user_id = seed_user("USER").await;
        let token =
            String::from(seed_access_token_for(user_id, product_listings_write_scope()).await);

        let response = send_json(
            reqwest::Method::POST,
            products_path(&shop.id().to_string()),
            Some(&token),
            &json!([product("unrelated-partner")]),
        )
        .await?;
        let (status, body) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::FORBIDDEN, status);
        assert_eq!(json!("FORBIDDEN"), body["error"]);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_upsert_concurrently_without_returning_temporary_failure() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;
        let path = products_path(&auth.shop_id);
        let body = json!([product("concurrent-upsert")]);

        let (first, second) = tokio::join!(
            send_json(reqwest::Method::PUT, path.clone(), Some(&auth.token), &body,),
            send_json(reqwest::Method::PUT, path, Some(&auth.token), &body),
        );
        let (first_status, first_body) = response_json(first?).await?;
        let (second_status, second_body) = response_json(second?).await?;

        assert_eq!(reqwest::StatusCode::OK, first_status);
        assert_eq!(json!([]), first_body);
        assert_eq!(reqwest::StatusCode::OK, second_status);
        assert_eq!(json!([]), second_body);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_partner_product_batch_without_authorization() -> TestResult {
    let result: TestResult = async {
        let shop = seed_shop().await;

        let response = send_json(
            reqwest::Method::POST,
            products_path(&shop.id().to_string()),
            None,
            &json!([product("missing-authorization")]),
        )
        .await?;
        let (status, _) = response_json(response).await?;

        assert_eq!(reqwest::StatusCode::UNAUTHORIZED, status);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_not_expose_legacy_partner_product_item_delete_route() -> TestResult {
    let result: TestResult = async {
        let auth = partner_auth(product_listings_write_scope()).await?;

        let response = send_json(
            reqwest::Method::DELETE,
            format!("{}/legacy-item", products_path(&auth.shop_id)),
            Some(&auth.token),
            &json!({}),
        )
        .await?;

        assert_eq!(reqwest::StatusCode::NOT_FOUND, response.status());
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    assert_test_result(result);
}

async fn partner_auth(scopes: HashSet<Scope>) -> Result<PartnerAuth, Infallible> {
    let pool = get_postgres_client().await;
    seed_current_fx_snapshot(&pool).await;
    let shop = seed_shop().await;
    let user_id = seed_user("USER").await;
    seed_partner_shop(user_id, shop.id()).await;
    let token = String::from(seed_access_token_for(user_id, scopes).await);

    Ok(PartnerAuth {
        shop_id: shop.id().to_string(),
        token,
    })
}

async fn create_product(auth: &PartnerAuth, shop_listing_id: &str) -> TestResult {
    create_products(auth, &[shop_listing_id]).await
}

async fn create_products(auth: &PartnerAuth, shop_listing_ids: &[&str]) -> TestResult {
    let response = send_json(
        reqwest::Method::POST,
        products_path(&auth.shop_id),
        Some(&auth.token),
        &Value::Array(
            shop_listing_ids
                .iter()
                .map(|shop_listing_id| product(shop_listing_id))
                .collect(),
        ),
    )
    .await?;
    let (status, body) = response_json(response).await?;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(json!([]), body);
    Ok::<(), Box<dyn std::error::Error>>(())
}

async fn send_json(
    method: reqwest::Method,
    path: String,
    token: Option<&str>,
    body: &Value,
) -> Result<reqwest::Response, reqwest::Error> {
    let request =
        reqwest::Client::new().request(method, format!("{}{}", AURA_API.base_url(), path));
    let request = match token {
        Some(token) => request.bearer_auth(token),
        None => request,
    };

    request.json(body).send().await
}

async fn response_json(
    response: reqwest::Response,
) -> Result<(reqwest::StatusCode, Value), Infallible> {
    Ok(json_response(response).await)
}

fn product_listings_write_scope() -> HashSet<Scope> {
    HashSet::from([Scope::ProductListingsWrite])
}

fn products_path(shop_id: &str) -> String {
    format!("/api/v1/shops/{shop_id}/product-listings")
}

fn product(shop_listing_id: &str) -> Value {
    json!({
        "shopListingId": shop_listing_id,
        "title": { "text": "Synchronous Cabinet", "language": "en" },
        "description": { "text": "Created in the request transaction.", "language": "en" },
        "availability": "AVAILABLE",
        "url": format!("https://partner.example/product-listings/{shop_listing_id}"),
        "images": []
    })
}

fn patch_product(shop_listing_id: &str, availability: &str) -> Value {
    json!({
        "shopListingId": shop_listing_id,
        "availability": availability
    })
}

fn delete_product(shop_listing_id: &str) -> Value {
    json!({ "shopListingId": shop_listing_id })
}

fn failure(shop_id: &str, shop_listing_id: &str, error: &str) -> Value {
    json!({
        "shopId": shop_id,
        "shopListingId": shop_listing_id,
        "error": error
    })
}
