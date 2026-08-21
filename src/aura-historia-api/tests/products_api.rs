mod api_support;

use api_support::{
    assert_problem, aura_api_app_with_failed_search_embedding, json_response, product_route_slugs,
    seed_current_fx_snapshot, seed_product,
};
use opensearch::IndexParts;
use serde_json::{Value, json};
use test_api::{
    AuraHistoriaApi, DynamoDB, IntegrationTestService, OpenSearch, Postgres, aura_integration_test,
    get_opensearch_client, get_postgres_client, refresh_index,
};
use time::{Duration, OffsetDateTime};

const BUSINESS_SCHEMA: Postgres = Postgres::new_schema_once("migrations");
const DYNAMODB: DynamoDB = DynamoDB();
const OPENSEARCH: OpenSearch = OpenSearch();
const PRODUCTS_INDEX: &str = "products";
static AURA_API: AuraHistoriaApi = AuraHistoriaApi::new(api_support::aura_api_app);
static AURA_API_WITH_FAILED_EMBEDDING: AuraHistoriaApi =
    AuraHistoriaApi::new(aura_api_app_with_failed_search_embedding);

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_get_product_details_by_id() {
    let product_id = seed_product().await;

    let response = get_json(format!("/api/v1/products/{product_id}")).await;
    let cache_control = response
        .0
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (status, body) = json_response(response.0).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(json!(product_id.to_string()), body["item"]["productId"]);
    assert_eq!("CURRENT", body["item"]["pricing"]["valuation"]["type"]);
    assert!(body["item"]["pricing"].get("source").is_some());
    assert!(body["item"]["pricing"].get("display").is_some());
    assert!(body["item"].get("price").is_none());
    assert!(body["item"].get("priceEstimateMin").is_none());
    assert!(body["item"].get("priceEstimateMax").is_none());
    assert!(body["item"].get("currency").is_none());
    assert!(body.get("userState").is_none());
    assert_eq!(
        Some("public, max-age=180, s-maxage=900".to_owned()),
        cache_control
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_get_product_details_by_slug() {
    let product_id = seed_product().await;
    let (shop_slug_id, product_slug_id) = product_route_slugs(product_id).await;

    let (response, _) = get_json(format!(
        "/api/v1/by-slug/shops/{shop_slug_id}/products/{product_slug_id}"
    ))
    .await;
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(json!(product_id.to_string()), body["item"]["productId"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_get_product_history_by_id() {
    let product_id = seed_product().await;

    let (response, _) = get_json(format!("/api/v1/products/{product_id}/history")).await;
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert!(body.as_array().is_some_and(|events| !events.is_empty()));
    assert_eq!(json!(product_id.to_string()), body[0]["productId"]);
    assert_eq!(
        Some("public, max-age=180, s-maxage=900".to_owned()),
        cache_control
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_get_product_history_by_slug() {
    let product_id = seed_product().await;
    let (shop_slug_id, product_slug_id) = product_route_slugs(product_id).await;

    let (response, _) = get_json(format!(
        "/api/v1/by-slug/shops/{shop_slug_id}/products/{product_slug_id}/history"
    ))
    .await;
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(json!(product_id.to_string()), body[0]["productId"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_return_pending_similar_products_by_id() {
    let product_id = seed_product().await;

    let (response, _) = get_json(format!("/api/v1/products/{product_id}/similar")).await;

    assert_eq!(reqwest::StatusCode::ACCEPTED, response.status());
    assert_eq!(
        Some(format!("/api/v1/products/{product_id}/similar")),
        response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned)
    );
    assert_eq!(
        Some("public, max-age=300, s-maxage=900"),
        response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_return_pending_similar_products_by_slug() {
    let product_id = seed_product().await;
    let (shop_slug_id, product_slug_id) = product_route_slugs(product_id).await;

    let (response, _) = get_json(format!(
        "/api/v1/by-slug/shops/{shop_slug_id}/products/{product_slug_id}/similar"
    ))
    .await;

    assert_eq!(reqwest::StatusCode::ACCEPTED, response.status());
    assert_eq!(
        Some(format!(
            "/api/v1/by-slug/shops/{shop_slug_id}/products/{product_slug_id}/similar"
        )),
        response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned)
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_page_product_search_without_duplicates_when_using_cursor() {
    let products = [
        search_document(
            "Price page cabinet",
            100,
            "AVAILABLE",
            "ACTIVE",
            "Price Shop",
            "2025-01-01T00:00:00Z",
        ),
        search_document(
            "Price page cabinet",
            200,
            "AVAILABLE",
            "ACTIVE",
            "Price Shop",
            "2025-01-02T00:00:00Z",
        ),
        search_document(
            "Price page cabinet",
            300,
            "AVAILABLE",
            "ACTIVE",
            "Price Shop",
            "2025-01-03T00:00:00Z",
        ),
        search_document(
            "Price page cabinet",
            400,
            "AVAILABLE",
            "ACTIVE",
            "Price Shop",
            "2025-01-04T00:00:00Z",
        ),
    ];
    index_search_documents(products.iter().map(|(_, document)| document.clone())).await;

    let (first_response, _) = get_json(
        "/api/v1/products?language=en&currency=USD&productQuery[0]=Price%20page%20cabinet&sort=created&order=asc&size=2".to_owned(),
    )
    .await;
    let cache_control = first_response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (first_status, first_body) = json_response(first_response).await;

    assert_eq!(reqwest::StatusCode::OK, first_status);
    assert_eq!(json!(4), first_body["total"]);
    assert_eq!(json!(2), first_body["size"]);
    assert_eq!(
        vec![products[0].0.clone(), products[1].0.clone()],
        product_ids(&first_body)
    );
    assert!(first_body["searchAfter"].is_object());
    assert!(first_body["searchAfter"]["fxRateId"].is_string());
    assert!(first_body["searchAfter"]["searchAfter"].is_array());
    assert_eq!(
        Some("public, max-age=60, s-maxage=300".to_owned()),
        cache_control
    );

    let search_after = serde_json::to_string(&first_body["searchAfter"])
        .unwrap_or_else(|error| panic!("failed to encode search cursor: {error}"));
    let (second_response, _) = get_json(format!(
        "/api/v1/products?language=en&currency=USD&productQuery[0]=Price%20page%20cabinet&sort=created&order=asc&size=2&searchAfter={}",
        url_encode(&search_after)
    ))
    .await;
    let (second_status, second_body) = json_response(second_response).await;

    assert_eq!(reqwest::StatusCode::OK, second_status);
    assert_eq!(
        vec![products[2].0.clone(), products[3].0.clone()],
        product_ids(&second_body)
    );
    assert!(
        product_ids(&first_body)
            .iter()
            .all(|id| !product_ids(&second_body).contains(id))
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_keep_product_search_fx_snapshot_pinned_across_pages_when_newer_snapshot_is_captured()
 {
    let products = [
        search_document(
            "Pinned FX cursor cabinet",
            100,
            "AVAILABLE",
            "ACTIVE",
            "Pinned FX Shop",
            "2025-01-01T00:00:00Z",
        ),
        search_document(
            "Pinned FX cursor cabinet",
            100,
            "AVAILABLE",
            "ACTIVE",
            "Pinned FX Shop",
            "2025-01-02T00:00:00Z",
        ),
    ];
    index_search_documents(products.iter().map(|(_, document)| document.clone())).await;

    let captured_at = OffsetDateTime::now_utc();
    let original_fx_rate_id = capture_fx_snapshot(captured_at, 2_000_000).await;
    let first_page_path = "/api/v1/products?language=en&currency=EUR&productQuery[0]=Pinned%20FX%20cursor&price[min]=50&price[max]=50&sort=created&order=asc&size=1".to_owned();
    let (first_response, _) = get_json(first_page_path.clone()).await;
    let (first_status, first_body) = json_response(first_response).await;

    assert_eq!(reqwest::StatusCode::OK, first_status);
    assert_eq!(vec![products[0].0.clone()], product_ids(&first_body));
    assert_eq!(
        json!({ "amount": 50, "currency": "EUR" }),
        first_body["items"][0]["item"]["displayPrice"]
    );
    assert_eq!(
        json!(original_fx_rate_id.to_string()),
        first_body["searchAfter"]["fxRateId"]
    );

    let search_after = serde_json::to_string(&first_body["searchAfter"])
        .unwrap_or_else(|error| panic!("failed to encode search cursor: {error}"));
    let newer_fx_rate_id = capture_fx_snapshot(OffsetDateTime::now_utc(), 1_000_000).await;
    assert_ne!(original_fx_rate_id, newer_fx_rate_id);

    let (second_response, _) = get_json(format!(
        "{first_page_path}&searchAfter={}",
        url_encode(&search_after)
    ))
    .await;
    let (second_status, second_body) = json_response(second_response).await;

    assert_eq!(reqwest::StatusCode::OK, second_status);
    assert_eq!(vec![products[1].0.clone()], product_ids(&second_body));
    assert_eq!(
        json!({ "amount": 50, "currency": "EUR" }),
        second_body["items"][0]["item"]["displayPrice"]
    );
    assert_eq!(
        json!(original_fx_rate_id.to_string()),
        second_body["searchAfter"]["fxRateId"]
    );

    let (fresh_response, _) = get_json(first_page_path).await;
    let (fresh_status, fresh_body) = json_response(fresh_response).await;

    assert_eq!(reqwest::StatusCode::OK, fresh_status);
    assert!(product_ids(&fresh_body).is_empty());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_keep_sold_display_when_fx_snapshot_changes() {
    let captured_at = OffsetDateTime::now_utc() + Duration::days(730);
    let sale_fx_rate_id = capture_fx_snapshot(captured_at, 2_000_000).await;
    let (product_id, mut document) = search_document(
        "Immutable sold cabinet",
        1_000,
        "SOLD",
        "ACTIVE",
        "Sold FX Shop",
        "2025-01-01T00:00:00Z",
    );
    document["sourcePrice"] = Value::Null;
    document["salePrices"] = json!({
        "eur": 40, "gbp": 40, "usd": 40, "aud": 40,
        "cad": 40, "nzd": 40, "cny": 40, "brl": 40,
        "pln": 40, "try": 40, "jpy": 40, "czk": 40,
        "rub": 40, "aed": 40, "sar": 40, "hkd": 40,
        "sgd": 40, "chf": 40
    });
    document["saleFxRateId"] = json!(sale_fx_rate_id.to_string());
    document["soldAt"] = json!("2025-01-01T00:00:00Z");
    index_search_documents([document]).await;

    let path = "/api/v1/products?language=en&currency=EUR&productQuery[0]=Immutable%20sold%20cabinet&price[min]=40&price[max]=40&sort=created&order=asc".to_owned();
    let (before_response, _) = get_json(path.clone()).await;
    let (before_status, before_body) = json_response(before_response).await;

    assert_eq!(reqwest::StatusCode::OK, before_status);
    assert_eq!(vec![product_id.clone()], product_ids(&before_body));
    assert_eq!(json!("SOLD"), before_body["items"][0]["item"]["state"]);
    assert_eq!(
        json!({ "amount": 40, "currency": "EUR" }),
        before_body["items"][0]["item"]["displayPrice"]
    );

    capture_fx_snapshot(captured_at + Duration::days(1), 1_000_000).await;
    let (after_response, _) = get_json(path).await;
    let (after_status, after_body) = json_response(after_response).await;

    assert_eq!(reqwest::StatusCode::OK, after_status);
    assert_eq!(vec![product_id], product_ids(&after_body));
    assert_eq!(
        json!({ "amount": 40, "currency": "EUR" }),
        after_body["items"][0]["item"]["displayPrice"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_return_matching_product_search_summary() {
    let target = search_document(
        "Renaissance walnut cabinet",
        125,
        "AVAILABLE",
        "ACTIVE",
        "Cabinet Shop",
        "2025-01-01T00:00:00Z",
    );
    let unrelated = search_document(
        "Bronze garden sculpture",
        130,
        "AVAILABLE",
        "ACTIVE",
        "Cabinet Shop",
        "2025-01-01T00:00:00Z",
    );
    index_search_documents([target.1.clone(), unrelated.1]).await;

    let (response, _) = get_json(
        "/api/v1/products?language=en&currency=USD&productQuery[0]=Renaissance%20walnut".to_owned(),
    )
    .await;
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert!(body["total"].is_null());
    assert_eq!(vec![target.0], product_ids(&body));
    assert_eq!(
        json!("Renaissance walnut cabinet"),
        body["items"][0]["item"]["title"]["text"]
    );
    assert_eq!(
        json!("USD"),
        body["items"][0]["item"]["displayPrice"]["currency"]
    );
    assert_eq!(
        json!(125),
        body["items"][0]["item"]["displayPrice"]["amount"]
    );
    assert_eq!(json!("AVAILABLE"), body["items"][0]["item"]["state"]);
    assert_eq!(json!("ACTIVE"), body["items"][0]["item"]["lifecycle"]);
    assert!(body["items"][0].get("userState").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_hybrid_search_products_when_mock_embedding_succeeds() {
    let target = search_document(
        "Ornate candle holder",
        125,
        "AVAILABLE",
        "ACTIVE",
        "Semantic Shop",
        "2025-01-01T00:00:00Z",
    );
    let unrelated = search_document(
        "Bronze garden sculpture",
        130,
        "AVAILABLE",
        "ACTIVE",
        "Semantic Shop",
        "2025-01-01T00:00:00Z",
    );
    let unrelated_embedding = std::iter::once(1.0)
        .chain(std::iter::repeat_n(
            0.0,
            embedding::EMBEDDING_DIMENSIONS - 1,
        ))
        .collect();
    index_search_documents([
        with_embedding(target.1.clone(), vec![1.0; embedding::EMBEDDING_DIMENSIONS]),
        with_embedding(unrelated.1, unrelated_embedding),
    ])
    .await;

    let (response, _) = get_json(
        "/api/v1/products?language=en&currency=USD&productQuery[0]=vintage%20brass%20lamp"
            .to_owned(),
    )
    .await;
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some(&target.0), product_ids(&body).first());
    assert!(body["total"].is_null());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API_WITH_FAILED_EMBEDDING])]
async fn should_fall_back_to_bm25_when_mock_embedding_fails() {
    let target = search_document(
        "Vintage brass lamp",
        125,
        "AVAILABLE",
        "ACTIVE",
        "Fallback Shop",
        "2025-01-01T00:00:00Z",
    );
    index_search_documents([target.1.clone()]).await;

    let (response, _) = get_json_from(
        AURA_API_WITH_FAILED_EMBEDDING.base_url(),
        "/api/v1/products?language=en&currency=USD&productQuery[0]=Vintage%20brass%20lamp",
    )
    .await;
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(json!(1), body["total"]);
    assert_eq!(vec![target.0], product_ids(&body));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_intersect_product_search_filters() {
    let target = search_document_with_shop(
        "Filter cabinet",
        550,
        "LISTED",
        "ACTIVE",
        "Imperial Antiques",
        "imperial-antiques",
        "2025-01-01T00:00:00Z",
    );
    let wrong_shop = search_document_with_shop(
        "Filter cabinet",
        550,
        "LISTED",
        "ACTIVE",
        "Other Antiques",
        "other-antiques",
        "2025-01-01T00:00:00Z",
    );
    let wrong_state = search_document_with_shop(
        "Filter cabinet",
        550,
        "AVAILABLE",
        "ACTIVE",
        "Imperial Antiques",
        "imperial-antiques",
        "2025-01-01T00:00:00Z",
    );
    let wrong_price = search_document_with_shop(
        "Filter cabinet",
        2_000,
        "LISTED",
        "ACTIVE",
        "Imperial Antiques",
        "imperial-antiques",
        "2025-01-01T00:00:00Z",
    );
    index_search_documents([target.1.clone(), wrong_shop.1, wrong_state.1, wrong_price.1]).await;

    let (response, _) = get_json(
        "/api/v1/products?language=en&currency=USD&productQuery[0]=Filter%20cabinet&shopName[0]=Imperial%20Antiques&state[0]=LISTED&price[min]=500&price[max]=600".to_owned(),
    )
    .await;
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert!(body["total"].is_null());
    assert_eq!(vec![target.0], product_ids(&body));
    assert_eq!(
        json!("Imperial Antiques"),
        body["items"][0]["item"]["shopName"]
    );
    assert_eq!(json!("LISTED"), body["items"][0]["item"]["state"]);
    assert_eq!(
        json!(550),
        body["items"][0]["item"]["displayPrice"]["amount"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_hide_deleted_products_from_default_search() {
    let active = search_document(
        "Lifecycle fixture",
        100,
        "AVAILABLE",
        "ACTIVE",
        "Lifecycle Shop",
        "2025-01-01T00:00:00Z",
    );
    let deleted = search_document(
        "Lifecycle fixture",
        200,
        "AVAILABLE",
        "DELETED",
        "Lifecycle Shop",
        "2025-01-01T00:00:00Z",
    );
    index_search_documents([active.1.clone(), deleted.1]).await;

    let (response, _) = get_json(
        "/api/v1/products?language=en&currency=USD&productQuery[0]=Lifecycle%20fixture".to_owned(),
    )
    .await;
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(vec![active.0], product_ids(&body));
    assert_eq!(json!("ACTIVE"), body["items"][0]["item"]["lifecycle"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_return_deleted_products_when_lifecycle_filter_requests_them() {
    let active = search_document(
        "Deleted fixture",
        100,
        "AVAILABLE",
        "ACTIVE",
        "Lifecycle Shop",
        "2025-01-01T00:00:00Z",
    );
    let deleted = search_document(
        "Deleted fixture",
        200,
        "AVAILABLE",
        "DELETED",
        "Lifecycle Shop",
        "2025-01-01T00:00:00Z",
    );
    index_search_documents([active.1, deleted.1.clone()]).await;

    let (response, _) = get_json(
        "/api/v1/products?language=en&currency=USD&productQuery[0]=Deleted%20fixture&lifecycle[0]=DELETED".to_owned(),
    )
    .await;
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(vec![deleted.0], product_ids(&body));
    assert_eq!(json!("DELETED"), body["items"][0]["item"]["lifecycle"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_filter_product_search_by_created_date_range() {
    let january = search_document(
        "Date fixture",
        100,
        "AVAILABLE",
        "ACTIVE",
        "Date Shop",
        "2025-01-15T12:00:00Z",
    );
    let june = search_document(
        "Date fixture",
        200,
        "AVAILABLE",
        "ACTIVE",
        "Date Shop",
        "2025-06-15T12:00:00Z",
    );
    index_search_documents([january.1.clone(), june.1]).await;

    let (response, _) = get_json(
        "/api/v1/products?language=en&currency=USD&productQuery[0]=Date%20fixture&created[min]=2025-01-01T00%3A00%3A00Z&created[max]=2025-01-31T23%3A59%3A59Z".to_owned(),
    )
    .await;
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(vec![january.0], product_ids(&body));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_product_id() {
    let (response, _) = get_json("/api/v1/products/not-a-uuid".to_owned()).await;
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_UUID",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_product_slug() {
    let (response, _) =
        get_json("/api/v1/by-slug/shops/Invalid/products/product-a1b2c3".to_owned()).await;
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_PATH_PARAMETER_VALUE",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_optional_product_authentication() {
    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/products/{}",
            AURA_API.base_url(),
            product_core::product_id::ProductId::new()
        ))
        .bearer_auth("invalid")
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to get product with invalid token: {error}"));
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, DYNAMODB, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_product_search_sort() {
    let (response, _) =
        get_json("/api/v1/products?language=en&currency=USD&sort=invalid&order=asc".to_owned())
            .await;
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "BAD_SORT_VALUE",
    );
}

async fn get_json(path: String) -> (reqwest::Response, String) {
    get_json_from(AURA_API.base_url(), &path).await
}

async fn get_json_from(base_url: &str, path: &str) -> (reqwest::Response, String) {
    let url = format!("{base_url}{path}");
    let response = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to GET {path}: {error}"));
    (response, url)
}

async fn capture_fx_snapshot(captured_at: OffsetDateTime, usd_units_per_eur: i64) -> uuid::Uuid {
    let fx_rate_id = uuid::Uuid::new_v4();
    let pool = get_postgres_client().await;
    sqlx::query(
        "INSERT INTO fx_rates (fx_rate_id, captured_at, source, source_event_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(fx_rate_id)
    .bind(captured_at)
    .bind("fxratesapi")
    .bind(fx_rate_id.to_string())
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to capture FX snapshot: {error}"));

    for currency in [
        "EUR", "GBP", "USD", "AUD", "CAD", "NZD", "CNY", "BRL", "PLN", "TRY", "JPY", "CZK", "RUB",
        "AED", "SAR", "HKD", "SGD", "CHF",
    ] {
        let units_per_eur = match currency {
            "EUR" => 1_000_000,
            "USD" => usd_units_per_eur,
            _ => 1_250_000,
        };
        sqlx::query(
            "INSERT INTO fx_rate_quotes (fx_rate_id, currency, units_per_eur) VALUES ($1, $2, $3)",
        )
        .bind(fx_rate_id)
        .bind(currency)
        .bind(units_per_eur)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to capture FX quote: {error}"));
    }

    fx_rate_id
}

async fn index_search_documents(documents: impl IntoIterator<Item = Value>) {
    let pool = get_postgres_client().await;
    seed_current_fx_snapshot(&pool).await;
    let client = get_opensearch_client().await;
    for document in documents {
        let product_id = document["productId"]
            .as_str()
            .unwrap_or_else(|| panic!("search fixture has no productId"))
            .to_owned();
        let response = client
            .index(IndexParts::IndexId(PRODUCTS_INDEX, &product_id))
            .body(document)
            .send()
            .await
            .unwrap_or_else(|error| panic!("failed to index search fixture: {error}"));
        if !response.status_code().is_success() {
            let status = response.status_code();
            let body = response
                .text()
                .await
                .unwrap_or_else(|error| panic!("failed to read index failure: {error}"));
            panic!("failed to index search fixture: {status}: {body}");
        }
    }
    refresh_index(PRODUCTS_INDEX).await;
}

fn search_document(
    title: &str,
    price_usd: u64,
    state: &str,
    lifecycle: &str,
    shop_name: &str,
    created: &str,
) -> (String, Value) {
    search_document_with_shop(
        title,
        price_usd,
        state,
        lifecycle,
        shop_name,
        "search-shop",
        created,
    )
}

fn search_document_with_shop(
    title: &str,
    price_usd: u64,
    state: &str,
    lifecycle: &str,
    shop_name: &str,
    shop_slug_id: &str,
    created: &str,
) -> (String, Value) {
    let product_id = uuid::Uuid::new_v4().to_string();
    let event_id = uuid::Uuid::new_v4().to_string();
    let shop_id = uuid::Uuid::new_v4().to_string();
    let seller_id = uuid::Uuid::new_v4().to_string();
    let product_slug_id = format!("search-{}", &product_id[..6]);
    (
        product_id.clone(),
        json!({
            "productId": product_id,
            "productSlugId": product_slug_id,
            "shopSlugId": shop_slug_id,
            "sellerSlugId": "search-seller",
            "eventId": event_id,
            "shopId": shop_id,
            "sellerId": seller_id,
            "shopsProductId": product_slug_id,
            "shopName": shop_name,
            "sellerName": shop_name,
            "shopType": "COMMERCIAL_DEALER",
            "structuredAddressAddressline": null,
            "structuredAddressAddresslineExtra": null,
            "structuredAddressLocality": null,
            "structuredAddressRegion": null,
            "structuredAddressPostalCode": null,
            "structuredAddressCountry": null,
            "structuredAddressContinent": null,
            "geoAddress": null,
            "title": { "text": title, "language": "EN" },
            "titleDe": null,
            "titleEn": title,
            "titleFr": null,
            "titleEs": null,
            "titleIt": null,
            "sourcePrice": { "amount": price_usd, "currency": "USD" },
            "state": state,
            "lifecycle": lifecycle,
            "url": "https://shop.example/product",
            "viewUrl": "https://aura.example/product",
            "images": [],
            "embedding": null,
            "auctionStart": null,
            "auctionEnd": null,
            "created": created,
            "updated": created
        }),
    )
}

fn with_embedding(mut document: Value, embedding: Vec<f32>) -> Value {
    document["embedding"] = json!(embedding);
    document
}

fn product_ids(body: &Value) -> Vec<String> {
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("search response has no items array"))
        .iter()
        .map(|item| {
            item["item"]["productId"]
                .as_str()
                .unwrap_or_else(|| panic!("search item has no product ID"))
                .to_owned()
        })
        .collect()
}

fn url_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}
