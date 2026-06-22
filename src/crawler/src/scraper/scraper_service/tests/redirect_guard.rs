use super::*;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::pipeline::scrape_product::is_redirect_to_homepage;

#[test]
fn should_detect_same_host_product_redirect_to_homepage() {
    let original = Url::parse("https://example.com/products/123").unwrap();
    let final_url = Url::parse("https://example.com/").unwrap();

    assert!(is_redirect_to_homepage(&original, &final_url));
}

#[test]
fn should_detect_www_variant_redirect_to_homepage() {
    let original = Url::parse("https://example.com/products/123").unwrap();
    let final_url = Url::parse("https://www.example.com/").unwrap();

    assert!(is_redirect_to_homepage(&original, &final_url));
}

#[test]
fn should_not_detect_redirect_to_another_product_page_as_homepage() {
    let original = Url::parse("https://example.com/products/123").unwrap();
    let final_url = Url::parse("https://example.com/products/456").unwrap();

    assert!(!is_redirect_to_homepage(&original, &final_url));
}

#[test]
fn should_not_detect_equivalent_url_normalization_as_homepage_redirect() {
    let original = Url::parse("https://example.com/products/123#details").unwrap();
    let final_url = Url::parse("https://example.com/products/123").unwrap();

    assert!(!is_redirect_to_homepage(&original, &final_url));
}

#[tokio::test]
async fn should_mark_product_removed_when_product_url_redirects_to_homepage() {
    let id = shop_id();
    let url = product_url();
    let homepage = Url::parse("https://example.com/").unwrap();

    let mut fetcher = MockHtmlFetcher::new();
    let final_url = homepage.clone();
    fetcher.expect_fetch().once().returning(move |_| {
        let final_url = final_url.clone();
        Box::pin(async move { Ok(fetch_result_for(sample_html(), final_url)) })
    });

    let schema_svc = MockProductSchemaService::new();
    let norm_svc = MockProductNormalizationService::new();

    let mut cand_svc = MockScraperCandidateService::new();
    let url_for_set_state = url.clone();
    cand_svc
        .expect_set_state()
        .once()
        .withf(move |received_shop_id, received_url, received_state| {
            *received_shop_id == id
                && received_url == &url_for_set_state
                && *received_state == UrlState::Removed
        })
        .returning(|_, _, _| Box::pin(async { Ok(()) }));

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Box::new(norm_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_SHOP,
    );

    let err = service.scrape(&id, &url, None).await.unwrap_err();

    assert!(matches!(err, ScraperError::ProductRemoved { .. }));
}
