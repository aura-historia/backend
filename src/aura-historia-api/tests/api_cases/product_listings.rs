use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};

use api_support::{
    assert_problem, aura_api_app_with_failed_search_embedding, json_response,
    seed_current_fx_snapshot, seed_product,
};
use application::transaction::{Transaction, UnitOfWork};
use fxrate_core::FxRateId;
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use opensearch::{IndexParts, indices::IndicesPutMappingParts};
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::{
    description::Description,
    listing_availability::ListingAvailability,
    product_listing::{
        ListingSaleObservation, NewProductListing, ProductListing, ProductListingAuction,
        ProductListingPricing,
    },
    product_listing_id::ProductListingId,
    source_listing_id::SourceListingId,
    title::Title,
};
use product_listing_postgres::{
    SqlxProductListingEventStoreFactory, SqlxProductListingRepositoryFactory,
};
use product_listing_service::ports::{
    ProductListingEventStore, ProductListingEventStoreFactory, ProductListingRepository,
    ProductListingRepositoryFactory, stamp_product_listing_events,
};
use serde_json::{Value, json};
use test_api::{
    AuraHistoriaApi, IntegrationTestService, aura_integration_test, get_opensearch_client,
    get_postgres_client, refresh_index,
};
use time::{Duration, OffsetDateTime, UtcOffset};
use url::Url;

const PRODUCTS_INDEX: &str = "product-listings";
static AURA_API_WITH_FAILED_EMBEDDING: AuraHistoriaApi =
    AuraHistoriaApi::new(aura_api_app_with_failed_search_embedding);

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_get_product_details_by_id() {
    let product_listing_id = seed_product().await;

    let response = get_json(format!("/api/v1/product-listings/{product_listing_id}")).await;
    let cache_control = response
        .0
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (status, body) = json_response(response.0).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(
        json!(product_listing_id.to_string()),
        body["item"]["productListingId"]
    );
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

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_get_product_history_by_id() {
    let product_listing_id = seed_product().await;

    let (response, _) = get_json(format!(
        "/api/v1/product-listings/{product_listing_id}/history"
    ))
    .await;
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert!(body.as_array().is_some_and(|events| !events.is_empty()));
    assert_eq!(
        json!(product_listing_id.to_string()),
        body[0]["productListingId"]
    );
    assert_eq!(
        Some("public, max-age=180, s-maxage=900".to_owned()),
        cache_control
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_get_product_history_with_timestamped_event_payloads() {
    let product_listing_id = ProductListingId::new();
    let source_offset =
        UtcOffset::from_hms(5, 30, 0).unwrap_or_else(|error| panic!("source offset: {error}"));
    let auction_start = (OffsetDateTime::from_unix_timestamp(1_700_000_000)
        .unwrap_or_else(|error| panic!("auction start: {error}"))
        + Duration::nanoseconds(123_456_789))
    .to_offset(source_offset);
    let auction_end = (auction_start + Duration::hours(3)).to_offset(source_offset);
    let mut product = ProductListing::create(NewProductListing {
        id: product_listing_id,
        listing_source_id: ListingSourceId::new(),
        source_listing_id: SourceListingId::try_from(format!(
            "timestamped-history-{product_listing_id}"
        ))
        .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
        title: Some(Localized::new(
            Language::En,
            Title::from("Timestamped history ProductListing"),
        )),
        description: Some(Localized::new(
            Language::En,
            Description::from("Timestamp-bearing event payload fixture"),
        )),
        pricing: ProductListingPricing::default(),
        availability: Some(ListingAvailability::Available),
        url: Url::parse("https://api-acceptance.example/timestamped-history")
            .unwrap_or_else(|error| panic!("product URL: {error}")),
        images: IndexSet::new(),
        auction: ProductListingAuction {
            start: Some(auction_start),
            end: Some(auction_end),
        },
    })
    .unwrap_or_else(|error| panic!("create timestamped history product: {error}"));
    let created_events = stamp_product_listing_events(
        product_listing_id,
        OffsetDateTime::now_utc(),
        product.take_pending_event_payloads(),
    );
    let created_event_id = created_events
        .last()
        .map(|event| event.event_id)
        .unwrap_or_else(|| panic!("created product event is missing"));

    product
        .replace_auction(ProductListingAuction {
            start: Some((auction_start + Duration::days(1)).to_offset(source_offset)),
            end: Some((auction_end + Duration::days(1)).to_offset(source_offset)),
        })
        .unwrap_or_else(|error| panic!("change auction: {error}"));
    let observation = ListingSaleObservation::new(
        (auction_end + Duration::nanoseconds(987_654_321)).to_offset(source_offset),
        FxRateId::new(),
    );
    product
        .record_sale_observation(observation)
        .unwrap_or_else(|error| panic!("record sale observation: {error}"));
    product.retract_sale_observation();
    let changed_events = stamp_product_listing_events(
        product_listing_id,
        OffsetDateTime::now_utc(),
        product.take_pending_event_payloads(),
    );
    let current_event_id = changed_events
        .last()
        .map(|event| event.event_id)
        .unwrap_or_else(|| panic!("changed product event is missing"));

    let unit_of_work = SqlxUnitOfWork::new(get_postgres_client().await);
    let products = SqlxProductListingRepositoryFactory::new();
    let events = SqlxProductListingEventStoreFactory::new();
    let mut transaction = unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin product history transaction: {error}"));
    products
        .in_transaction(&mut transaction)
        .insert(&product, created_event_id)
        .await
        .unwrap_or_else(|error| panic!("insert timestamped history product: {error:?}"));
    for event in &created_events {
        events
            .in_transaction(&mut transaction)
            .append(event)
            .await
            .unwrap_or_else(|error| panic!("append created product event: {error:?}"));
    }
    products
        .in_transaction(&mut transaction)
        .update(&product, created_event_id, current_event_id)
        .await
        .unwrap_or_else(|error| panic!("update timestamped history product: {error:?}"));
    for event in &changed_events {
        events
            .in_transaction(&mut transaction)
            .append(event)
            .await
            .unwrap_or_else(|error| panic!("append changed product event: {error:?}"));
    }
    transaction
        .commit()
        .await
        .unwrap_or_else(|error| panic!("commit product history transaction: {error}"));

    let (response, _) = get_json(format!(
        "/api/v1/product-listings/{product_listing_id}/history"
    ))
    .await;
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    let history = body
        .as_array()
        .unwrap_or_else(|| panic!("product history is not an array: {body}"));
    let event = |event_type| {
        history
            .iter()
            .find(|event| event["eventType"] == event_type)
            .unwrap_or_else(|| panic!("missing {event_type} history event"))
    };
    for event_type in ["PRODUCT_LISTING_CREATED", "PRODUCT_LISTING_AUCTION_CHANGED"] {
        let auction = &event(event_type)["payload"]["auction"];
        assert!(auction["start"].is_string());
        assert!(auction["end"].is_string());
    }
    for event_type in [
        "PRODUCT_LISTING_SALE_OBSERVED",
        "PRODUCT_LISTING_SALE_OBSERVATION_RETRACTED",
    ] {
        assert!(event(event_type)["payload"]["observedAt"].is_string());
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_pending_similar_products_by_id() {
    let product_listing_id = seed_product().await;

    let (response, _) = get_json(format!(
        "/api/v1/product-listings/{product_listing_id}/similar"
    ))
    .await;

    assert_eq!(reqwest::StatusCode::ACCEPTED, response.status());
    assert_eq!(
        Some(format!(
            "/api/v1/product-listings/{product_listing_id}/similar"
        )),
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

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_page_product_search_without_duplicates_when_using_cursor() {
    let products = [
        search_document(
            "Price page cabinet",
            100,
            "AVAILABLE",
            "Price Listing Source",
            "2025-01-01T00:00:00Z",
        ),
        search_document(
            "Price page cabinet",
            200,
            "AVAILABLE",
            "Price Listing Source",
            "2025-01-02T00:00:00Z",
        ),
        search_document(
            "Price page cabinet",
            300,
            "AVAILABLE",
            "Price Listing Source",
            "2025-01-03T00:00:00Z",
        ),
        search_document(
            "Price page cabinet",
            400,
            "AVAILABLE",
            "Price Listing Source",
            "2025-01-04T00:00:00Z",
        ),
    ];
    index_search_documents(products.iter().map(|(_, document)| document.clone())).await;

    let (first_response, _) = get_json(
        "/api/v1/product-listings?language=en&currency=USD&productQuery[0]=Price%20page%20cabinet&sort=created&order=asc&size=2".to_owned(),
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
        product_listing_ids(&first_body)
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
        "/api/v1/product-listings?language=en&currency=USD&productQuery[0]=Price%20page%20cabinet&sort=created&order=asc&size=2&searchAfter={}",
        url_encode(&search_after)
    ))
    .await;
    let (second_status, second_body) = json_response(second_response).await;

    assert_eq!(reqwest::StatusCode::OK, second_status);
    assert_eq!(
        vec![products[2].0.clone(), products[3].0.clone()],
        product_listing_ids(&second_body)
    );
    assert!(
        product_listing_ids(&first_body)
            .iter()
            .all(|id| !product_listing_ids(&second_body).contains(id))
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_keep_product_search_fx_snapshot_pinned_across_pages_when_newer_snapshot_is_captured()
 {
    let products = [
        search_document(
            "Pinned FX cursor cabinet",
            100,
            "AVAILABLE",
            "Pinned FX Listing Source",
            "2025-01-01T00:00:00Z",
        ),
        search_document(
            "Pinned FX cursor cabinet",
            100,
            "AVAILABLE",
            "Pinned FX Listing Source",
            "2025-01-02T00:00:00Z",
        ),
    ];
    index_search_documents(products.iter().map(|(_, document)| document.clone())).await;

    let captured_at = OffsetDateTime::now_utc();
    let original_fx_rate_id = capture_fx_snapshot(captured_at, 2_000_000).await;
    let first_page_path = "/api/v1/product-listings?language=en&currency=EUR&productQuery[0]=Pinned%20FX%20cursor&price[min]=50&price[max]=50&sort=created&order=asc&size=1".to_owned();
    let (first_response, _) = get_json(first_page_path.clone()).await;
    let (first_status, first_body) = json_response(first_response).await;

    assert_eq!(reqwest::StatusCode::OK, first_status);
    assert_eq!(
        vec![products[0].0.clone()],
        product_listing_ids(&first_body)
    );
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
    assert_eq!(
        vec![products[1].0.clone()],
        product_listing_ids(&second_body)
    );
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
    assert!(product_listing_ids(&fresh_body).is_empty());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_keep_sold_display_when_fx_snapshot_changes() {
    let captured_at = OffsetDateTime::now_utc() + Duration::days(730);
    let sale_fx_rate_id = capture_fx_snapshot(captured_at, 2_000_000).await;
    let (product_listing_id, mut document) = search_document(
        "Immutable sold cabinet",
        1_000,
        "SOLD_OUT",
        "Sold FX Listing Source",
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
    document["saleObservationFxRateId"] = json!(sale_fx_rate_id.to_string());
    document["saleObservedAt"] = json!("2025-01-01T00:00:00Z");
    index_search_documents([document]).await;

    let path = "/api/v1/product-listings?language=en&currency=EUR&productQuery[0]=Immutable%20sold%20cabinet&price[min]=40&price[max]=40&sort=created&order=asc".to_owned();
    let (before_response, _) = get_json(path.clone()).await;
    let (before_status, before_body) = json_response(before_response).await;

    assert_eq!(reqwest::StatusCode::OK, before_status);
    assert_eq!(
        vec![product_listing_id.clone()],
        product_listing_ids(&before_body)
    );
    assert_eq!(
        json!("SOLD_OUT"),
        before_body["items"][0]["item"]["availability"]
    );
    assert_eq!(
        json!({ "amount": 40, "currency": "EUR" }),
        before_body["items"][0]["item"]["displayPrice"]
    );

    capture_fx_snapshot(captured_at + Duration::days(1), 1_000_000).await;
    let (after_response, _) = get_json(path).await;
    let (after_status, after_body) = json_response(after_response).await;

    assert_eq!(reqwest::StatusCode::OK, after_status);
    assert_eq!(vec![product_listing_id], product_listing_ids(&after_body));
    assert_eq!(
        json!({ "amount": 40, "currency": "EUR" }),
        after_body["items"][0]["item"]["displayPrice"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_matching_product_search_summary() {
    let target = search_document(
        "Renaissance walnut cabinet",
        125,
        "AVAILABLE",
        "Cabinet Listing Source",
        "2025-01-01T00:00:00Z",
    );
    let unrelated = search_document(
        "Bronze garden sculpture",
        130,
        "AVAILABLE",
        "Cabinet Listing Source",
        "2025-01-01T00:00:00Z",
    );
    index_search_documents([target.1.clone(), unrelated.1]).await;

    let (response, _) = get_json(
        "/api/v1/product-listings?language=en&currency=USD&productQuery[0]=Renaissance%20walnut"
            .to_owned(),
    )
    .await;
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert!(body["total"].is_null());
    assert_eq!(vec![target.0], product_listing_ids(&body));
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
    assert_eq!(json!("AVAILABLE"), body["items"][0]["item"]["availability"]);
    assert!(body["items"][0].get("userState").is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_hybrid_search_products_when_mock_embedding_succeeds() {
    let target = search_document(
        "Ornate candle holder",
        125,
        "AVAILABLE",
        "Semantic Listing Source",
        "2025-01-01T00:00:00Z",
    );
    let unrelated = search_document(
        "Bronze garden sculpture",
        130,
        "AVAILABLE",
        "Semantic Listing Source",
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
        "/api/v1/product-listings?language=en&currency=USD&productQuery[0]=vintage%20brass%20lamp"
            .to_owned(),
    )
    .await;
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(Some(&target.0), product_listing_ids(&body).first());
    assert!(body["total"].is_null());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API_WITH_FAILED_EMBEDDING])]
async fn should_fall_back_to_bm25_when_mock_embedding_fails() {
    let target = search_document(
        "Vintage brass lamp",
        125,
        "AVAILABLE",
        "Fallback Listing Source",
        "2025-01-01T00:00:00Z",
    );
    index_search_documents([target.1.clone()]).await;

    let (response, _) = get_json_from(
        AURA_API_WITH_FAILED_EMBEDDING.base_url(),
        "/api/v1/product-listings?language=en&currency=USD&productQuery[0]=Vintage%20brass%20lamp",
    )
    .await;
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(json!(1), body["total"]);
    assert_eq!(vec![target.0], product_listing_ids(&body));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_intersect_product_search_filters() {
    let target = search_document_with_source(
        "Filter cabinet",
        550,
        "AVAILABLE",
        "Imperial Antiques",
        "imperial-antiques",
        "2025-01-01T00:00:00Z",
    );
    let wrong_listing_source = search_document_with_source(
        "Filter cabinet",
        550,
        "AVAILABLE",
        "Other Antiques",
        "other-antiques",
        "2025-01-01T00:00:00Z",
    );
    let wrong_availability = search_document_with_source(
        "Filter cabinet",
        550,
        "OUT_OF_STOCK",
        "Imperial Antiques",
        "imperial-antiques",
        "2025-01-01T00:00:00Z",
    );
    let wrong_price = search_document_with_source(
        "Filter cabinet",
        2_000,
        "AVAILABLE",
        "Imperial Antiques",
        "imperial-antiques",
        "2025-01-01T00:00:00Z",
    );
    index_search_documents([
        target.1.clone(),
        wrong_listing_source.1,
        wrong_availability.1,
        wrong_price.1,
    ])
    .await;

    let (response, _) = get_json(
        format!(
            "/api/v1/product-listings?language=en&currency=USD&productQuery[0]=Filter%20cabinet&listingSourceId[0]={}&availability[0]=AVAILABLE&price[min]=500&price[max]=600",
            target.1["listingSourceId"]
        ),
    )
    .await;
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert!(body["total"].is_null());
    assert_eq!(vec![target.0], product_listing_ids(&body));
    assert_eq!(
        json!("Imperial Antiques"),
        body["items"][0]["item"]["source"]["name"]
    );
    assert_eq!(json!("AVAILABLE"), body["items"][0]["item"]["availability"]);
    assert_eq!(
        json!(550),
        body["items"][0]["item"]["displayPrice"]["amount"]
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_return_projected_product_listings_from_default_search() {
    let active = search_document(
        "Lifecycle fixture",
        100,
        "AVAILABLE",
        "Lifecycle Listing Source",
        "2025-01-01T00:00:00Z",
    );

    index_search_documents([active.1.clone()]).await;

    let (response, _) = get_json(
        "/api/v1/product-listings?language=en&currency=USD&productQuery[0]=Lifecycle%20fixture"
            .to_owned(),
    )
    .await;
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(vec![active.0], product_listing_ids(&body));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_filter_product_search_by_availability() {
    let available = search_document(
        "Availability fixture",
        100,
        "AVAILABLE",
        "Availability Listing Source",
        "2025-01-01T00:00:00Z",
    );
    let sold_out = search_document(
        "Availability fixture",
        200,
        "SOLD_OUT",
        "Availability Listing Source",
        "2025-01-01T00:00:00Z",
    );
    index_search_documents([available.1, sold_out.1.clone()]).await;

    let (response, _) = get_json(
        "/api/v1/product-listings?language=en&currency=USD&productQuery[0]=Availability%20fixture&availability[0]=SOLD_OUT".to_owned(),
    )
    .await;
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(vec![sold_out.0], product_listing_ids(&body));
    assert_eq!(json!("SOLD_OUT"), body["items"][0]["item"]["availability"]);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_filter_product_search_by_created_date_range() {
    let january = search_document(
        "Date fixture",
        100,
        "AVAILABLE",
        "Date Listing Source",
        "2025-01-15T12:00:00Z",
    );
    let june = search_document(
        "Date fixture",
        200,
        "AVAILABLE",
        "Date Listing Source",
        "2025-06-15T12:00:00Z",
    );
    index_search_documents([january.1.clone(), june.1]).await;

    let (response, _) = get_json(
        "/api/v1/product-listings?language=en&currency=USD&productQuery[0]=Date%20fixture&created[min]=2025-01-01T00%3A00%3A00Z&created[max]=2025-01-31T23%3A59%3A59Z".to_owned(),
    )
    .await;
    let (status, body) = json_response(response).await;

    assert_eq!(reqwest::StatusCode::OK, status);
    assert_eq!(vec![january.0], product_listing_ids(&body));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_product_listing_id() {
    let (response, _) = get_json("/api/v1/product-listings/not-a-uuid".to_owned()).await;
    let (status, body) = json_response(response).await;

    assert_problem(
        status,
        &body,
        reqwest::StatusCode::BAD_REQUEST,
        "INVALID_UUID",
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_optional_product_authentication() {
    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/product-listings/{}",
            AURA_API.base_url(),
            product_listing_core::product_listing_id::ProductListingId::new()
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

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_product_search_sort() {
    let (response, _) = get_json(
        "/api/v1/product-listings?language=en&currency=USD&sort=invalid&order=asc".to_owned(),
    )
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
    ensure_canonical_product_listing_mapping(client).await;
    for document in documents {
        let product_listing_id = document["productListingId"]
            .as_str()
            .unwrap_or_else(|| panic!("search fixture has no productListingId"))
            .to_owned();
        let response = client
            .index(IndexParts::IndexId(PRODUCTS_INDEX, &product_listing_id))
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

async fn ensure_canonical_product_listing_mapping(client: &opensearch::OpenSearch) {
    let response = client
        .indices()
        .put_mapping(IndicesPutMappingParts::Index(&[PRODUCTS_INDEX]))
        .body(json!({
            "properties": {
                "availability": { "type": "keyword" }
            }
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("failed to add canonical availability mapping: {error}"));
    if !response.status_code().is_success() {
        let status = response.status_code();
        let body = response
            .text()
            .await
            .unwrap_or_else(|error| panic!("failed to read mapping failure: {error}"));
        panic!("failed to add canonical availability mapping: {status}: {body}");
    }
}

fn search_document(
    title: &str,
    price_usd: u64,
    availability: &str,
    listing_source_name: &str,
    created: &str,
) -> (String, Value) {
    search_document_with_source(
        title,
        price_usd,
        availability,
        listing_source_name,
        "search-listing-source",
        created,
    )
}

fn search_document_with_source(
    title: &str,
    price_usd: u64,
    availability: &str,
    _listing_source_name: &str,
    _listing_source_slug_id: &str,
    created: &str,
) -> (String, Value) {
    let product_listing_id = uuid::Uuid::new_v4().to_string();
    let event_id = uuid::Uuid::new_v4().to_string();
    let listing_source_id = uuid::Uuid::new_v4().to_string();
    let product_listing_slug_id = format!("search-{}", &product_listing_id[..6]);
    (
        product_listing_id.clone(),
        json!({
            "productListingId": product_listing_id,
            "productListingSlugId": product_listing_slug_id,
            "listingSourceId": listing_source_id,
            "sourceListingId": product_listing_slug_id,
            "eventId": event_id,
            "title": { "text": title, "language": "EN" },
            "titleDe": null,
            "titleEn": title,
            "titleFr": null,
            "titleEs": null,
            "titleIt": null,
            "sourcePrice": { "amount": price_usd, "currency": "USD" },
            "availability": availability,
            "url": "https://listing-source.example/product",
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

fn product_listing_ids(body: &Value) -> Vec<String> {
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("search response has no items array"))
        .iter()
        .map(|item| {
            item["item"]["productListingId"]
                .as_str()
                .unwrap_or_else(|| panic!("search item has no product ID"))
                .to_owned()
        })
        .collect()
}

fn url_encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}
