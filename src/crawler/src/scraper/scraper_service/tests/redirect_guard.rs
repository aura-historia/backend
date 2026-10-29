use super::*;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::pipeline::scrape_product::is_redirect_to_non_product_page;

#[test]
fn should_detect_same_host_product_redirect_to_homepage() {
    let original = Url::parse("https://example.com/products/123").unwrap();
    let final_url = Url::parse("https://example.com/").unwrap();

    assert!(is_redirect_to_non_product_page(&original, &final_url, None));
}

#[test]
fn should_detect_www_variant_redirect_to_homepage() {
    let original = Url::parse("https://example.com/products/123").unwrap();
    let final_url = Url::parse("https://www.example.com/").unwrap();

    assert!(is_redirect_to_non_product_page(&original, &final_url, None));
}

#[test]
fn should_detect_same_host_product_redirect_to_category_page_when_pattern_does_not_match() {
    let original =
        Url::parse("https://www.ebth.com/items/7055109-digital-print-television-poster-dirt")
            .unwrap();
    let final_url = Url::parse(
        "https://www.ebth.com/collectibles/memorabilia/television-and-movie-memorabilia",
    )
    .unwrap();

    assert!(is_redirect_to_non_product_page(
        &original,
        &final_url,
        Some(r"/items/")
    ));
}

#[test]
fn should_not_detect_redirect_to_another_matching_product_page() {
    let original = Url::parse("https://example.com/products/123").unwrap();
    let final_url = Url::parse("https://example.com/products/456").unwrap();

    assert!(!is_redirect_to_non_product_page(
        &original,
        &final_url,
        Some(r"/products/")
    ));
}

#[test]
fn should_not_detect_equivalent_url_normalization_as_homepage_redirect() {
    let original = Url::parse("https://example.com/products/123#details").unwrap();
    let final_url = Url::parse("https://example.com/products/123").unwrap();

    assert!(!is_redirect_to_non_product_page(
        &original,
        &final_url,
        Some(r"/products/")
    ));
}

#[test]
fn should_not_detect_redirect_that_only_adds_or_removes_trailing_slash() {
    for (original, final_url) in [
        (
            "https://example.com/products/123",
            "https://example.com/products/123/",
        ),
        (
            "https://example.com/products/123/",
            "https://example.com/products/123",
        ),
    ] {
        let original = Url::parse(original).unwrap();
        let final_url = Url::parse(final_url).unwrap();

        for pattern in [
            Some(r"^https://example\.com/products/\d+$"),
            Some(r"^https://example\.com/products/\d+/$"),
            None,
        ] {
            assert!(
                !is_redirect_to_non_product_page(&original, &final_url, pattern),
                "{original} -> {final_url} under {pattern:?}"
            );
        }
    }
}

#[test]
fn should_detect_redirect_from_slashed_product_url_to_another_page_outside_pattern() {
    let original = Url::parse("https://example.com/products/123/").unwrap();
    let final_url = Url::parse("https://example.com/collections/chairs/").unwrap();

    assert!(is_redirect_to_non_product_page(
        &original,
        &final_url,
        Some(r"/products/\d+$")
    ));
}

#[test]
fn should_not_reject_non_homepage_redirect_when_pattern_is_missing() {
    let original = Url::parse("https://example.com/products/123").unwrap();
    let final_url = Url::parse("https://example.com/collections/chairs").unwrap();

    assert!(!is_redirect_to_non_product_page(
        &original, &final_url, None
    ));
}

#[test]
fn should_not_reject_non_homepage_redirect_when_pattern_is_invalid() {
    let original = Url::parse("https://example.com/products/123").unwrap();
    let final_url = Url::parse("https://example.com/collections/chairs").unwrap();

    assert!(!is_redirect_to_non_product_page(
        &original,
        &final_url,
        Some("[")
    ));
}

#[tokio::test]
async fn should_withdraw_product_when_product_url_redirects_to_homepage() {
    let id = listing_source_id();
    let url = product_url();
    let homepage = Url::parse("https://example.com/").unwrap();

    let mut fetcher = MockHtmlFetcher::new();
    let final_url = homepage.clone();
    fetcher.expect_fetch().once().returning(move |_| {
        let final_url = final_url.clone();
        Box::pin(async move { Ok(fetch_result_for(sample_html(), final_url)) })
    });

    let schema_svc = MockProductListingSchemaService::new();

    let mut cand_svc = MockScraperCandidateService::new();
    let url_for_set_presence = url.clone();
    cand_svc
        .expect_set_disposition()
        .never()
        .withf(
            move |received_listing_source_id, received_url, received_state, _| {
                *received_listing_source_id == id
                    && received_url == &url_for_set_presence
                    && *received_state == CrawlerDisposition::Active
            },
        )
        .returning(|_, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );

    let err = service
        .scrape(primary_only_request(&id, &url))
        .await
        .unwrap_err();

    assert!(matches!(err, ScraperError::ProductListingRemoved { .. }));
}

#[tokio::test]
async fn should_withdraw_product_when_redirected_url_does_not_match_product_pattern() {
    let id = listing_source_id();
    let url = Url::parse("https://www.ebth.com/items/7055109-digital-print-television-poster-dirt")
        .unwrap();
    let category = Url::parse(
        "https://www.ebth.com/collectibles/memorabilia/television-and-movie-memorabilia",
    )
    .unwrap();

    let mut fetcher = MockHtmlFetcher::new();
    let final_url = category.clone();
    fetcher.expect_fetch().once().returning(move |_| {
        let final_url = final_url.clone();
        Box::pin(async move { Ok(fetch_result_for(sample_html(), final_url)) })
    });

    let schema_svc = MockProductListingSchemaService::new();

    let mut cand_svc = MockScraperCandidateService::new();
    let url_for_set_presence = url.clone();
    cand_svc
        .expect_set_disposition()
        .never()
        .withf(
            move |received_listing_source_id, received_url, received_state, _| {
                *received_listing_source_id == id
                    && received_url == &url_for_set_presence
                    && *received_state == CrawlerDisposition::Active
            },
        )
        .returning(|_, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );

    let err = service
        .scrape(primary_only_request_with_pattern(&id, &url, r"/items/"))
        .await
        .unwrap_err();

    assert!(matches!(err, ScraperError::ProductListingRemoved { .. }));
}

/// A shop that serves a page under one trailing-slash spelling redirects the other one there.
/// The page is captured under the URL it is stored under: the redirect is neither a move nor a
/// removal, whichever spelling the pattern was written for.
#[tokio::test]
async fn should_capture_product_under_stored_url_when_redirect_only_changes_trailing_slash() {
    let id = listing_source_id();
    for (stored, served) in [
        (
            "https://example.com/products/123",
            "https://example.com/products/123/",
        ),
        (
            "https://example.com/products/123/",
            "https://example.com/products/123",
        ),
    ] {
        let url = Url::parse(stored).unwrap();
        let served = Url::parse(served).unwrap();
        let mut fetcher = MockHtmlFetcher::new();
        fetcher.expect_fetch().once().returning(move |_| {
            let served = served.clone();
            Box::pin(async move { Ok(fetch_result_for(sample_html(), served)) })
        });
        let schema = listing_source_product_schemas(id);
        let mut schema_svc = MockProductListingSchemaService::new();
        schema_svc
            .expect_find_product_schema()
            .once()
            .returning(move |_| {
                let schema = schema.clone();
                Box::pin(async move { Ok(Some(schema)) })
            });
        let service = ScraperServiceImpl::new_with_schema_seed_pages(
            Box::new(fetcher),
            Box::new(schema_svc),
            Arc::new(MockScraperCandidateService::new()),
            1,
            DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
        );

        let scraped = service
            .scrape(primary_only_request_with_pattern(
                &id,
                &url,
                r"^https://example\.com/products/\d+$",
            ))
            .await
            .result
            .unwrap_or_else(|error| panic!("{stored}: {error:?}"))
            .unwrap_or_else(|| panic!("{stored}: the page must be captured"));

        assert_eq!(scraped.effective_url, url, "{stored}");
        assert_eq!(
            scraped.raw_input.raw_values().value()["url"]["value"],
            serde_json::json!(stored)
        );
    }
}

fn scraper_with_redirect_disallowed(final_url: Url) -> ScraperServiceImpl {
    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let final_url = final_url.clone();
        Box::pin(async move {
            Err(
                crate::scraper::scraper_service::html_fetcher::FetchError::RedirectDisallowed {
                    final_url,
                },
            )
        })
    });
    ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(MockProductListingSchemaService::new()),
        Arc::new(MockScraperCandidateService::new()),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    )
}

#[tokio::test]
async fn should_withdraw_product_when_it_redirects_to_a_non_product_page_robots_txt_disallows() {
    let id = listing_source_id();
    let url = Url::parse("https://example.com/items/1").unwrap();
    let service =
        scraper_with_redirect_disallowed(Url::parse("https://example.com/sold/").unwrap());

    let err = service
        .scrape(primary_only_request_with_pattern(&id, &url, r"/items/"))
        .await
        .unwrap_err();

    assert!(
        matches!(err, ScraperError::ProductListingRemoved { .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn should_refuse_product_when_it_redirects_to_a_product_page_robots_txt_disallows() {
    let id = listing_source_id();
    let url = Url::parse("https://example.com/items/1").unwrap();
    let service =
        scraper_with_redirect_disallowed(Url::parse("https://example.com/items/1-moved").unwrap());

    let err = service
        .scrape(primary_only_request_with_pattern(&id, &url, r"/items/"))
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            ScraperError::HttpError {
                kind: crate::network::policy::NetworkErrorKind::RobotsDisallowed,
                ..
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn should_report_the_shop_responsive_when_it_redirects_to_a_url_robots_txt_disallows() {
    let id = listing_source_id();
    let url = Url::parse("https://example.com/items/1").unwrap();
    for final_url in [
        "https://example.com/sold/",
        "https://example.com/items/1-moved",
    ] {
        let service = scraper_with_redirect_disallowed(Url::parse(final_url).unwrap());

        let outcome = with_test_scrape_domain(
            service.scrape(primary_only_request_with_pattern(&id, &url, r"/items/")),
        )
        .await;

        assert_eq!(
            outcome.domain_health,
            crate::scraper::scraper_service::domain::product::DomainFetchHealth::Responsive,
            "{final_url}"
        );
    }
}
