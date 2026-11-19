//! A captured URL whose page shows another identity under unchanged schemas
//! (`docs/crawler/scheduling-and-retries.md#captured-identity`).

use crate::scraper::candidate_service::ScraperCandidate;
use crate::scraper::scraper_service::ScrapedProduct;
use crate::spider::classification::url_metadata::CrawlerUrlWriteOutcome;
use listing_source_core::ListingSourceId;
use tracing::warn;

/// A page whose own identity changed while the schemas did not: the selected schema extracts
/// another identity from the URL than it did at the URL's last capture under the same schema set,
/// so the shop likely shows another product at the URL. The scrape still captures under the URL's
/// captured identity when no other URL carries the new one
/// (`docs/crawler/scheduling-and-retries.md#captured-identity`). A schema change, or a page that
/// extracts no identity, such as a sold page without it, shows nothing about the product.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PageIdentityChange {
    listing_source_id: ListingSourceId,
    url: url::Url,
    listing_source_name: String,
    domain: String,
    captured_identity: String,
    previous_page_identity: String,
    page_identity: String,
}

impl PageIdentityChange {
    /// The change the scrape of `candidate` shows, when the identity its unredirected page shows
    /// under the same schema set differs from the one recorded at its last capture.
    pub(super) fn of(candidate: &ScraperCandidate, scraped: &ScrapedProduct) -> Option<Self> {
        let previous = candidate.last_extracted_source_listing_id.as_deref()?;
        let current = scraped.extracted_source_listing_id.as_deref()?;
        let same_schemas = candidate.last_scraped_schema_fingerprint.as_deref()
            == Some(&scraped.schema_fingerprint);
        (previous != current && same_schemas && candidate.url == scraped.effective_url).then(|| {
            Self {
                listing_source_id: candidate.listing_source_id,
                url: candidate.url.clone(),
                listing_source_name: candidate.listing_source_name.clone(),
                domain: candidate.crawl_host.clone(),
                captured_identity: candidate.last_source_listing_id.clone().unwrap_or_default(),
                previous_page_identity: previous.to_owned(),
                page_identity: current.to_owned(),
            }
        })
    }
}

/// Logs `change` once the write that records it (`page_identity_changed_at`) applied, so a stale
/// or failed write, which the next pass repeats, logs nothing; answers whether it logged.
pub(super) fn report_page_identity_change(
    change: Option<&PageIdentityChange>,
    write_result: &Result<CrawlerUrlWriteOutcome, sqlx::Error>,
) -> bool {
    let (Some(change), Ok(CrawlerUrlWriteOutcome::Applied)) = (change, write_result) else {
        return false;
    };
    warn!(
        event = "crawler.product_url_page_identity_changed",
        listing_source_id = %change.listing_source_id,
        listing_source_name = %change.listing_source_name,
        domain = %change.domain,
        url = %change.url,
        captured_identity = %change.captured_identity,
        previous_page_identity = %change.previous_page_identity,
        page_identity = %change.page_identity,
        "A product page shows another identity under unchanged schemas; capturing it under its captured identity"
    );
    true
}

#[cfg(test)]
mod tests {
    use super::super::RawCaptureSuccessAction;
    use super::super::tests::{scrape_candidate, scrape_candidate_context, scrape_outcome};
    use super::*;
    use crate::scraper::candidate_service::MockScraperCandidateService;
    use crate::scraper::raw_input::crawler_verified_removal_input;
    use crate::scraper::scraper_service::MockScraperService;
    use crate::service::cron::test_support::scraper_candidate;

    const URL: &str = "https://example.com/products/chair";

    fn candidate() -> ScraperCandidate {
        let mut candidate = scraper_candidate("ListingSource", url::Url::parse(URL).unwrap());
        candidate.last_source_listing_id = Some("SKU-1".to_owned());
        candidate.last_extracted_source_listing_id = Some("SKU-1".to_owned());
        candidate.last_scraped_schema_fingerprint = Some("schemas".to_owned());
        candidate
    }

    fn scraped(extracted: Option<&str>, fingerprint: &str, url: &str) -> ScrapedProduct {
        let effective_url = url::Url::parse(url).unwrap();
        ScrapedProduct {
            acquired_at: time::OffsetDateTime::now_utc(),
            raw_input: crawler_verified_removal_input(&effective_url)
                .unwrap_or_else(|error| panic!("test raw input: {error}")),
            hash: "hash".to_owned(),
            schema_fingerprint: fingerprint.to_owned(),
            raw_input_sha256: vec![7; 32],
            effective_url,
            source_listing_id: "SKU-1".to_owned(),
            extracted_source_listing_id: extracted.map(str::to_owned),
            title: Some("Chair".to_owned()),
        }
    }

    #[test]
    fn should_report_another_page_identity_under_unchanged_schemas() {
        let candidate = candidate();
        let scraped = scraped(Some("SKU-2"), "schemas", URL);

        let change = PageIdentityChange::of(&candidate, &scraped).unwrap();

        assert_eq!(
            (
                change.captured_identity.as_str(),
                change.previous_page_identity.as_str(),
                change.page_identity.as_str()
            ),
            ("SKU-1", "SKU-1", "SKU-2")
        );
    }

    #[rstest::rstest]
    #[case::same_identity(Some("SKU-1"), "schemas", URL)]
    #[case::schemas_changed(Some("SKU-2"), "new schemas", URL)]
    #[case::no_identity_on_the_page(None, "schemas", URL)]
    #[case::redirected(Some("SKU-2"), "schemas", "https://example.com/products/table")]
    fn should_report_nothing_without_evidence_of_another_product(
        #[case] extracted: Option<&str>,
        #[case] fingerprint: &str,
        #[case] url: &str,
    ) {
        let candidate = candidate();
        let scraped = scraped(extracted, fingerprint, url);

        assert_eq!(PageIdentityChange::of(&candidate, &scraped), None);
    }

    #[test]
    fn should_report_nothing_before_the_page_identity_was_recorded() {
        let mut candidate = candidate();
        candidate.last_extracted_source_listing_id = None;

        assert_eq!(
            PageIdentityChange::of(&candidate, &scraped(Some("SKU-2"), "schemas", URL)),
            None
        );
    }

    /// The change is logged only once the write that records it applied: a stale or failed
    /// write records nothing, and the next pass sees the change again.
    #[test]
    fn should_report_a_change_only_after_its_write_applied() {
        let (events, _guard) = crate::test_support::logged_events::LoggedEvents::capture();
        let change = PageIdentityChange::of(&candidate(), &scraped(Some("SKU-2"), "schemas", URL));

        assert!(report_page_identity_change(
            change.as_ref(),
            &Ok(CrawlerUrlWriteOutcome::Applied)
        ));
        assert!(!report_page_identity_change(
            change.as_ref(),
            &Ok(CrawlerUrlWriteOutcome::NoopStale)
        ));
        assert!(!report_page_identity_change(
            change.as_ref(),
            &Err(sqlx::Error::PoolTimedOut)
        ));
        assert!(!report_page_identity_change(
            None,
            &Ok(CrawlerUrlWriteOutcome::Applied)
        ));
        assert_eq!(
            events
                .named("crawler.product_url_page_identity_changed")
                .len(),
            1
        );
    }

    /// The capture of a page whose identity changed records the change, and reports it, only
    /// when the collector's write applies.
    #[tokio::test]
    async fn should_record_the_change_with_the_capture_and_report_it_after_the_write() {
        let candidate = candidate();
        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().once().returning(|_| {
            Box::pin(async { scrape_outcome(Ok(Some(scraped(Some("SKU-2"), "schemas", URL)))) })
        });
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_identity_carrier()
            .once()
            .returning(|_, _, _| Box::pin(async { Ok(None) }));
        let ctx = scrape_candidate_context(scraper_candidates, scraper_service);

        let outcome = scrape_candidate(candidate, &ctx).await;

        match &outcome.capture.expect("the page is captured").on_success {
            RawCaptureSuccessAction::MarkScraped(meta) => {
                assert_eq!(meta.source_listing_id, "SKU-1");
                assert!(meta.page_identity_change.is_some());
            }
            _ => panic!("a product page is marked scraped after capture"),
        }
    }
}
