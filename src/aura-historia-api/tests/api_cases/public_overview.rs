use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};
use api_support::{json_response, seed_product};
use platform_postgres::SqlxUnitOfWork;
use product_listing_postgres::SqlxPublicCatalogueOverviewRefresherFactory;
use product_listing_service::use_cases::{
    PublicCatalogueOverview, RefreshPublicCatalogueOverviewHandler,
    RefreshPublicCatalogueOverviewUseCase,
};
use serde_json::json;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

async fn get_overview(
    path_and_query: &str,
    authorization: Option<&str>,
) -> (reqwest::StatusCode, serde_json::Value, Option<String>) {
    let mut request =
        reqwest::Client::new().get(format!("{}{path_and_query}", AURA_API.base_url()));
    if let Some(authorization) = authorization {
        request = request.header(reqwest::header::AUTHORIZATION, authorization);
    }
    let response = request
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to call public overview API: {error}"));
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let (status, body) = json_response(response).await;
    (status, body, cache_control)
}

async fn refresh() -> PublicCatalogueOverview {
    RefreshPublicCatalogueOverviewHandler::new(
        SqlxUnitOfWork::new(get_postgres_client().await),
        SqlxPublicCatalogueOverviewRefresherFactory::new(),
    )
    .execute()
    .await
    .unwrap_or_else(|error| panic!("failed to refresh public overview: {error}"))
    .unwrap_or_else(|| panic!("the first refresh must be due"))
    .overview
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_answer_unavailable_without_caching_before_the_first_refresh() {
    let (status, body, cache_control) = get_overview("/api/v1/overview", None).await;

    assert_eq!(reqwest::StatusCode::SERVICE_UNAVAILABLE, status);
    assert_eq!(Some("private, no-store".to_owned()), cache_control);
    assert_eq!(
        Some(&json!("OVERVIEW_TEMPORARILY_UNAVAILABLE")),
        body.get("error")
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_share_the_stored_overview_regardless_of_query_strings() {
    seed_product().await;
    let calculated = refresh().await;
    // Later catalogue changes wait for the next due refresh.
    seed_product().await;

    let mut representations = Vec::new();
    for path_and_query in [
        "/api/v1/overview",
        "/api/v1/overview?utm_source=landing&language=de",
    ] {
        let (status, mut body, cache_control) = get_overview(path_and_query, None).await;

        assert_eq!(reqwest::StatusCode::OK, status, "{path_and_query}");
        assert_eq!(
            Some("public, max-age=0, s-maxage=21600, stale-if-error=0".to_owned()),
            cache_control,
            "{path_and_query}"
        );
        let as_of = body
            .as_object_mut()
            .and_then(|body| body.remove("asOf"))
            .and_then(|as_of| as_of.as_str().map(str::to_owned))
            .unwrap_or_else(|| panic!("overview must contain asOf: {body}"));
        let as_of = OffsetDateTime::parse(&as_of, &Rfc3339)
            .unwrap_or_else(|error| panic!("asOf must be RFC 3339: {error}"));
        assert_eq!(calculated.as_of, as_of);
        representations.push(body);
    }

    assert_eq!(
        json!({
            "sourcesIndexed": 1,
            "liveObjects": 1,
            "countries": 0,
            "languages": 0,
            "newObjectsLast7Days": 1
        }),
        representations[0]
    );
    assert_eq!(representations[0], representations[1]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_keep_authorization_bearing_overview_requests_private() {
    refresh().await;

    let (status, body, cache_control) =
        get_overview("/api/v1/overview", Some("Bearer invalid")).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some("private, no-store".to_owned()), cache_control);
    assert_eq!(Some(&json!(0)), body.get("liveObjects"));
}
