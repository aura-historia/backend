//! The live schema audit pass: it re-checks the live product schemas of crawl-enabled
//! ListingSources on a sample of their captured product pages and opens a product-schema review
//! for the review agents where they fail ([`audit_live_schemas`]). It runs in `agent` authoring
//! mode only, where the agents repair such a review; the contract is in
//! `docs/crawler/extraction-and-review.md` (live schema audit).

use crate::CrawlerReviewId;
use crate::review::captured_identity_sample::SampledCapturedPage;
use crate::review::model::{PAGE_ROLE_SEED, SchemaReviewPageInput};
use crate::review::repository::{
    CrawlerReviewRepository, ReviewRepositoryError, SchemaAuditReview, SchemaAuditSource,
};
use crate::review::schema_audit::audit_live_schemas;
use crate::scraper::scraper_service::HtmlFetcher;
use crate::service::cron::CronStop;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};
use url::Url;

/// How long a ListingSource's audit stands before its live schemas are audited again.
pub const SCHEMA_AUDIT_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How many ListingSources one pass audits. Each costs a few page fetches at the shop's pacing.
pub const SCHEMA_AUDIT_SOURCES_PER_PASS: i64 = 3;

/// How many audit reviews opened within [`SCHEMA_AUDIT_AGE`] may wait at once. A pass opens none
/// beyond it, so a first audit of many old schemas does not bury the agents' queue and budget; the
/// sources it leaves are audited in later passes. An older review waits for a human and no longer
/// counts.
pub const MAX_PENDING_SCHEMA_AUDIT_REVIEWS: i64 = 10;

/// What auditing one ListingSource came to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AuditOutcome {
    /// The live schemas passed the audit.
    Passed,
    /// The audit found only what a review of these live schemas already accepted.
    Accepted,
    /// The audit opened the review that names the findings.
    Opened(CrawlerReviewId),
    /// A product-schema review the scraper opened meanwhile holds the source; it is audited again
    /// once that review is decided.
    Deferred,
    /// No sampled page could be fetched as a product page, so the audit saw nothing.
    Unfetched,
    /// A stop was requested before every sampled page was fetched.
    Interrupted,
}

pub struct SchemaAudit {
    repository: CrawlerReviewRepository,
    fetcher: Arc<dyn HtmlFetcher>,
}

impl SchemaAudit {
    /// Audits through `repository`, fetching pages with the crawler's shared `fetcher`, so the
    /// audit's requests count against the same robots.txt cache and per-domain pacing as the
    /// scraper's.
    pub fn new(repository: CrawlerReviewRepository, fetcher: Arc<dyn HtmlFetcher>) -> Self {
        Self {
            repository,
            fetcher,
        }
    }

    /// Audits up to [`SCHEMA_AUDIT_SOURCES_PER_PASS`] due ListingSources one after the other,
    /// while fewer than [`MAX_PENDING_SCHEMA_AUDIT_REVIEWS`] recent audit reviews are pending. A
    /// failure of one source is logged and leaves the others to the pass.
    pub async fn run_once(&self, stop: &CronStop) {
        let pending = match self
            .repository
            .pending_schema_audit_reviews(SCHEMA_AUDIT_AGE)
            .await
        {
            Ok(pending) => pending,
            Err(error) => {
                warn!(event = "crawler.schema_audit.failed", error = %error, "Schema audit pass could not count pending audit reviews");
                return;
            }
        };
        let mut open_slots = MAX_PENDING_SCHEMA_AUDIT_REVIEWS - pending;
        if open_slots <= 0 {
            info!(
                event = "crawler.schema_audit.paused",
                pending,
                "Schema audit pass skipped: enough audit reviews wait for the review agents"
            );
            return;
        }
        let sources = match self
            .repository
            .schema_audit_due_sources(SCHEMA_AUDIT_AGE, SCHEMA_AUDIT_SOURCES_PER_PASS)
            .await
        {
            Ok(sources) => sources,
            Err(error) => {
                warn!(event = "crawler.schema_audit.failed", error = %error, "Schema audit pass could not read the due ListingSources");
                return;
            }
        };
        for source in sources {
            if stop.is_requested() {
                break;
            }
            match self.audit(&source, stop).await {
                Ok(AuditOutcome::Opened(_)) => {
                    open_slots -= 1;
                    if open_slots == 0 {
                        break;
                    }
                }
                Ok(AuditOutcome::Interrupted) => break,
                Ok(_) => {}
                Err(error) => warn!(
                    event = "crawler.schema_audit.failed",
                    listing_source_id = %source.listing_source_id,
                    error = %error,
                    "Schema audit of a ListingSource failed; it is audited again with its next audit"
                ),
            }
        }
    }

    /// Audits the live schemas of `source` on its sampled captured product pages and opens the
    /// audit review when it finds what no review of these schemas accepted. The audit is recorded
    /// first, so a source whose audit fails waits for its next audit instead of taking a place in
    /// every pass.
    async fn audit(
        &self,
        source: &SchemaAuditSource,
        stop: &CronStop,
    ) -> Result<AuditOutcome, ReviewRepositoryError> {
        let listing_source_id = &source.listing_source_id;
        self.repository
            .record_schema_audit(listing_source_id)
            .await?;
        let (live_schemas, urls) = self
            .repository
            .schema_audit_sample(listing_source_id)
            .await?;
        let mut sampled = Vec::with_capacity(urls.len());
        for page in urls {
            let Some(html) = self.fetch_product_page(&page.url, stop).await else {
                return Ok(AuditOutcome::Interrupted);
            };
            sampled.push(SampledCapturedPage { page, html });
        }
        let fetched_pages = sampled.iter().filter(|page| page.html.is_some()).count();
        if fetched_pages == 0 {
            info!(
                event = "crawler.schema_audit.unfetched",
                listing_source_id = %listing_source_id,
                sampled_pages = sampled.len(),
                "Schema audit fetched none of the sampled captured product pages"
            );
            return Ok(AuditOutcome::Unfetched);
        }
        let findings = audit_live_schemas(&source.listing_source_name, &live_schemas, &sampled);
        if findings.is_empty() {
            info!(
                event = "crawler.schema_audit.passed",
                listing_source_id = %listing_source_id,
                sampled_pages = sampled.len(),
                fetched_pages,
                "Live schemas passed the schema audit"
            );
            return Ok(AuditOutcome::Passed);
        }
        let codes = findings.codes();
        let accepted = self
            .repository
            .accepted_schema_audit_findings(listing_source_id, &live_schemas)
            .await?;
        if findings.accepted_by(&accepted) {
            info!(
                event = "crawler.schema_audit.accepted",
                listing_source_id = %listing_source_id,
                findings = ?codes,
                "Schema audit found only what a review of the live schemas accepted"
            );
            return Ok(AuditOutcome::Accepted);
        }
        let pages = sampled
            .into_iter()
            .filter_map(|sampled| {
                Some(SchemaReviewPageInput {
                    url: sampled.page.url.to_string(),
                    role: PAGE_ROLE_SEED.to_owned(),
                    raw_html: sampled.html?,
                })
            })
            .collect();
        let review = self
            .repository
            .open_schema_audit_review(listing_source_id, &live_schemas, pages, &codes)
            .await?;
        let SchemaAuditReview::Opened(review_id) = review else {
            info!(
                event = "crawler.schema_audit.deferred",
                listing_source_id = %listing_source_id,
                findings = ?codes,
                "Schema audit opened no review: a schema review of the source is pending"
            );
            return Ok(AuditOutcome::Deferred);
        };
        info!(
            event = "crawler.schema_audit.opened",
            listing_source_id = %listing_source_id,
            review_id = %review_id,
            findings = ?codes,
            fetched_pages,
            "Schema audit opened a review of the live schemas; the source's new listings wait for it"
        );
        Ok(AuditOutcome::Opened(review_id))
    }

    /// The HTML of the captured product page at `url`: `Some(None)` when it could not be fetched
    /// or redirected elsewhere, such as a sold item the shop now sends to a category page, which
    /// is no product page to audit the schemas on; `None` when a stop is requested first, which
    /// also ends a running fetch.
    async fn fetch_product_page(&self, url: &Url, stop: &CronStop) -> Option<Option<String>> {
        if stop.is_requested() {
            return None;
        }
        let mut stopped = stop.subscribe();
        tokio::select! {
            fetched = self.fetcher.fetch(url) => Some(
                fetched
                    .ok()
                    .filter(|fetched| fetched.final_url == *url)
                    .map(|fetched| fetched.html),
            ),
            _ = stopped.wait_for(|requested| *requested) => None,
        }
    }
}
