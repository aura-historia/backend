use super::*;
use crate::scraper::scraper_service::{ScrapeMode, ScrapeRequest, ScraperError};

fn request(mode: ScrapeMode, domain_id: Option<crate::CrawlerDomainId>) -> ScrapeRequest {
    ScrapeRequest {
        domain_id,
        listing_source_id: listing_source_id(),
        url: product_url(),
        raw_source_record_key: product_url().to_string(),
        product_url_pattern: None,
        last_scraped_hash: None,
        last_scraped_schema_fingerprint: None,
        expected_last_captured_raw_input_sha256: None,
        fallback_currency: None,
        mode,
    }
}

fn service_without_fetch_expectation() -> ScraperServiceImpl {
    ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(MockHtmlFetcher::new()),
        Box::new(MockProductListingSchemaService::new()),
        Box::new(MockProductListingNormalizationService::new()),
        Arc::new(MockScraperCandidateService::new()),
        3,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    )
}

#[tokio::test]
async fn normal_requires_explicit_domain_context() {
    let outcome = service_without_fetch_expectation()
        .scrape(request(ScrapeMode::Normal, None))
        .await;

    assert!(matches!(
        outcome.result,
        Err(ScraperError::MissingDomainContext { .. })
    ));
}

#[tokio::test]
async fn domain_probe_requires_explicit_domain_context() {
    let outcome = service_without_fetch_expectation()
        .scrape(request(ScrapeMode::DomainProbe, None))
        .await;

    assert!(matches!(
        outcome.result,
        Err(ScraperError::MissingDomainContext { .. })
    ));
}
