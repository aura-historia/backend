//! A captured URL whose page shows another identity under unchanged schemas
//! (`docs/crawler/scheduling-and-retries.md#captured-identity`): the scrape keeps capturing under
//! the URL's captured identity only when [`same_product_after_identity_change`] shows the page
//! still shows the captured product, and otherwise parks the URL in an identity conflict, so
//! another product's content is never written into the URL's listing.

use super::{ScrapeBatchContext, ScrapeCandidateOutcome, persist_identity_conflict};
use crate::scraper::candidate_service::ScraperCandidate;
use crate::scraper::same_product::{
    ProductUrlIdentity, SameProductEvidence, same_product_after_identity_change,
};
use crate::scraper::scraper_service::ScrapedProduct;
use crate::spider::classification::url_metadata::CrawlerUrlWriteOutcome;
use listing_source_core::ListingSourceId;
use tracing::{debug, warn};

/// The event of a page that shows another identity under unchanged schemas.
const PAGE_IDENTITY_CHANGED_EVENT: &str = "crawler.product_url_page_identity_changed";

/// A page whose own identity changed while the schemas did not: the selected schema extracts
/// another identity from the URL than it did at the URL's last capture under the same schema set,
/// so the shop may show another product at the URL. A schema change, or a page that extracts no
/// identity, such as a sold page without it, shows nothing about the product.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PageIdentityChange {
    listing_source_id: ListingSourceId,
    url: url::Url,
    listing_source_name: String,
    domain: String,
    captured_identity: String,
    previous_page_identity: String,
    page_identity: String,
    /// The signal that the page still shows the captured product; `None` takes it for another.
    evidence: Option<SameProductEvidence>,
}

impl PageIdentityChange {
    /// The change the scrape of `candidate` shows, when the identity its unredirected page shows
    /// under the same schema set differs from the one recorded at its last capture and from the
    /// one the URL's listing was captured under: a page that shows the captured identity again
    /// shows the captured product.
    pub(super) fn of(candidate: &ScraperCandidate, scraped: &ScrapedProduct) -> Option<Self> {
        let previous = candidate.last_extracted_source_listing_id.as_deref()?;
        let current = scraped.extracted_source_listing_id.as_deref()?;
        let same_schemas = candidate.last_scraped_schema_fingerprint.as_deref()
            == Some(&scraped.schema_fingerprint);
        let shows_captured = candidate.last_source_listing_id.as_deref() == Some(current);
        (previous != current
            && !shows_captured
            && same_schemas
            && candidate.url == scraped.effective_url)
            .then(|| Self {
                listing_source_id: candidate.listing_source_id,
                url: candidate.url.clone(),
                listing_source_name: candidate.listing_source_name.clone(),
                domain: candidate.crawl_host.clone(),
                captured_identity: candidate.last_source_listing_id.clone().unwrap_or_default(),
                previous_page_identity: previous.to_owned(),
                page_identity: current.to_owned(),
                evidence: same_product_after_identity_change(
                    ProductUrlIdentity {
                        url: &candidate.url,
                        captured_identity: candidate.last_source_listing_id.as_deref(),
                        title: candidate.last_captured_title.as_deref(),
                    },
                    current,
                    scraped.title.as_deref(),
                ),
            })
    }

    fn evidence_name(&self) -> &'static str {
        self.evidence.map_or("none", SameProductEvidence::as_str)
    }

    fn conflict_message(&self) -> String {
        format!(
            "the page shows another product under unchanged schemas: source listing id \
             `{}` where its last capture showed `{}`, not the product captured here as `{}`",
            self.page_identity, self.previous_page_identity, self.captured_identity
        )
    }
}

/// What the scrape does with a page whose identity changed under unchanged schemas.
pub(super) enum PageIdentity {
    /// The scrape captures under the URL's captured identity, recording the change if any.
    Capture(Option<Box<PageIdentityChange>>),
    /// The page shows another product: the URL was parked in an identity conflict.
    Parked(Box<ScrapeCandidateOutcome>),
}

/// Captures the scrape of `candidate` unless its page shows another identity under unchanged
/// schemas without a signal that it still shows the captured product; parks it in an identity
/// conflict then. Nothing a rescrape sees changes before the page or the schemas do, so the URL
/// stays parked until its page shows the captured product again or the schemas change.
pub(super) async fn settle_page_identity(
    candidate: &ScraperCandidate,
    ctx: &ScrapeBatchContext,
    scraped: &ScrapedProduct,
) -> PageIdentity {
    match PageIdentityChange::of(candidate, scraped) {
        None => PageIdentity::Capture(None),
        Some(change) if change.evidence.is_some() => PageIdentity::Capture(Some(Box::new(change))),
        Some(change) => {
            log_parked(candidate, &change);
            PageIdentity::Parked(Box::new(
                persist_identity_conflict(candidate, ctx, &change.conflict_message()).await,
            ))
        }
    }
}

/// Logs a parked change at WARN on the first conflict of the URL's run, and at DEBUG on each
/// retry that parks it again.
fn log_parked(candidate: &ScraperCandidate, change: &PageIdentityChange) {
    macro_rules! log_at {
        ($level:ident) => {
            $level!(
                event = PAGE_IDENTITY_CHANGED_EVENT,
                listing_source_id = %change.listing_source_id,
                listing_source_name = %change.listing_source_name,
                domain = %change.domain,
                url = %change.url,
                captured_identity = %change.captured_identity,
                previous_page_identity = %change.previous_page_identity,
                page_identity = %change.page_identity,
                evidence = change.evidence_name(),
                outcome = "identity_conflict",
                "A product page shows another product under unchanged schemas; parking it in an identity conflict"
            )
        };
    }
    if candidate.identity_conflict_streak == 0 {
        log_at!(warn);
    } else {
        log_at!(debug);
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
        event = PAGE_IDENTITY_CHANGED_EVENT,
        listing_source_id = %change.listing_source_id,
        listing_source_name = %change.listing_source_name,
        domain = %change.domain,
        url = %change.url,
        captured_identity = %change.captured_identity,
        previous_page_identity = %change.previous_page_identity,
        page_identity = %change.page_identity,
        evidence = change.evidence_name(),
        outcome = "captured",
        "A product page shows another identity under unchanged schemas; capturing it under its captured identity"
    );
    true
}

#[cfg(test)]
mod tests {
    use super::super::tests::{scrape_candidate, scrape_candidate_context, scrape_outcome};
    use super::super::{RawCaptureSuccessAction, SOURCE_LISTING_ID_CONFLICT_ERROR_KIND};
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
        candidate.last_captured_title = Some("Chair".to_owned());
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
                assert_eq!(
                    meta.page_identity_change
                        .as_ref()
                        .and_then(|change| change.evidence),
                    Some(SameProductEvidence::Title)
                );
            }
            _ => panic!("a product page is marked scraped after capture"),
        }
    }

    fn titled(mut scraped: ScrapedProduct, title: &str) -> ScrapedProduct {
        scraped.title = Some(title.to_owned());
        scraped
    }

    #[rstest::rstest]
    #[case::same_title("Chair", Some(SameProductEvidence::Title))]
    #[case::another_title("Table", None)]
    fn should_judge_whether_the_page_still_shows_the_captured_product(
        #[case] title: &str,
        #[case] evidence: Option<SameProductEvidence>,
    ) {
        let scraped = titled(scraped(Some("SKU-2"), "schemas", URL), title);

        let change = PageIdentityChange::of(&candidate(), &scraped).unwrap();

        assert_eq!(change.evidence, evidence);
    }

    /// A page that shows another product under unchanged schemas is parked instead of written
    /// into the URL's listing.
    #[tokio::test]
    async fn should_park_a_page_that_shows_another_product_under_unchanged_schemas() {
        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().once().returning(|_| {
            let scraped = titled(scraped(Some("SKU-2"), "schemas", URL), "Table");
            Box::pin(async { scrape_outcome(Ok(Some(scraped))) })
        });
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_identity_carrier()
            .once()
            .returning(|_, _, _| Box::pin(async { Ok(None) }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(|_, url, kind, message, _, _, _| {
                url.as_str() == URL
                    && kind == SOURCE_LISTING_ID_CONFLICT_ERROR_KIND
                    && message.contains("`SKU-2` where its last capture showed `SKU-1`")
            })
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });
        let ctx = scrape_candidate_context(scraper_candidates, scraper_service);

        let outcome = scrape_candidate(candidate(), &ctx).await;

        assert!(outcome.capture.is_none());
        assert_eq!(
            outcome.failure_kind.as_deref(),
            Some(SOURCE_LISTING_ID_CONFLICT_ERROR_KIND)
        );
    }

    /// The first conflict of a run warns; each retry that parks the URL again logs at DEBUG.
    #[rstest::rstest]
    #[case::first_conflict(0, tracing::Level::WARN)]
    #[case::retry(1, tracing::Level::DEBUG)]
    fn should_warn_of_a_parked_page_once_per_conflict_run(
        #[case] streak: u32,
        #[case] level: tracing::Level,
    ) {
        let (events, _guard) = crate::test_support::logged_events::LoggedEvents::capture();
        let mut candidate = candidate();
        candidate.identity_conflict_streak = streak;
        let change = PageIdentityChange::of(
            &candidate,
            &titled(scraped(Some("SKU-2"), "schemas", URL), "Table"),
        )
        .unwrap();

        log_parked(&candidate, &change);

        let logged = events.named(PAGE_IDENTITY_CHANGED_EVENT);
        assert_eq!(logged.len(), 1);
        assert_eq!(logged[0].level, level);
        assert_eq!(logged[0].field("outcome"), Some("identity_conflict"));
        assert_eq!(logged[0].field("evidence"), Some("none"));
    }

    /// A renumbered page captured as `SKU-1` and since showing `A-1001` that shows `SKU-1`
    /// again shows the captured product, whatever its title now.
    #[test]
    fn should_report_nothing_when_the_page_shows_the_captured_identity_again() {
        let mut candidate = candidate();
        candidate.last_extracted_source_listing_id = Some("A-1001".to_owned());
        let scraped = titled(scraped(Some("SKU-1"), "schemas", URL), "Chair - sold");

        assert_eq!(PageIdentityChange::of(&candidate, &scraped), None);
    }

    /// A parked URL whose page shows its previous identity again is captured: nothing parks it.
    #[tokio::test]
    async fn should_capture_a_parked_url_once_its_page_shows_its_previous_identity_again() {
        let mut candidate = candidate();
        candidate.identity_conflict_streak = 2;
        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().once().returning(|_| {
            let scraped = titled(scraped(Some("SKU-1"), "schemas", URL), "Table");
            Box::pin(async { scrape_outcome(Ok(Some(scraped))) })
        });
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates.expect_mark_fetch_failure().never();
        let ctx = scrape_candidate_context(scraper_candidates, scraper_service);

        let outcome = scrape_candidate(candidate, &ctx).await;

        match &outcome.capture.expect("the page is captured").on_success {
            RawCaptureSuccessAction::MarkScraped(meta) => {
                assert_eq!(meta.source_listing_id, "SKU-1");
                assert!(meta.page_identity_change.is_none());
            }
            _ => panic!("a product page is marked scraped after capture"),
        }
    }
}
