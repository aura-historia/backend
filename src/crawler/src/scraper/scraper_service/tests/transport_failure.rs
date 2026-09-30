use super::*;
use crate::network::policy::{DomainFailureKind, NetworkErrorKind};
use crate::scraper::scraper_service::domain::product::{DomainFetchHealth, FetchFailureSource};
use crate::scraper::scraper_service::service::FetchError;
use crate::scraper::scraper_service::{ScrapeMode, ScrapeRequest, ScraperError};
use std::time::Duration;

fn service_for_primary_failure(error: FetchError) -> ScraperServiceImpl {
    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let error = error.clone();
        Box::pin(async move { Err(error) })
    });

    ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(MockProductListingSchemaService::new()),
        Box::new(MockProductListingNormalizationService::new()),
        Arc::new(MockScraperCandidateService::new()),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    )
}

fn primary_request(
    listing_source_id: listing_source_core::ListingSourceId,
    url: Url,
    fence: Vec<u8>,
) -> ScrapeRequest {
    ScrapeRequest {
        domain_id: None,
        listing_source_id,
        raw_source_record_key: url.to_string(),
        url,
        product_url_pattern: None,
        last_scraped_hash: None,
        last_scraped_schema_fingerprint: None,
        expected_last_captured_raw_input_sha256: Some(fence),
        fallback_currency: None,
        mode: ScrapeMode::PrimaryOnly,
    }
}

#[tokio::test]
async fn primary_network_failure_preserves_non_null_raw_input_fence() {
    let listing_source_id = listing_source_id();
    let url = product_url();
    let fence = vec![0x11; 32];
    let service = service_for_primary_failure(FetchError::Network {
        kind: NetworkErrorKind::HttpStatus(429),
        details: "rate limited".to_owned(),
    });

    let outcome = with_test_scrape_domain(service.scrape(primary_request(
        listing_source_id,
        url.clone(),
        fence.clone(),
    )))
    .await;

    assert!(matches!(
        &outcome.result,
        Err(ScraperError::HttpError {
            kind: NetworkErrorKind::HttpStatus(429),
            ..
        })
    ));
    assert_eq!(
        outcome.domain_health,
        DomainFetchHealth::CircuitFailure {
            kind: DomainFailureKind::Http429,
            status_code: Some(429),
            retry_after: None,
        }
    );
    let failure = outcome
        .fetch_failure
        .expect("primary transport failure should carry context");
    assert_eq!(failure.url, url);
    assert_eq!(failure.source, FetchFailureSource::Primary);
    assert_eq!(failure.kind, NetworkErrorKind::HttpStatus(429));
    assert_eq!(failure.status_code, Some(429));
    assert_eq!(failure.expected_last_captured_raw_input_sha256, Some(fence));
}

#[tokio::test]
async fn primary_network_failure_with_metadata_preserves_fence_and_retry_after() {
    let listing_source_id = listing_source_id();
    let url = product_url();
    let fence = vec![0x12; 32];
    let retry_after = Duration::from_secs(300);
    let service = service_for_primary_failure(FetchError::NetworkWithMetadata {
        kind: NetworkErrorKind::HttpStatus(429),
        status: Some(429),
        retry_after: Some(retry_after),
        details: "rate limited".to_owned(),
    });

    let outcome = with_test_scrape_domain(service.scrape(primary_request(
        listing_source_id,
        url.clone(),
        fence.clone(),
    )))
    .await;

    assert!(matches!(
        &outcome.result,
        Err(ScraperError::HttpErrorWithMetadata(metadata))
            if metadata.kind == NetworkErrorKind::HttpStatus(429)
                && metadata.status_code == Some(429)
                && metadata.retry_after == Some(retry_after)
    ));
    let failure = outcome
        .fetch_failure
        .expect("primary transport failure should carry context");
    assert_eq!(failure.url, url);
    assert_eq!(failure.source, FetchFailureSource::Primary);
    assert_eq!(failure.status_code, Some(429));
    assert_eq!(failure.retry_after, Some(retry_after));
    assert_eq!(failure.expected_last_captured_raw_input_sha256, Some(fence));
}

#[tokio::test]
async fn primary_removed_response_preserves_fence_and_responsive_health() {
    let listing_source_id = listing_source_id();
    let url = product_url();
    let fence = vec![0x13; 32];
    let service = service_for_primary_failure(FetchError::NetworkWithMetadata {
        kind: NetworkErrorKind::HttpStatus(404),
        status: Some(404),
        retry_after: None,
        details: "not found".to_owned(),
    });

    let outcome = with_test_scrape_domain(service.scrape(primary_request(
        listing_source_id,
        url.clone(),
        fence.clone(),
    )))
    .await;

    assert!(matches!(
        &outcome.result,
        Err(ScraperError::ProductListingRemoved { .. })
    ));
    assert_eq!(outcome.domain_health, DomainFetchHealth::Responsive);
    let failure = outcome
        .fetch_failure
        .expect("removed response should carry primary context");
    assert_eq!(failure.url, url);
    assert_eq!(failure.source, FetchFailureSource::Primary);
    assert_eq!(failure.status_code, Some(404));
    assert_eq!(failure.expected_last_captured_raw_input_sha256, Some(fence));
}
