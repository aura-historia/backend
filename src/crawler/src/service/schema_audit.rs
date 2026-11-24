//! The live schema audit pass: it re-checks the live product schemas of crawl-enabled
//! ListingSources on a sample of their captured product pages and opens a product-schema review
//! for the review agents where they fail ([`audit_live_schemas`]). It runs in `agent` authoring
//! mode only, where the agents repair such a review; the contract is in
//! `docs/crawler/extraction-and-review.md` (live schema audit).

use crate::CrawlerReviewId;
use crate::review::captured_identity_sample::SampledCapturedPage;
use crate::review::model::{PAGE_ROLE_SEED, SchemaReviewPageInput};
use crate::review::repository::{
    CrawlerReviewRepository, ReviewRepositoryError, SchemaAuditSource,
};
use crate::review::schema_audit::audit_live_schemas;
use crate::scraper::scraper_service::HtmlFetcher;
use crate::service::cron::CronStop;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// How long a ListingSource's audit stands before its live schemas are audited again.
pub const SCHEMA_AUDIT_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How many ListingSources one pass audits. Each costs a few page fetches at the shop's pacing.
pub const SCHEMA_AUDIT_SOURCES_PER_PASS: i64 = 3;

/// How many audit reviews may wait for the review agents at once. A pass opens none beyond it,
/// so a first audit of many old schemas does not bury the agents' queue and budget; the sources
/// it leaves are audited in later passes.
pub const MAX_PENDING_SCHEMA_AUDIT_REVIEWS: i64 = 10;

/// What auditing one ListingSource came to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AuditOutcome {
    /// The live schemas passed the audit.
    Passed,
    /// The audit found only what an approval of these live schemas already accepted.
    Accepted,
    /// The audit opened, or joined, the review that names the findings.
    Opened(CrawlerReviewId),
    /// A stop was requested before every sampled page was fetched; nothing was recorded.
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
    /// while fewer than [`MAX_PENDING_SCHEMA_AUDIT_REVIEWS`] audit reviews are pending. A failure
    /// of one source is logged and leaves the others to the pass.
    pub async fn run_once(&self, stop: &CronStop) {
        let pending = match self.repository.pending_schema_audit_reviews().await {
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
                Ok(_) => {}
                Err(error) => warn!(
                    event = "crawler.schema_audit.failed",
                    listing_source_id = %source.listing_source_id,
                    error = %error,
                    "Schema audit of a ListingSource failed"
                ),
            }
        }
    }

    /// Audits the live schemas of `source` on its sampled captured product pages, records the
    /// audit and opens the audit review when it finds what no approval of these schemas accepted.
    async fn audit(
        &self,
        source: &SchemaAuditSource,
        stop: &CronStop,
    ) -> Result<AuditOutcome, ReviewRepositoryError> {
        let listing_source_id = &source.listing_source_id;
        let (live_schemas, urls) = self
            .repository
            .schema_audit_sample(listing_source_id)
            .await?;
        let mut sampled = Vec::with_capacity(urls.len());
        for page in urls {
            if stop.is_requested() {
                return Ok(AuditOutcome::Interrupted);
            }
            let html = self
                .fetcher
                .fetch(&page.url)
                .await
                .ok()
                .map(|fetched| fetched.html);
            sampled.push(SampledCapturedPage { page, html });
        }
        let findings = audit_live_schemas(&source.listing_source_name, &live_schemas, &sampled);
        let accepted = if findings.is_empty() {
            None
        } else {
            self.repository
                .accepted_schema_audit_findings(listing_source_id, &live_schemas)
                .await?
        };
        self.repository
            .record_schema_audit(listing_source_id)
            .await?;
        let fetched_pages = sampled.iter().filter(|page| page.html.is_some()).count();
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
        if accepted.is_some_and(|accepted| findings.accepted_by(&accepted)) {
            info!(
                event = "crawler.schema_audit.accepted",
                listing_source_id = %listing_source_id,
                findings = ?codes,
                "Schema audit found only what an approval of the live schemas accepted"
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
        let review_id = self
            .repository
            .open_schema_audit_review(listing_source_id, &live_schemas, pages, &codes)
            .await?;
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
}
