use super::*;
use crate::scraper::candidate_service::RedirectDestination;
use crate::scraper::scraper_service::domain::errors::{
    ListingRemoval, RemovalEvidenceKind, ScraperError,
};
use crate::scraper::scraper_service::pipeline::scrape_product::{
    Redirect, RedirectTarget, is_redirect_to_non_product_page,
};
use test_api::IntegrationTestService as _;

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

    assert!(matches!(err, ScraperError::ProductListingRemoved(_)));
}

/// A category page a sold item redirects to, which the cached product schemas do not apply to.
fn category_html() -> String {
    r#"<!DOCTYPE html>
    <html>
    <body>
      <main>
        <h2>Television and movie memorabilia</h2>
        <ul><li><a href="/items/7055110-poster">Poster</a></li></ul>
      </main>
    </body>
    </html>"#
        .to_string()
}

/// A scraper whose shop answers every request with `html` under `served`, and whose ListingSource
/// has the cached schemas of [`listing_source_product_schemas`], which a scrape that ends at the
/// redirect never reads.
fn scraper_serving(html: String, served: Url, id: ListingSourceId) -> ScraperServiceImpl {
    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let (html, served) = (html.clone(), served.clone());
        Box::pin(async move { Ok(fetch_result_for(html, served)) })
    });
    let schema = listing_source_product_schemas(id);
    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .times(..=1)
        .returning(move |_| {
            let schema = schema.clone();
            Box::pin(async move { Ok(Some(schema)) })
        });
    ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Arc::new(MockScraperCandidateService::new()),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    )
}

fn removal_of(error: ScraperError) -> ListingRemoval {
    match error {
        ScraperError::ProductListingRemoved(removal) => *removal,
        other => panic!("expected a removal, got {other:?}"),
    }
}

/// A redirect outside the product URL pattern alone is no removal evidence; a target that every
/// cached schema fails on is.
#[tokio::test]
async fn should_withdraw_product_when_it_redirects_outside_the_pattern_to_a_page_every_cached_schema_fails_on()
 {
    let id = listing_source_id();
    let url = Url::parse("https://www.ebth.com/items/7055109-digital-print-television-poster-dirt")
        .unwrap();
    let category = Url::parse(
        "https://www.ebth.com/collectibles/memorabilia/television-and-movie-memorabilia",
    )
    .unwrap();
    let service = scraper_serving(category_html(), category.clone(), id);

    let removal = removal_of(
        service
            .scrape(primary_only_request_with_pattern(&id, &url, r"/items/"))
            .await
            .unwrap_err(),
    );

    assert_eq!(removal.url, url);
    assert_eq!(
        removal.evidence.kind,
        RemovalEvidenceKind::RedirectToNonProductPage
    );
    assert_eq!(removal.evidence.final_url, Some(category));
    assert_eq!(removal.evidence.status, Some(200));
}

/// Without a cached schema nothing shows whether a page outside the pattern is a product page, so
/// the redirect neither withdraws the listing nor makes the target a page a schema review or
/// generation starts from: no schema is authored and no LLM call is spent.
#[tokio::test]
async fn should_leave_the_url_unverified_when_it_redirects_outside_the_pattern_before_any_schema_is_cached()
 {
    let id = listing_source_id();
    let url = Url::parse("https://example.com/items/123").unwrap();
    let category = Url::parse("https://example.com/collectibles/memorabilia").unwrap();
    let mut fetcher = MockHtmlFetcher::new();
    let served = category.clone();
    fetcher.expect_fetch().once().returning(move |_| {
        let served = served.clone();
        Box::pin(async move { Ok(fetch_result_for(category_html(), served)) })
    });
    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc
        .expect_find_product_schema()
        .once()
        .returning(|_| Box::pin(async { Ok(None) }));
    schema_svc.expect_create_product_schemas().never();
    schema_svc.expect_generate_single_schema_for_page().never();
    let mut cand_svc = MockScraperCandidateService::new();
    cand_svc
        .expect_try_increment_listing_source_llm_calls_with_limit()
        .never();
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

    assert!(
        matches!(&err, ScraperError::RedirectTargetUnverified { destination }
            if *destination == category),
        "{err:?}"
    );
}

/// A redirect outside the pattern to a page a cached schema still extracts leads to a product
/// page, so the product is captured under the target instead of being withdrawn.
#[tokio::test]
async fn should_capture_product_under_its_target_when_it_redirects_outside_the_pattern_to_a_page_a_cached_schema_extracts()
 {
    let id = listing_source_id();
    let url = Url::parse("https://example.com/items/123").unwrap();
    let moved = Url::parse("https://example.com/catalogue/123-biedermeier-chair").unwrap();
    let service = scraper_serving(sample_html(), moved.clone(), id);

    let scraped = service
        .scrape(primary_only_request_with_pattern(&id, &url, r"/items/"))
        .await
        .result
        .unwrap_or_else(|error| panic!("{error:?}"))
        .expect("the page must be captured");

    assert_eq!(scraped.effective_url, moved);
}

/// Butchoff answers a browser-style request for every product with a redirect to the same path
/// behind a locale segment. The live page is captured under the URL it is stored under, however
/// the language segment changes, and is never withdrawn.
#[tokio::test]
async fn should_capture_product_under_stored_url_when_redirect_only_changes_the_language_segment() {
    let id = listing_source_id();
    for (stored, served, pattern) in [
        (
            "https://www.butchoff.com/artworkdetail/769450/17445/a-substantial-wine-cooler",
            "https://www.butchoff.com/en-us/artworkdetail/769450/17445/a-substantial-wine-cooler",
            r"^https?://[^/]+/artworkdetail/[0-9]+/[0-9]+/[^/?#]+$",
        ),
        (
            "https://example.com/en/products/chair",
            "https://example.com/products/chair/",
            r"^https://example\.com/en/products/[^/]+$",
        ),
        (
            "https://example.com/en/products/chair",
            "https://example.com/de-de/products/chair",
            r"^https://example\.com/en/products/[^/]+$",
        ),
    ] {
        let url = Url::parse(stored).unwrap();
        let service = scraper_serving(sample_html(), Url::parse(served).unwrap(), id);

        let scraped = service
            .scrape(primary_only_request_with_pattern(&id, &url, pattern))
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

#[tokio::test]
async fn should_withdraw_product_when_it_redirects_to_the_homepage_of_a_language() {
    let id = listing_source_id();
    let url = Url::parse("https://example.com/en-us/products/123").unwrap();
    let homepage = Url::parse("https://example.com/en-us/").unwrap();
    let service = scraper_serving(sample_html(), homepage.clone(), id);

    let removal = removal_of(
        service
            .scrape(primary_only_request_with_pattern(&id, &url, r"/products/"))
            .await
            .unwrap_err(),
    );

    assert_eq!(
        removal.evidence.kind,
        RemovalEvidenceKind::RedirectToHomepage
    );
    assert_eq!(removal.evidence.final_url, Some(homepage));
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
async fn should_withdraw_product_when_it_redirects_to_a_homepage_robots_txt_disallows() {
    let id = listing_source_id();
    let url = Url::parse("https://example.com/items/1").unwrap();
    let homepage = Url::parse("https://example.com/").unwrap();
    let service = scraper_with_redirect_disallowed(homepage.clone());

    let removal = removal_of(
        service
            .scrape(primary_only_request_with_pattern(&id, &url, r"/items/"))
            .await
            .unwrap_err(),
    );

    assert_eq!(
        removal.evidence.kind,
        RemovalEvidenceKind::RedirectToHomepage
    );
    assert_eq!(removal.evidence.final_url, Some(homepage));
    assert_eq!(removal.evidence.status, None);
}

/// A target robots.txt disallows is not fetched, so its URL alone judges it: a page the pattern
/// takes, or the page itself behind a language segment, is refused and retried.
#[tokio::test]
async fn should_refuse_product_when_it_redirects_to_a_product_page_robots_txt_disallows() {
    let id = listing_source_id();
    let url = Url::parse("https://example.com/items/1").unwrap();
    for target in [
        "https://example.com/items/1-moved",
        "https://example.com/en/items/1",
        "https://example.com/en-us/items/2",
    ] {
        let service = scraper_with_redirect_disallowed(Url::parse(target).unwrap());

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
            "{target}: {err:?}"
        );
    }
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

        let outcome = with_test_scrape_observation(
            service.scrape(primary_only_request_with_pattern(&id, &url, r"/items/")),
        )
        .await;

        assert_eq!(
            outcome.scrape_health,
            crate::scraper::scraper_service::domain::product::DomainFetchHealth::Responsive,
            "{final_url}"
        );
    }
}

/// A sold item a shop sends to a page robots.txt disallows outside the product URL pattern is a
/// verified removal, judged by the target's URL alone, as is one sent to a disallowed homepage.
#[tokio::test]
async fn should_withdraw_product_when_it_redirects_outside_the_pattern_to_a_page_robots_txt_disallows()
 {
    let id = listing_source_id();
    let url = Url::parse("https://example.com/items/1").unwrap();
    for target in [
        "https://example.com/sold/",
        "https://www.example.com/en-us/sold/chairs",
    ] {
        let target = Url::parse(target).unwrap();
        let service = scraper_with_redirect_disallowed(target.clone());

        let removal = removal_of(
            service
                .scrape(primary_only_request_with_pattern(&id, &url, r"/items/"))
                .await
                .unwrap_err(),
        );

        assert_eq!(removal.url, url);
        assert_eq!(
            removal.evidence.kind,
            RemovalEvidenceKind::RedirectToDisallowedPage,
            "{target}"
        );
        assert_eq!(removal.evidence.final_url, Some(target));
        assert_eq!(removal.evidence.status, None);
    }
}

/// Removal evidence is read only from the shop's own host: a disallowed page elsewhere, or
/// without a product URL pattern to be outside of, is refused.
#[tokio::test]
async fn should_refuse_product_when_a_disallowed_redirect_target_shows_no_removal() {
    let id = listing_source_id();
    let url = Url::parse("https://example.com/items/1").unwrap();
    for (target, pattern) in [
        ("https://elsewhere.example.org/sold/", Some(r"/items/")),
        ("https://elsewhere.example.org/", Some(r"/items/")),
        ("https://example.com/sold/", None),
    ] {
        let service = scraper_with_redirect_disallowed(Url::parse(target).unwrap());
        let request = match pattern {
            Some(pattern) => primary_only_request_with_pattern(&id, &url, pattern),
            None => primary_only_request(&id, &url),
        };

        let err = service.scrape(request).await.unwrap_err();

        assert!(
            matches!(
                err,
                ScraperError::HttpError {
                    kind: crate::network::policy::NetworkErrorKind::RobotsDisallowed,
                    ..
                }
            ),
            "{target}: {err:?}"
        );
    }
}

/// A scraper whose shop answers every request with `html` under `served` and whose crawler stores
/// `destination` at the target. Nothing is read from the schema cache: a scrape that settles at
/// the destination extracts nothing.
fn scraper_with_stored_destination(
    served: Url,
    destination: RedirectDestination,
) -> ScraperServiceImpl {
    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |_| {
        let served = served.clone();
        Box::pin(async move { Ok(fetch_result_for(sample_html(), served)) })
    });
    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc.expect_find_product_schema().never();
    schema_svc.expect_create_product_schemas().never();
    let mut cand_svc = MockScraperCandidateService::new();
    cand_svc
        .expect_redirect_destination()
        .once()
        .returning(move |_, _| Box::pin(async move { Ok(destination) }));
    ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    )
}

/// A request of a URL captured as `LADLE-7`, which the destination page shows another identity
/// for: the extraction would be an identity conflict retried on every pass.
fn request_captured_as_another_identity(id: &ListingSourceId, url: &Url) -> ScrapeRequest {
    ScrapeRequest {
        last_source_listing_id: Some("LADLE-7".to_owned()),
        ..normal_request_with_pattern(id, url, r"/products/")
    }
}

/// A redirect to a different product of the ListingSource, stored under its own row, ends the
/// scrape at the fetch: the URL is retired as that page's duplicate instead of extracting the
/// page, comparing identities and failing every pass with an identity conflict.
#[tokio::test]
async fn should_retire_a_url_that_redirects_to_another_stored_product_before_extracting_it() {
    let id = listing_source_id();
    let url = Url::parse("https://example.com/products/ladle").unwrap();
    let target = Url::parse("https://example.com/products/spoon").unwrap();
    let service = scraper_with_stored_destination(
        target.clone(),
        RedirectDestination::ProductOfListingSource,
    );

    let err = service
        .scrape(request_captured_as_another_identity(&id, &url))
        .await
        .unwrap_err();

    assert!(
        matches!(&err, ScraperError::RedirectToKnownProduct { destination }
            if *destination == target),
        "{err:?}"
    );
}

/// A page the crawler stores as no product page of the ListingSource is a removal before anything
/// is extracted, whatever identity the page shows.
#[tokio::test]
async fn should_withdraw_a_url_that_redirects_to_a_stored_non_product_page_before_extracting_it() {
    let id = listing_source_id();
    let url = Url::parse("https://example.com/products/ladle").unwrap();
    let target = Url::parse("https://example.com/products/ladles").unwrap();
    let service = scraper_with_stored_destination(
        target.clone(),
        RedirectDestination::NotAProductOfListingSource,
    );

    let removal = removal_of(
        service
            .scrape(request_captured_as_another_identity(&id, &url))
            .await
            .unwrap_err(),
    );

    assert_eq!(removal.url, url);
    assert_eq!(
        removal.evidence.kind,
        RemovalEvidenceKind::RedirectToStoredNonProductPage
    );
    assert_eq!(removal.evidence.final_url, Some(target));
    assert_eq!(removal.evidence.status, Some(200));
}

#[tokio::test]
async fn should_leave_a_url_unsettled_when_the_rows_at_its_redirect_target_disagree() {
    let id = listing_source_id();
    let url = Url::parse("https://example.com/products/ladle").unwrap();
    let target = Url::parse("https://example.com/products/spoon").unwrap();
    let service = scraper_with_stored_destination(target.clone(), RedirectDestination::Unsettled);

    let err = service
        .scrape(request_captured_as_another_identity(&id, &url))
        .await
        .unwrap_err();

    assert!(
        matches!(&err, ScraperError::RedirectDestinationUnsettled { destination, .. }
            if *destination == target),
        "{err:?}"
    );
}

/// A target no row stores is judged by what the scrape extracts from it, and the row moves there.
#[tokio::test]
async fn should_extract_the_page_when_no_row_stores_the_redirect_target() {
    let id = listing_source_id();
    let url = Url::parse("https://example.com/products/ladle").unwrap();
    let target = Url::parse("https://example.com/products/spoon").unwrap();
    let mut fetcher = MockHtmlFetcher::new();
    let served = target.clone();
    fetcher.expect_fetch().once().returning(move |_| {
        let served = served.clone();
        Box::pin(async move { Ok(fetch_result_for(sample_html(), served)) })
    });
    let schema = listing_source_product_schemas(id);
    let mut schema_svc = MockProductListingSchemaService::new();
    schema_svc.expect_find_product_schema().returning(move |_| {
        let schema = schema.clone();
        Box::pin(async move { Ok(Some(schema)) })
    });
    let mut cand_svc = MockScraperCandidateService::new();
    cand_svc
        .expect_redirect_destination()
        .once()
        .returning(|_, _| Box::pin(async { Ok(RedirectDestination::Unknown) }));
    let service = ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schema_svc),
        Arc::new(cand_svc),
        1,
        DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
    );

    let scraped = service
        .scrape(normal_request_with_pattern(&id, &url, r"/products/"))
        .await
        .result
        .unwrap_or_else(|error| panic!("{error:?}"))
        .expect("the page must be captured");

    assert_eq!(scraped.effective_url, target);
}

/// Butchoff sends `/artworkdetail/<id>/<slug>` to `/en-us/artworkdetail/<id>/<slug>`. A redirect
/// that also changes the path and adds a language segment moves the row to the target without the
/// segment, which the pattern learned from unprefixed URLs takes, so the row stays a product URL.
#[tokio::test]
async fn should_move_the_row_to_the_unprefixed_target_when_a_redirect_changes_the_path_and_adds_a_language_segment()
 {
    let id = listing_source_id();
    let pattern = r"^https://www\.butchoff\.com/artworkdetail/[0-9]+/[0-9]+/[^/?#]+$";
    let url = Url::parse("https://www.butchoff.com/artworkdetail/1/2/old-name").unwrap();
    let served = Url::parse("https://www.butchoff.com/en-us/artworkdetail/1/2/new-name").unwrap();
    let service = scraper_serving(sample_html(), served, id);

    let scraped = service
        .scrape(primary_only_request_with_pattern(&id, &url, pattern))
        .await
        .result
        .unwrap_or_else(|error| panic!("{error:?}"))
        .expect("the page must be captured");

    assert_eq!(
        scraped.effective_url.as_str(),
        "https://www.butchoff.com/artworkdetail/1/2/new-name"
    );
    assert_eq!(
        scraped.raw_input.raw_values().value()["url"]["value"],
        serde_json::json!("https://www.butchoff.com/artworkdetail/1/2/new-name")
    );
}

#[test]
fn should_store_the_target_spelling_the_pattern_takes_for_a_redirect_that_adds_a_language_segment()
{
    let pattern = Some(r"^https://shop\.example/item/");
    let stored = |original: &str, served: &str, pattern| {
        Redirect::of(
            &Url::parse(original).unwrap(),
            &Url::parse(served).unwrap(),
            pattern,
        )
    };

    // Unprefixed row, prefixed target the pattern only takes without the segment.
    let moved = stored(
        "https://shop.example/item/1",
        "https://shop.example/en-us/item/2",
        pattern,
    );
    assert_eq!(moved.target, RedirectTarget::ProductPage);
    assert_eq!(moved.url.as_str(), "https://shop.example/item/2");
    // A row stored with its segment keeps the target's own spelling.
    let prefixed_pattern = Some(r"^https://shop\.example/(en|de)/item/");
    let kept = stored(
        "https://shop.example/en/item/1",
        "https://shop.example/de/item/2",
        prefixed_pattern,
    );
    assert_eq!(kept.target, RedirectTarget::ProductPage);
    assert_eq!(kept.url.as_str(), "https://shop.example/de/item/2");
    // A first segment that merely looks like a language is not dropped where the pattern does not
    // take the rest.
    let looks_like_language = stored(
        "https://shop.example/item/1",
        "https://shop.example/art/12",
        pattern,
    );
    assert_eq!(
        looks_like_language.target,
        RedirectTarget::OutsideProductPattern
    );
    assert_eq!(
        looks_like_language.url.as_str(),
        "https://shop.example/art/12"
    );
    // Without a usable pattern nothing says the segment is not part of the page.
    let unjudged = stored(
        "https://shop.example/item/1",
        "https://shop.example/en-us/item/2",
        None,
    );
    assert_eq!(unjudged.target, RedirectTarget::ProductPage);
    assert_eq!(unjudged.url.as_str(), "https://shop.example/en-us/item/2");
}

/// Stores a crawler URL row of `class` of `listing_source_id`.
async fn store_crawler_url(
    pool: &sqlx::PgPool,
    listing_source_id: ListingSourceId,
    url: &Url,
    class: crate::spider::classification::url_metadata::UrlClass,
    last_source_listing_id: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO listing_source_urls \
             (listing_source_id, url, raw_source_record_key, url_class, \
              crawler_disposition, last_source_listing_id, created, updated) \
         VALUES ($1, $2, $2, $3, $4, $5, NOW(), NOW())",
    )
    .bind(listing_source_id.as_uuid())
    .bind(url.as_str())
    .bind(class.as_str())
    .bind(CrawlerDisposition::Active.as_str())
    .bind(last_source_listing_id)
    .execute(pool)
    .await
    .unwrap_or_else(|error| panic!("insert crawler URL: {error}"));
}

/// The rows the crawler stores settle a redirected URL before its page is extracted: the schema
/// cache is never read. A URL captured as `LADLE-7` that the shop redirects to another stored
/// product of the ListingSource is retired as that page's duplicate instead of failing every pass
/// as an identity conflict, and one redirected to a page stored as a non-product page is removed.
#[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
async fn should_settle_a_redirect_by_the_stored_rows_at_its_target_before_extracting_it() {
    use crate::spider::classification::url_metadata::UrlClass;
    let pool = test_api::get_postgres_client().await;
    let id = listing_source_id();
    sqlx::query(
        "INSERT INTO listing_sources \
         (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
         VALUES ($1, 'Redirect source', 'redirect-source', TRUE, NOW(), NOW())",
    )
    .bind(id.as_uuid())
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("insert listing source: {error}"));
    crate::test_support::store_crawl_root(&pool, id, "https://example.com/").await;
    let url = Url::parse("https://example.com/products/ladle").unwrap();
    let product = Url::parse("https://example.com/products/spoon").unwrap();
    let category = Url::parse("https://example.com/products/ladles").unwrap();
    store_crawler_url(&pool, id, &url, UrlClass::ProductListing, Some("LADLE-7")).await;
    store_crawler_url(
        &pool,
        id,
        &product,
        UrlClass::ProductListing,
        Some("SPOON-3"),
    )
    .await;
    store_crawler_url(&pool, id, &category, UrlClass::Category, None).await;
    let scraper_redirecting_to = |served: Url| {
        let mut fetcher = MockHtmlFetcher::new();
        fetcher.expect_fetch().once().returning(move |_| {
            let served = served.clone();
            Box::pin(async move { Ok(fetch_result_for(sample_html(), served)) })
        });
        let mut schema_svc = MockProductListingSchemaService::new();
        schema_svc.expect_find_product_schema().never();
        schema_svc.expect_create_product_schemas().never();
        ScraperServiceImpl::new_with_schema_seed_pages(
            Box::new(fetcher),
            Box::new(schema_svc),
            Arc::new(
                crate::scraper::candidate_service::ScraperCandidateServiceImpl::new(pool.clone()),
            ),
            1,
            DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
        )
    };
    let request = request_captured_as_another_identity(&id, &url);

    let duplicate = scraper_redirecting_to(product.clone())
        .scrape(request.clone())
        .await
        .result
        .unwrap_err();
    let removal = removal_of(
        scraper_redirecting_to(category.clone())
            .scrape(request)
            .await
            .result
            .unwrap_err(),
    );

    assert!(
        matches!(&duplicate, ScraperError::RedirectToKnownProduct { destination }
            if *destination == product),
        "{duplicate:?}"
    );
    assert_eq!(removal.url, url);
    assert_eq!(
        removal.evidence.kind,
        RemovalEvidenceKind::RedirectToStoredNonProductPage
    );
    assert_eq!(removal.evidence.final_url, Some(category));
}
