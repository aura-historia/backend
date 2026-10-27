use super::job::CrawlerCronJob;
use super::metrics::emit_pass_duration;
use super::stop::CronStop;
use crate::CrawlerReviewId;
use crate::metrics::{CrawlerMetric, MetricDimension, MetricRecord};
use crate::network::policy::{
    DomainFailureKind, NetworkErrorKind, domain_cooldown, durable_retry_cooldown_for,
};
use crate::scraper::candidate_service::{
    DomainCircuitOpenRequest, DomainHealthSnapshot, FetchFailureRequest, RedirectUrlMove,
    SchemaReviewWait, SchemaReviewWaitRequest, ScrapeCompletion, ScraperCandidate,
    ScraperCandidateService, next_domain_failure_streak,
};
use crate::scraper::css_selector::product_schema_service::ProductListingSchemaServiceError;
use crate::scraper::domain_circuit::{DomainBatchEvidence, DomainCircuitDecision};
use crate::scraper::raw_input::{crawler_provenance, crawler_verified_removal_input};
use crate::scraper::scraper_service::domain::product::{
    current_fetch_failure, domain_health_for_scraper_error, with_scrape_observation,
};
use crate::scraper::scraper_service::{
    DomainFetchHealth, DomainFetchStats, FetchFailureContext, FetchFailureSource, ScrapeMode,
    ScrapeOutcome, ScrapeRequest, ScrapedProduct, ScraperError, ScraperService,
};
use crate::service::raw_capture::{
    ProductListingRawCaptureItem, ProductListingRawCaptureOutcome, ProductListingRawCaptureService,
};
use crate::spider::classification::url_metadata::{CrawlerDisposition, CrawlerUrlWriteOutcome};
use crate::spider::local_lock::{
    DomainLock, DomainLockScope, ListingSourceLock, LocalLockManager, UrlLock,
};
use listing_source_core::ListingSourceId;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tracing::{Instrument, debug, error, info, warn};

/// Retry cooldown applied to URLs whose scrape failed on schema generation, review,
/// identity-conflict or LLM-budget outcomes rather than on transport, so the next pass
/// neither re-spends LLM budget on the same ListingSource nor refetches a page that waits
/// for the reviewer.
const SCHEMA_ERROR_RETRY_COOLDOWN: Duration = Duration::from_secs(30 * 60);
/// Upper bound of the retry delay of a page that keeps waiting on the same pending
/// product-schema review. Approval and rejection release waiting pages at once; the bound
/// keeps a page whose review stays pending, for example awaiting repair, from going unchecked
/// for much longer than a reviewer's working session.
const SCHEMA_REVIEW_WAIT_MAX_RETRY_DELAY: Duration = Duration::from_secs(4 * 60 * 60);
/// `last_error_kind` of a URL whose schema derived another identity than the captured one.
const SOURCE_LISTING_ID_CONFLICT_ERROR_KIND: &str = "SourceListingIdConflict";

/// Every scrape completion records its URL `ACTIVE`, sold pages included.
/// `DORMANT_SOLD` is scheduled exactly like `ACTIVE`, so the scraper no longer writes
/// it; rows that hold it stay valid.
const SCRAPED_URL_DISPOSITION: CrawlerDisposition = CrawlerDisposition::Active;

/// Context for scraping a domain's candidates.
struct ScrapeDomainContext {
    scraper: Arc<dyn ScraperService>,
    scraper_candidates: Arc<dyn ScraperCandidateService>,
    lock_manager: Arc<LocalLockManager>,
    command_tx: mpsc::Sender<QueuedRawCapture>,
    budget_exhausted_listing_sources: Arc<Mutex<HashSet<ListingSourceId>>>,
    domain_id: crate::CrawlerDomainId,
    batch_time_budget: Duration,
    stop: CronStop,
}

/// Metadata applied only after durable raw capture succeeds.
struct CandidateMeta {
    listing_source_id: listing_source_core::ListingSourceId,
    original_url: url::Url,
    effective_url: url::Url,
    hash: String,
    schema_fingerprint: String,
    raw_input_sha256: Vec<u8>,
    source_listing_id: String,
    expected_last_captured_raw_input_sha256: Option<Vec<u8>>,
}

struct RawCaptureRequest {
    item: ProductListingRawCaptureItem,
    on_success: RawCaptureSuccessAction,
    on_failure: RawCaptureFailureAction,
}

enum RawCaptureSuccessAction {
    MarkScraped(CandidateMeta),
    MarkRemoved {
        listing_source_id: ListingSourceId,
        url: url::Url,
        raw_input_sha256: Vec<u8>,
        expected_last_captured_raw_input_sha256: Option<Vec<u8>>,
    },
}

enum RawCaptureFailureAction {
    None,
    MarkScraperFailure {
        listing_source_id: ListingSourceId,
        url: url::Url,
        expected_last_captured_raw_input_sha256: Option<Vec<u8>>,
    },
}

impl From<(ProductListingRawCaptureItem, CandidateMeta)> for RawCaptureRequest {
    fn from((item, meta): (ProductListingRawCaptureItem, CandidateMeta)) -> Self {
        Self {
            item,
            on_success: RawCaptureSuccessAction::MarkScraped(meta),
            on_failure: RawCaptureFailureAction::None,
        }
    }
}

struct QueuedRawCapture {
    request: RawCaptureRequest,
    /// The configured domain of the scraped URL, the capture's metric dimension.
    listing_source_domain: String,
    enqueued_at: tokio::time::Instant,
}

#[derive(Default)]
struct ScrapeCandidateOutcome {
    capture: Option<RawCaptureRequest>,
    errored: bool,
    skipped: bool,
    /// The URL lost a process-local URL or ListingSource lock race and stays
    /// eligible for a later pass without any durable state change.
    deferred: bool,
    domain_health: DomainFetchHealth,
    /// The HTTP status the candidate's fetch failed with, when it failed on one.
    http_failure_status: Option<u16>,
    /// The failure opened the domain circuit: the rest of the batch stops.
    circuit_opened: bool,
    /// The domain snapshot an applied domain write left behind, which the
    /// batch carries forward as the fence for its later writes.
    persisted_domain_health: Option<DomainHealthSnapshot>,
    /// robots.txt of the URL's origin could not be reached, which disallows
    /// every URL there until it is fetched again.
    robots_unreachable: bool,
    /// The error kind the failure recorded on its URL.
    failure_kind: Option<String>,
}

struct ScrapeDomainOutcome {
    succeeded: usize,
    failed: usize,
    skipped: usize,
    /// URLs deferred by lock contention: every candidate of a deferred domain batch plus
    /// URLs that lost a URL or ListingSource lock race inside a running batch. Counted in
    /// the same unit as `succeeded`, `failed` and `skipped`.
    deferred_urls: usize,
    /// The whole domain batch was deferred by scraper domain lock contention.
    deferred: bool,
}

/// The metric dimensions and per-batch observations of one scraper domain batch. Every
/// candidate of a batch belongs to the batch's domain and so to one ListingSource.
struct DomainBatchMetrics {
    listing_source_id: ListingSourceId,
    listing_source_domain: String,
    failure_kinds: BTreeMap<String, u64>,
    circuit_opened: bool,
}

impl DomainBatchMetrics {
    fn of(candidate: &ScraperCandidate) -> Self {
        Self {
            listing_source_id: candidate.listing_source_id,
            listing_source_domain: candidate.listing_source_domain.clone(),
            failure_kinds: BTreeMap::new(),
            circuit_opened: false,
        }
    }

    fn observe(&mut self, candidate_outcome: &ScrapeCandidateOutcome) {
        if let Some(kind) = &candidate_outcome.failure_kind {
            *self.failure_kinds.entry(kind.clone()).or_default() += 1;
        }
        // Only an applied write opened the circuit; a stale fence left it as it was.
        self.circuit_opened |=
            candidate_outcome.circuit_opened && candidate_outcome.persisted_domain_health.is_some();
    }

    fn emit(self, outcome: &ScrapeDomainOutcome) {
        let record = || {
            MetricRecord::for_listing_source_domain(
                &self.listing_source_id,
                &self.listing_source_domain,
            )
        };
        record()
            .with_value(CrawlerMetric::ScrapesSucceeded, outcome.succeeded as u64)
            .with_value(CrawlerMetric::ScrapesFailed, outcome.failed as u64)
            .with_value(
                CrawlerMetric::DomainCircuitOpened,
                u64::from(self.circuit_opened),
            )
            .emit();
        for (kind, count) in &self.failure_kinds {
            record()
                .with_dimension(MetricDimension::FailureKind, kind.as_str())
                .with_value(CrawlerMetric::UrlFailures, *count)
                .emit();
        }
    }
}

struct ScheduledScrapeDomainOutcome {
    domain_id: crate::CrawlerDomainId,
    outcome: ScrapeDomainOutcome,
}

/// Captures a batch of raw observations, then updates crawler-local metadata only for durable
/// capture outcomes. Worker normalization owns canonical ProductListing mutation.
#[tracing::instrument(
    name = "crawler_flush_raw_capture_batch",
    skip(raw_capture, scraper_candidates, batch),
    fields(batch_size = batch.len())
)]
async fn flush_batch(
    raw_capture: &Arc<dyn ProductListingRawCaptureService>,
    scraper_candidates: &Arc<dyn ScraperCandidateService>,
    batch: Vec<QueuedRawCapture>,
    queue_depth: usize,
) {
    let batch_size = batch.len();
    let oldest_item_age_ms = batch
        .iter()
        .map(|queued| queued.enqueued_at.elapsed().as_millis())
        .max()
        .unwrap_or(0);
    let observations = batch
        .iter()
        .map(|queued| queued.request.item.clone())
        .collect();

    let capture_started_at = tokio::time::Instant::now();
    let mut outcomes = raw_capture.capture(observations).await;
    let capture_latency_ms = capture_started_at.elapsed().as_millis();
    let expected = batch.len();
    let actual = outcomes.len();

    if actual != expected {
        warn!(
            expected,
            actual,
            "Raw ProductListing capture returned an incomplete result; unmatched URLs will be retried"
        );
    }

    outcomes.truncate(expected);
    if outcomes.len() < expected {
        outcomes.resize(expected, ProductListingRawCaptureOutcome::RetryableFailure);
    }
    emit_products_captured(&batch, &outcomes);

    let persisted_count = outcomes
        .iter()
        .filter(|outcome| **outcome == ProductListingRawCaptureOutcome::Persisted)
        .count();
    let discarded_missing_source_count = outcomes
        .iter()
        .filter(|outcome| **outcome == ProductListingRawCaptureOutcome::DiscardedMissingSource)
        .count();
    let persistence_failure_count = outcomes
        .iter()
        .filter(|outcome| **outcome == ProductListingRawCaptureOutcome::RetryableFailure)
        .count();
    let mut mark_as_scraped_count = 0;
    let mut stale_completion_count = 0;
    let mut mark_as_scraped_failure_count = 0;

    for (queued, outcome) in batch.into_iter().zip(outcomes) {
        match (
            outcome,
            queued.request.on_success,
            queued.request.on_failure,
        ) {
            (
                ProductListingRawCaptureOutcome::Persisted,
                RawCaptureSuccessAction::MarkScraped(meta),
                _,
            ) => {
                let write_result = if meta.original_url == meta.effective_url {
                    scraper_candidates
                        .mark_as_scraped(ScrapeCompletion {
                            listing_source_id: meta.listing_source_id,
                            url: meta.original_url.clone(),
                            hash: meta.hash.clone(),
                            schema_fingerprint: meta.schema_fingerprint.clone(),
                            raw_input_sha256: meta.raw_input_sha256.clone(),
                            source_listing_id: meta.source_listing_id.clone(),
                            disposition: SCRAPED_URL_DISPOSITION,
                            expected_last_captured_raw_input_sha256: meta
                                .expected_last_captured_raw_input_sha256
                                .clone(),
                        })
                        .await
                } else {
                    scraper_candidates
                        .move_redirected_url(RedirectUrlMove {
                            listing_source_id: meta.listing_source_id,
                            original_url: meta.original_url.clone(),
                            effective_url: meta.effective_url.clone(),
                            hash: meta.hash.clone(),
                            schema_fingerprint: meta.schema_fingerprint.clone(),
                            raw_input_sha256: meta.raw_input_sha256.clone(),
                            source_listing_id: meta.source_listing_id.clone(),
                            disposition: SCRAPED_URL_DISPOSITION,
                            expected_last_captured_raw_input_sha256: meta
                                .expected_last_captured_raw_input_sha256
                                .clone(),
                        })
                        .await
                };
                match write_result {
                    Ok(CrawlerUrlWriteOutcome::Applied) => {
                        mark_as_scraped_count += 1;
                    }
                    Ok(CrawlerUrlWriteOutcome::NoopStale) => {
                        stale_completion_count += 1;
                        debug!(
                            listing_source_id = %meta.listing_source_id,
                            url = %meta.original_url,
                            crawler_url_write_outcome = "stale_noop",
                            "Skipped stale crawler scrape completion after durable raw capture"
                        );
                    }
                    Err(error) => {
                        mark_as_scraped_failure_count += 1;
                        warn!(listing_source_id = %meta.listing_source_id, error = %error, url = %meta.original_url, "Failed to mark product as scraped after raw capture");
                    }
                }
            }
            (
                ProductListingRawCaptureOutcome::Persisted,
                RawCaptureSuccessAction::MarkRemoved {
                    listing_source_id,
                    url,
                    raw_input_sha256,
                    expected_last_captured_raw_input_sha256,
                },
                _,
            ) => match scraper_candidates
                .mark_removed(
                    &listing_source_id,
                    &url,
                    &raw_input_sha256,
                    expected_last_captured_raw_input_sha256.as_deref(),
                )
                .await
            {
                Ok(CrawlerUrlWriteOutcome::Applied) => {
                    mark_as_scraped_count += 1;
                }
                Ok(CrawlerUrlWriteOutcome::NoopStale) => {
                    stale_completion_count += 1;
                    debug!(
                        listing_source_id = %listing_source_id,
                        url = %url,
                        crawler_url_write_outcome = "stale_noop",
                        "Skipped stale crawler removal completion after durable raw capture"
                    );
                }
                Err(error) => {
                    mark_as_scraped_failure_count += 1;
                    warn!(error = %error, listing_source_id = %listing_source_id, url = %url, "Raw removal capture committed but crawler scrape metadata update failed");
                }
            },
            (
                ProductListingRawCaptureOutcome::RetryableFailure,
                _,
                RawCaptureFailureAction::MarkScraperFailure {
                    listing_source_id,
                    url,
                    expected_last_captured_raw_input_sha256,
                },
            ) => match scraper_candidates
                .mark_scraper_failure(
                    &listing_source_id,
                    &url,
                    "RawCaptureFailed",
                    "verified removal raw capture did not commit",
                    expected_last_captured_raw_input_sha256.as_deref(),
                )
                .await
            {
                Ok(CrawlerUrlWriteOutcome::Applied) => {}
                Ok(CrawlerUrlWriteOutcome::NoopStale) => {
                    debug!(
                        listing_source_id = %listing_source_id,
                        url = %url,
                        crawler_url_write_outcome = "stale_noop",
                        "Skipped stale crawler raw-capture failure metadata"
                    );
                }
                Err(error) => {
                    warn!(
                        error = %error,
                        listing_source_id = %listing_source_id,
                        url = %url,
                        "Failed to persist raw-capture failure metadata"
                    );
                }
            },
            (
                ProductListingRawCaptureOutcome::RetryableFailure,
                _,
                RawCaptureFailureAction::None,
            ) => {}
            (ProductListingRawCaptureOutcome::DiscardedMissingSource, _, _) => {
                debug!(
                    outcome = "discarded_missing_source",
                    "Discarded crawler raw-capture completion without marking it persisted"
                );
            }
        }
    }

    info!(
        event = "crawler.raw_capture.batch",
        batch_size,
        queue_depth,
        oldest_item_age_ms,
        capture_latency_ms,
        persisted_count,
        discarded_missing_source_count,
        persistence_failure_count,
        mark_as_scraped_count,
        stale_completion_count,
        mark_as_scraped_failure_count,
        "Crawler raw capture batch complete"
    );
}

/// Records, per ListingSource domain of the batch, how many of its observations raw
/// capture persisted durably; a domain whose captures all failed records zero.
fn emit_products_captured(
    batch: &[QueuedRawCapture],
    outcomes: &[ProductListingRawCaptureOutcome],
) {
    let mut captured: HashMap<(ListingSourceId, &str), u64> = HashMap::new();
    for (queued, outcome) in batch.iter().zip(outcomes) {
        let source = (
            queued.request.item.command.listing_source_id,
            queued.listing_source_domain.as_str(),
        );
        *captured.entry(source).or_default() +=
            u64::from(*outcome == ProductListingRawCaptureOutcome::Persisted);
    }
    for ((listing_source_id, domain), count) in captured {
        MetricRecord::for_listing_source_domain(&listing_source_id, domain)
            .with_value(CrawlerMetric::ProductsCaptured, count)
            .emit();
    }
}

#[allow(clippy::result_large_err)]
async fn enqueue_raw_capture(
    command_tx: &mpsc::Sender<QueuedRawCapture>,
    listing_source_domain: &str,
    request: impl Into<RawCaptureRequest>,
) -> Result<Duration, mpsc::error::SendError<QueuedRawCapture>> {
    let queued = QueuedRawCapture {
        request: request.into(),
        listing_source_domain: listing_source_domain.to_owned(),
        enqueued_at: tokio::time::Instant::now(),
    };
    let wait_started_at = tokio::time::Instant::now();

    command_tx.send(queued).await?;

    Ok(wait_started_at.elapsed())
}

#[tracing::instrument(
    name = "crawler_scrape_candidate",
    skip(candidate),
    fields(
        listing_source_id = %candidate.listing_source_id,
        url = %candidate.url
    )
)]
fn handle_verified_removal(candidate: &ScraperCandidate) -> ScrapeCandidateOutcome {
    let input = match crawler_verified_removal_input(&candidate.url) {
        Ok(input) => input,
        Err(error) => {
            warn!(error = %error, listing_source_id = %candidate.listing_source_id, "Failed to build verified-removal raw input");
            return ScrapeCandidateOutcome {
                capture: None,
                errored: true,
                skipped: false,
                ..Default::default()
            };
        }
    };
    let raw_input_sha256 = match input.hash() {
        Ok(hash) => hash.as_bytes().to_vec(),
        Err(error) => {
            warn!(error = %error, listing_source_id = %candidate.listing_source_id, "Failed to hash verified-removal raw input");
            return ScrapeCandidateOutcome {
                capture: None,
                errored: true,
                skipped: false,
                ..Default::default()
            };
        }
    };
    let provenance = match crawler_provenance(None, None) {
        Ok(provenance) => provenance,
        Err(error) => {
            warn!(error = %error, listing_source_id = %candidate.listing_source_id, "Failed to build verified-removal provenance");
            return ScrapeCandidateOutcome {
                capture: None,
                errored: true,
                skipped: false,
                ..Default::default()
            };
        }
    };

    ScrapeCandidateOutcome {
        capture: Some(RawCaptureRequest {
            item: ProductListingRawCaptureItem::crawler(
                candidate.listing_source_id,
                &candidate.source_record_key,
                input,
                provenance,
            ),
            on_success: RawCaptureSuccessAction::MarkRemoved {
                listing_source_id: candidate.listing_source_id,
                url: candidate.url.clone(),
                raw_input_sha256,
                expected_last_captured_raw_input_sha256: candidate
                    .last_captured_raw_input_sha256
                    .clone(),
            },
            on_failure: RawCaptureFailureAction::MarkScraperFailure {
                listing_source_id: candidate.listing_source_id,
                url: candidate.url.clone(),
                expected_last_captured_raw_input_sha256: candidate
                    .last_captured_raw_input_sha256
                    .clone(),
            },
        }),
        errored: false,
        skipped: false,
        ..Default::default()
    }
}

#[cfg(test)]
async fn scrape_candidate(
    candidate: ScraperCandidate,
    ctx: &ScrapeDomainContext,
) -> ScrapeCandidateOutcome {
    scrape_candidate_in_batch(candidate, ctx, &DomainBatchEvidence::default()).await
}

async fn scrape_candidate_in_batch(
    candidate: ScraperCandidate,
    ctx: &ScrapeDomainContext,
    evidence: &DomainBatchEvidence,
) -> ScrapeCandidateOutcome {
    if should_skip_candidate(&candidate, ctx).await {
        return skipped_outcome();
    }

    let Some(_lock) = UrlLock::try_acquire(&ctx.lock_manager, &candidate.url) else {
        debug!("Deferring URL - URL lock is currently held");
        return deferred_outcome();
    };
    let Some(_listing_source_lock) =
        ListingSourceLock::try_acquire(&ctx.lock_manager, candidate.listing_source_id)
    else {
        debug!("Deferring URL because another worker is scraping this ListingSource");
        return deferred_outcome();
    };

    let scrape_outcome = run_candidate_scrape(&candidate, ctx).await;
    let domain_health = scrape_outcome.domain_health.clone();
    let fetch_failure = scrape_outcome.fetch_failure;
    let mut outcome = match scrape_outcome.result {
        Ok(Some(scraped)) => handle_successful_scrape(&candidate, ctx, scraped).await,
        Ok(None) => skipped_outcome(),
        Err(ScraperError::ProductListingRemoved { .. }) => handle_verified_removal(&candidate),
        Err(error) => {
            handle_scrape_error(
                &candidate,
                ctx,
                error,
                fetch_failure,
                domain_health.clone(),
                evidence,
            )
            .await
        }
    };
    if matches!(outcome.domain_health, DomainFetchHealth::NotObserved) {
        outcome.domain_health = domain_health;
    }
    outcome
}

fn skipped_outcome() -> ScrapeCandidateOutcome {
    ScrapeCandidateOutcome {
        skipped: true,
        ..Default::default()
    }
}

fn deferred_outcome() -> ScrapeCandidateOutcome {
    ScrapeCandidateOutcome {
        deferred: true,
        ..Default::default()
    }
}

async fn should_skip_candidate(candidate: &ScraperCandidate, ctx: &ScrapeDomainContext) -> bool {
    let exhausted = ctx.budget_exhausted_listing_sources.lock().await;
    if exhausted.contains(&candidate.listing_source_id) {
        debug!("Skipping URL — ListingSource LLM budget already exhausted in this batch");
        return true;
    }
    false
}

fn scrape_request(candidate: &ScraperCandidate, mode: ScrapeMode) -> ScrapeRequest {
    ScrapeRequest {
        domain_id: Some(candidate.domain_id),
        listing_source_id: candidate.listing_source_id,
        url: candidate.url.clone(),
        source_record_key: candidate.source_record_key.clone(),
        last_source_listing_id: candidate.last_source_listing_id.clone(),
        product_url_pattern: candidate.url_pattern.clone(),
        last_scraped_hash: candidate.last_scraped_hash.clone(),
        last_scraped_schema_fingerprint: candidate.last_scraped_schema_fingerprint.clone(),
        expected_last_captured_raw_input_sha256: candidate.last_captured_raw_input_sha256.clone(),
        fallback_currency: candidate.fallback_currency,
        mode,
    }
}

async fn run_candidate_scrape(
    candidate: &ScraperCandidate,
    ctx: &ScrapeDomainContext,
) -> ScrapeOutcome {
    let mode = if candidate.is_domain_probe {
        ScrapeMode::DomainProbe
    } else {
        ScrapeMode::Normal
    };
    let (mut outcome, observed_domain_health) =
        with_scrape_observation(ctx.scraper.scrape(scrape_request(candidate, mode))).await;
    if outcome.fetch_failure.is_none() {
        outcome.fetch_failure = current_fetch_failure();
    }
    if mode != ScrapeMode::DomainProbe
        && matches!(outcome.domain_health, DomainFetchHealth::NotObserved)
    {
        outcome.domain_health = observed_domain_health.unwrap_or_else(|| match &outcome.result {
            Ok(_) => DomainFetchHealth::Responsive,
            Err(error) => domain_health_for_scraper_error(error),
        });
    }
    outcome
}

async fn handle_successful_scrape(
    candidate: &ScraperCandidate,
    ctx: &ScrapeDomainContext,
    scraped: ScrapedProduct,
) -> ScrapeCandidateOutcome {
    let url_moved = candidate.url != scraped.effective_url;
    if let Some(captured_identity) = candidate
        .last_source_listing_id
        .as_deref()
        .filter(|identity| *identity != scraped.source_listing_id)
    {
        return reject_identity_conflict(candidate, ctx, captured_identity, &scraped).await;
    }
    if url_moved {
        match ctx
            .scraper_candidates
            .redirect_destination_is_available(&scraped.effective_url)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                warn!(
                    event = "crawler.product_url_move_rejected_destination_known",
                    listing_source_id = %candidate.listing_source_id,
                    old_url = %candidate.url,
                    new_url = %scraped.effective_url,
                    "Deferring redirect destination reconciliation to issue #1899"
                );
                return ScrapeCandidateOutcome {
                    skipped: true,
                    ..Default::default()
                };
            }
            Err(error) => {
                warn!(error = %error, "Failed to check redirect destination before raw capture");
                return ScrapeCandidateOutcome {
                    errored: true,
                    ..Default::default()
                };
            }
        }
    }
    let provenance = match crawler_provenance(
        Some(scraped.hash.as_str()),
        Some(scraped.schema_fingerprint.as_str()),
    ) {
        Ok(provenance) => provenance,
        Err(error) => {
            warn!(error = %error, listing_source_id = %candidate.listing_source_id, "Failed to build crawler raw-capture provenance");
            return ScrapeCandidateOutcome {
                errored: true,
                ..Default::default()
            };
        }
    };
    let meta = CandidateMeta {
        listing_source_id: candidate.listing_source_id,
        original_url: candidate.url.clone(),
        effective_url: scraped.effective_url.clone(),
        hash: scraped.hash,
        schema_fingerprint: scraped.schema_fingerprint,
        raw_input_sha256: scraped.raw_input_sha256,
        source_listing_id: scraped.source_listing_id,
        expected_last_captured_raw_input_sha256: candidate.last_captured_raw_input_sha256.clone(),
    };

    if !url_moved
        && candidate.last_captured_raw_input_sha256.as_deref()
            == Some(meta.raw_input_sha256.as_slice())
    {
        return match ctx
            .scraper_candidates
            .mark_as_scraped(ScrapeCompletion {
                listing_source_id: meta.listing_source_id,
                url: meta.original_url.clone(),
                hash: meta.hash.clone(),
                schema_fingerprint: meta.schema_fingerprint.clone(),
                raw_input_sha256: meta.raw_input_sha256.clone(),
                source_listing_id: meta.source_listing_id.clone(),
                disposition: SCRAPED_URL_DISPOSITION,
                expected_last_captured_raw_input_sha256: meta
                    .expected_last_captured_raw_input_sha256
                    .clone(),
            })
            .await
        {
            Ok(CrawlerUrlWriteOutcome::Applied) => ScrapeCandidateOutcome::default(),
            Ok(CrawlerUrlWriteOutcome::NoopStale) => {
                debug!(
                    listing_source_id = %meta.listing_source_id,
                    url = %meta.original_url,
                    crawler_url_write_outcome = "stale_noop",
                    "Skipped stale active crawler scrape completion"
                );
                ScrapeCandidateOutcome {
                    skipped: true,
                    ..Default::default()
                }
            }
            Err(error) => {
                warn!(error = %error, url = %meta.original_url, "Failed to persist unchanged raw scrape metadata");
                ScrapeCandidateOutcome {
                    errored: true,
                    ..Default::default()
                }
            }
        };
    }

    ScrapeCandidateOutcome {
        capture: Some(RawCaptureRequest {
            item: ProductListingRawCaptureItem::crawler(
                candidate.listing_source_id,
                &candidate.source_record_key,
                scraped.raw_input,
                provenance,
            )
            .acquired_at(scraped.acquired_at),
            on_success: RawCaptureSuccessAction::MarkScraped(meta),
            on_failure: RawCaptureFailureAction::None,
        }),
        ..Default::default()
    }
}

/// A page whose schema derives another identity than the one its listing was captured
/// under is not captured: the new identity would start a second listing. The conflict is
/// a schema outcome the reviewer resolves, so the URL keeps it with the schema cooldown
/// instead of being refetched on every scheduler pass.
async fn reject_identity_conflict(
    candidate: &ScraperCandidate,
    ctx: &ScrapeDomainContext,
    captured_identity: &str,
    scraped: &ScrapedProduct,
) -> ScrapeCandidateOutcome {
    warn!(
        event = "crawler.product_url_move_rejected_identity_conflict",
        listing_source_id = %candidate.listing_source_id,
        url = %candidate.url,
        captured_identity = %captured_identity,
        source_identity = %scraped.source_listing_id,
        "Rejecting scrape because crawler identity changed"
    );
    let error_message = format!(
        "extracted source listing id `{}` differs from the captured `{captured_identity}`",
        scraped.source_listing_id
    );
    persist_schema_retry_cooldown(
        candidate,
        ctx,
        SOURCE_LISTING_ID_CONFLICT_ERROR_KIND,
        &error_message,
    )
    .await;
    ScrapeCandidateOutcome {
        errored: true,
        failure_kind: Some(SOURCE_LISTING_ID_CONFLICT_ERROR_KIND.to_owned()),
        ..Default::default()
    }
}

struct HttpFailure {
    url: url::Url,
    kind: NetworkErrorKind,
    status_code: Option<u16>,
    retry_after: Option<Duration>,
    expected_last_captured_raw_input_sha256: Option<Vec<u8>>,
    fetch_stats: Option<DomainFetchStats>,
}

fn http_failure(
    candidate: &ScraperCandidate,
    error: &ScraperError,
    context: Option<FetchFailureContext>,
) -> Option<HttpFailure> {
    let (error_kind, error_status_code, error_retry_after) = match error {
        ScraperError::HttpError { kind, .. } => (*kind, None, None),
        ScraperError::HttpErrorWithMetadata(metadata) => {
            (metadata.kind, metadata.status_code, metadata.retry_after)
        }
        _ => return None,
    };
    let context = context.unwrap_or(FetchFailureContext {
        url: candidate.url.clone(),
        kind: error_kind,
        status_code: error_status_code,
        retry_after: error_retry_after,
        source: FetchFailureSource::Primary,
        expected_last_captured_raw_input_sha256: candidate.last_captured_raw_input_sha256.clone(),
        fetch_stats: None,
    });
    Some(HttpFailure {
        url: context.url,
        kind: context.kind,
        status_code: context.status_code.or(error_status_code),
        retry_after: context.retry_after.or(error_retry_after),
        expected_last_captured_raw_input_sha256: context.expected_last_captured_raw_input_sha256,
        fetch_stats: context.fetch_stats,
    })
}

async fn handle_scrape_error(
    candidate: &ScraperCandidate,
    ctx: &ScrapeDomainContext,
    error: ScraperError,
    fetch_failure: Option<FetchFailureContext>,
    domain_health: DomainFetchHealth,
    evidence: &DomainBatchEvidence,
) -> ScrapeCandidateOutcome {
    let error_message = error.to_string();
    let record = if let Some(failure) = http_failure(candidate, &error, fetch_failure) {
        persist_http_failure(candidate, ctx, &error_message, failure, evidence).await
    } else {
        persist_non_http_failure(candidate, ctx, &error, &error_message).await;
        ScrapeFailureRecord {
            error_kind: scraper_error_kind(&error).to_owned(),
            ..Default::default()
        }
    };
    record_batch_error_state(ctx, &error).await;

    ScrapeCandidateOutcome {
        errored: true,
        domain_health,
        http_failure_status: record.status_code,
        circuit_opened: record.circuit_opened,
        persisted_domain_health: record.persisted_domain_health,
        robots_unreachable: is_robots_unreachable(&error),
        failure_kind: Some(record.error_kind),
        ..Default::default()
    }
}

/// What persisting a scrape failure recorded, and for an HTTP failure what it did to
/// the domain.
#[derive(Default)]
struct ScrapeFailureRecord {
    /// The error kind recorded on the URL.
    error_kind: String,
    status_code: Option<u16>,
    circuit_opened: bool,
    /// The domain snapshot an applied domain write left behind.
    persisted_domain_health: Option<DomainHealthSnapshot>,
}

async fn persist_http_failure(
    candidate: &ScraperCandidate,
    ctx: &ScrapeDomainContext,
    error_message: &str,
    failure: HttpFailure,
    evidence: &DomainBatchEvidence,
) -> ScrapeFailureRecord {
    let cooldown = durable_retry_cooldown_for(failure.kind);
    let next_retry_at =
        time::OffsetDateTime::now_utc() + time::Duration::seconds(cooldown.as_secs() as i64);
    let status_code = failure.status_code.or(match failure.kind {
        NetworkErrorKind::HttpStatus(code) => Some(code),
        _ => None,
    });
    let error_kind = crawler_url_error_kind(failure.kind, status_code);
    let url_failure = FetchFailureRequest {
        listing_source_id: candidate.listing_source_id,
        url: failure.url.clone(),
        error_kind: error_kind.clone(),
        error_message: error_message.to_owned(),
        status_code: status_code.map(i32::from),
        next_retry_at,
        expected_last_captured_raw_input_sha256: failure
            .expected_last_captured_raw_input_sha256
            .clone(),
    };

    let decision = evidence.decide(
        &candidate.domain_health,
        &candidate.url,
        failure.kind,
        status_code,
    );
    let (circuit_opened, persisted_domain_health) = match decision {
        DomainCircuitDecision::UrlLocal => {
            persist_url_failure(ctx, url_failure).await;
            (false, None)
        }
        DomainCircuitDecision::RecordFailure(domain_kind) => (
            false,
            record_domain_failure(candidate, ctx, url_failure, domain_kind, status_code).await,
        ),
        DomainCircuitDecision::Open(domain_kind) => (
            true,
            open_domain_circuit(
                candidate,
                ctx,
                url_failure,
                domain_kind,
                status_code,
                &failure,
                evidence,
            )
            .await,
        ),
    };
    ScrapeFailureRecord {
        error_kind,
        status_code,
        circuit_opened,
        persisted_domain_health,
    }
}

/// Persists the URL failure and the domain failure as evidence, without a
/// cooldown: the domain keeps being scraped until the evidence is sustained.
async fn record_domain_failure(
    candidate: &ScraperCandidate,
    ctx: &ScrapeDomainContext,
    url_failure: FetchFailureRequest,
    domain_kind: DomainFailureKind,
    status_code: Option<u16>,
) -> Option<DomainHealthSnapshot> {
    let persisted =
        write_domain_failure(candidate, ctx, url_failure, domain_kind, status_code, None).await;
    match &persisted {
        Ok(Some(snapshot)) => info!(
            event = "crawler.scraper.domain_failure_recorded",
            domain_id = %candidate.domain_id,
            listing_source_domain = %candidate.listing_source_domain,
            error_kind = %domain_kind.as_str(),
            status_code = ?status_code,
            failure_streak = snapshot.scrape_failure_streak,
            "Recorded scraper domain failure without opening the circuit"
        ),
        Ok(None) => debug!(
            event = "crawler.scraper.domain_failure_record_stale",
            domain_id = %candidate.domain_id,
            listing_source_domain = %candidate.listing_source_domain,
            error_kind = %domain_kind.as_str(),
            "Skipped stale scraper domain failure record"
        ),
        Err(error) => warn!(
            event = "crawler.scraper.domain_failure_record_failed",
            error = %error,
            domain_id = %candidate.domain_id,
            listing_source_domain = %candidate.listing_source_domain,
            error_kind = %domain_kind.as_str(),
            "Failed to persist scraper domain failure"
        ),
    }
    persisted.ok().flatten()
}

/// Persists the URL failure and opens the domain circuit for `domain_kind`
/// with its cooldown, fenced by the candidate's domain snapshot.
async fn open_domain_circuit(
    candidate: &ScraperCandidate,
    ctx: &ScrapeDomainContext,
    url_failure: FetchFailureRequest,
    domain_kind: DomainFailureKind,
    status_code: Option<u16>,
    failure: &HttpFailure,
    evidence: &DomainBatchEvidence,
) -> Option<DomainHealthSnapshot> {
    let retry_after = failure.retry_after;
    let failure_streak = next_domain_failure_streak(&candidate.domain_health, domain_kind);
    let domain_cooldown = domain_cooldown(domain_kind, failure_streak, retry_after);
    let next_scrape_at =
        time::OffsetDateTime::now_utc() + time::Duration::seconds(domain_cooldown.as_secs() as i64);
    let persisted = write_domain_failure(
        candidate,
        ctx,
        url_failure,
        domain_kind,
        status_code,
        Some(next_scrape_at),
    )
    .await;
    match &persisted {
        Ok(Some(snapshot)) => warn!(
            event = "crawler.scraper.domain_circuit_opened",
            domain_id = %candidate.domain_id,
            listing_source_domain = %candidate.listing_source_domain,
            error_kind = %domain_kind.as_str(),
            status_code = ?status_code,
            failure_streak = snapshot.scrape_failure_streak,
            retry_after_seconds = retry_after.map(|value| value.as_secs()),
            next_scrape_at = %next_scrape_at,
            spider_active_on_domain = ctx
                .lock_manager
                .is_domain_locked(DomainLockScope::Spider, candidate.domain_id),
            batch_attempted_urls = evidence.attempted_urls() + 1,
            batch_slow_host_urls = evidence.slow_host_urls()
                + usize::from(!domain_kind.opens_on_first_failure()),
            batch_repeated_status_urls = status_code
                .map(|status| evidence.rejection_run_with(status, &candidate.url)),
            cooldown_seconds = domain_cooldown.as_secs(),
            latency_smoothed_ms = failure
                .fetch_stats
                .map(|stats| stats.smoothed_latency.as_millis()),
            latency_samples = failure.fetch_stats.map(|stats| stats.latency_samples),
            fetch_attempt_timeout_ms = failure
                .fetch_stats
                .map(|stats| stats.attempt_timeout.as_millis()),
            "Opened scraper domain circuit"
        ),
        Ok(None) => debug!(
            event = "crawler.scraper.domain_circuit_open_stale",
            domain_id = %candidate.domain_id,
            listing_source_domain = %candidate.listing_source_domain,
            error_kind = %domain_kind.as_str(),
            "Skipped stale scraper domain circuit opening"
        ),
        Err(error) => warn!(
            event = "crawler.scraper.domain_circuit_open_failed",
            error = %error,
            domain_id = %candidate.domain_id,
            listing_source_domain = %candidate.listing_source_domain,
            error_kind = %domain_kind.as_str(),
            "Failed to persist scraper domain circuit opening"
        ),
    }
    persisted.ok().flatten()
}

/// Writes the URL failure and the fenced domain failure. Returns the domain
/// snapshot the write left behind when it applied, `None` when the fence was
/// stale.
async fn write_domain_failure(
    candidate: &ScraperCandidate,
    ctx: &ScrapeDomainContext,
    url_failure: FetchFailureRequest,
    domain_kind: DomainFailureKind,
    status_code: Option<u16>,
    next_scrape_at: Option<time::OffsetDateTime>,
) -> Result<Option<DomainHealthSnapshot>, sqlx::Error> {
    let outcome = ctx
        .scraper_candidates
        .mark_fetch_failure_and_open_domain_circuit(DomainCircuitOpenRequest {
            url_failure,
            domain_id: candidate.domain_id,
            expected_domain_health: candidate.domain_health.clone(),
            domain_error_kind: domain_kind,
            domain_status_code: status_code.map(i32::from),
            next_scrape_at,
        })
        .await?;
    if outcome.url_failure == CrawlerUrlWriteOutcome::NoopStale {
        debug!(
            listing_source_id = %candidate.listing_source_id,
            crawler_url_write_outcome = "stale_noop",
            "Skipped stale crawler fetch failure metadata"
        );
    }
    Ok(match outcome.domain_circuit {
        CrawlerUrlWriteOutcome::Applied => {
            let failure_streak = outcome
                .persisted_failure_streak
                .expect("applied domain circuit must report its streak");
            Some(DomainHealthSnapshot {
                scrape_failure_streak: i32::try_from(failure_streak)
                    .expect("domain failure streak must fit persisted INT"),
                last_scrape_error_kind: Some(domain_kind.as_str().to_owned()),
                next_scrape_at,
            })
        }
        CrawlerUrlWriteOutcome::NoopStale => None,
    })
}

async fn persist_url_failure(ctx: &ScrapeDomainContext, request: FetchFailureRequest) {
    let url = request.url.clone();
    match ctx
        .scraper_candidates
        .mark_fetch_failure(
            &request.listing_source_id,
            &request.url,
            &request.error_kind,
            &request.error_message,
            request.status_code,
            request.next_retry_at,
            request.expected_last_captured_raw_input_sha256.as_deref(),
        )
        .await
    {
        Ok(CrawlerUrlWriteOutcome::Applied) => {}
        Ok(CrawlerUrlWriteOutcome::NoopStale) => debug!(
            url = %url,
            crawler_url_write_outcome = "stale_noop",
            "Skipped stale crawler fetch failure metadata"
        ),
        Err(error) => warn!(
            error = %error,
            url = %url,
            "Failed to persist scraper fetch failure metadata"
        ),
    }
}

/// How a scrape error that is not an HTTP failure is recorded on its URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NonHttpFailureRecord {
    /// The page waits on this pending product-schema review: the URL is retried after a
    /// delay that grows while it keeps waiting on the same review.
    SchemaReviewWait(CrawlerReviewId),
    /// Another schema-work outcome: the URL is retried after the schema cooldown.
    SchemaRetryCooldown,
    /// Any other failure: recorded without delaying the next scrape.
    ScraperFailure,
    /// The page was evaluated and has nothing to capture: the URL counts as scraped and
    /// keeps the outcome until a later scrape completes, without a retry.
    ScrapedWithoutCapture,
}

impl NonHttpFailureRecord {
    fn of(error: &ScraperError) -> Self {
        match error {
            ScraperError::MissingRequiredValue { .. } => return Self::ScrapedWithoutCapture,
            ScraperError::PendingSchemaReview { review_id, .. } => {
                return Self::SchemaReviewWait(*review_id);
            }
            _ => {}
        }
        let schema_work_outcome = matches!(
            error,
            ScraperError::SchemaRegenerationExhausted { .. }
                | ScraperError::FreshSchemaImplausible { .. }
                | ScraperError::SchemaClassificationRejected { .. }
                | ScraperError::LlmBudgetExceeded { .. }
        ) || is_schema_generation_validation_failure(error);
        if schema_work_outcome {
            Self::SchemaRetryCooldown
        } else {
            Self::ScraperFailure
        }
    }
}

async fn persist_non_http_failure(
    candidate: &ScraperCandidate,
    ctx: &ScrapeDomainContext,
    error: &ScraperError,
    error_message: &str,
) {
    let error_kind = scraper_error_kind(error);
    match NonHttpFailureRecord::of(error) {
        NonHttpFailureRecord::SchemaReviewWait(review_id) => {
            persist_schema_review_wait(candidate, ctx, review_id, error_kind, error_message).await;
        }
        NonHttpFailureRecord::SchemaRetryCooldown => {
            persist_schema_retry_cooldown(candidate, ctx, error_kind, error_message).await;
        }
        NonHttpFailureRecord::ScraperFailure => {
            persist_scraper_failure(candidate, ctx, error_kind, error_message).await;
        }
        NonHttpFailureRecord::ScrapedWithoutCapture => {
            persist_scraped_without_capture(candidate, ctx, error_kind, error_message).await;
        }
    }
}

async fn persist_scraped_without_capture(
    candidate: &ScraperCandidate,
    ctx: &ScrapeDomainContext,
    outcome_kind: &str,
    outcome_message: &str,
) {
    match ctx
        .scraper_candidates
        .mark_scraped_without_capture(
            &candidate.listing_source_id,
            &candidate.url,
            outcome_kind,
            outcome_message,
            candidate.last_captured_raw_input_sha256.as_deref(),
        )
        .await
    {
        Ok(CrawlerUrlWriteOutcome::Applied) => {}
        Ok(CrawlerUrlWriteOutcome::NoopStale) => debug!(
            listing_source_id = %candidate.listing_source_id,
            url = %candidate.url,
            crawler_url_write_outcome = "stale_noop",
            "Skipped stale crawler scrape outcome"
        ),
        Err(error) => warn!(error = %error, "Failed to persist scraper outcome"),
    }
}

/// Records a schema outcome on the URL with the bounded schema cooldown, so the URL waits
/// for the schemas to change instead of being retried on every scheduler pass.
async fn persist_schema_retry_cooldown(
    candidate: &ScraperCandidate,
    ctx: &ScrapeDomainContext,
    error_kind: &str,
    error_message: &str,
) {
    let next_retry_at = time::OffsetDateTime::now_utc()
        + time::Duration::seconds(SCHEMA_ERROR_RETRY_COOLDOWN.as_secs() as i64);
    persist_url_failure(
        ctx,
        FetchFailureRequest {
            listing_source_id: candidate.listing_source_id,
            url: candidate.url.clone(),
            error_kind: error_kind.to_owned(),
            error_message: error_message.to_owned(),
            status_code: None,
            next_retry_at,
            expected_last_captured_raw_input_sha256: candidate
                .last_captured_raw_input_sha256
                .clone(),
        },
    )
    .await;
}

/// Records that the page waits on a pending product-schema review. The retry delay grows
/// with each consecutive wait on the same review, so pages that cannot succeed before the
/// reviewer decides stop refetching every schema cooldown; approval or rejection releases them
/// at once.
async fn persist_schema_review_wait(
    candidate: &ScraperCandidate,
    ctx: &ScrapeDomainContext,
    review_id: CrawlerReviewId,
    error_kind: &str,
    error_message: &str,
) {
    let wait = SchemaReviewWait::after(candidate.schema_review_wait, review_id);
    let next_retry_at = time::OffsetDateTime::now_utc()
        + time::Duration::seconds(schema_review_wait_retry_delay(wait.streak).as_secs() as i64);
    let request = SchemaReviewWaitRequest {
        url_failure: FetchFailureRequest {
            listing_source_id: candidate.listing_source_id,
            url: candidate.url.clone(),
            error_kind: error_kind.to_owned(),
            error_message: error_message.to_owned(),
            status_code: None,
            next_retry_at,
            expected_last_captured_raw_input_sha256: candidate
                .last_captured_raw_input_sha256
                .clone(),
        },
        wait,
    };
    match ctx
        .scraper_candidates
        .mark_waiting_on_schema_review(request)
        .await
    {
        Ok(CrawlerUrlWriteOutcome::Applied) => {}
        Ok(CrawlerUrlWriteOutcome::NoopStale) => debug!(
            listing_source_id = %candidate.listing_source_id,
            url = %candidate.url,
            crawler_url_write_outcome = "stale_noop",
            "Skipped stale crawler schema review wait"
        ),
        Err(error) => warn!(
            error = %error,
            url = %candidate.url,
            "Failed to persist crawler schema review wait"
        ),
    }
}

/// The schema cooldown doubled for each earlier consecutive wait on the same review, capped
/// at [`SCHEMA_REVIEW_WAIT_MAX_RETRY_DELAY`].
fn schema_review_wait_retry_delay(streak: u32) -> Duration {
    let doublings = streak.saturating_sub(1);
    2_u32
        .checked_pow(doublings)
        .and_then(|factor| SCHEMA_ERROR_RETRY_COOLDOWN.checked_mul(factor))
        .map_or(SCHEMA_REVIEW_WAIT_MAX_RETRY_DELAY, |delay| {
            delay.min(SCHEMA_REVIEW_WAIT_MAX_RETRY_DELAY)
        })
}

async fn persist_scraper_failure(
    candidate: &ScraperCandidate,
    ctx: &ScrapeDomainContext,
    error_kind: &str,
    error_message: &str,
) {
    match ctx
        .scraper_candidates
        .mark_scraper_failure(
            &candidate.listing_source_id,
            &candidate.url,
            error_kind,
            error_message,
            candidate.last_captured_raw_input_sha256.as_deref(),
        )
        .await
    {
        Ok(CrawlerUrlWriteOutcome::Applied) => {}
        Ok(CrawlerUrlWriteOutcome::NoopStale) => debug!(
            listing_source_id = %candidate.listing_source_id,
            url = %candidate.url,
            crawler_url_write_outcome = "stale_noop",
            "Skipped stale crawler failure metadata"
        ),
        Err(error) => warn!(error = %error, "Failed to persist scraper failure metadata"),
    }
}

async fn record_batch_error_state(ctx: &ScrapeDomainContext, error: &ScraperError) {
    if let ScraperError::LlmBudgetExceeded {
        listing_source_id,
        max_calls,
        ..
    } = error
    {
        let mut exhausted = ctx.budget_exhausted_listing_sources.lock().await;
        if exhausted.insert(*listing_source_id) {
            info!(
                listing_source_id = %listing_source_id,
                max_calls,
                "LLM call budget exhausted for ListingSource; skipping remaining URLs in batch"
            );
        }
    } else if matches!(
        error,
        ScraperError::ProductListingRemoved { .. }
            | ScraperError::NotProductPage { .. }
            | ScraperError::PendingSchemaReview { .. }
    ) || is_robots_refusal(error)
    {
        debug!(error = %error, "Scraper run failed");
    } else {
        warn!(error = %error, "Scraper run failed");
    }
}

/// Returns a short, stable, machine-readable kind label for a [`ScraperError`].
///
/// These labels are persisted in `listing_source_urls.last_error_kind` so that
/// operators can filter / aggregate by error category without having to parse
/// the free-text message.  The `HttpError` variant is included for completeness
/// even though the caller currently only invokes this helper for non-HTTP errors.
fn scraper_error_kind(e: &ScraperError) -> &'static str {
    match e {
        ScraperError::HttpError { .. } | ScraperError::HttpErrorWithMetadata(_) => "HttpError",
        ScraperError::ProductListingRemoved { .. } => "ProductListingRemoved",
        ScraperError::NotProductPage { .. } => "NotProductPage",
        ScraperError::SchemaClassificationRejected { .. } => "SchemaClassificationRejected",
        ScraperError::NoHost { .. } => "NoHost",
        ScraperError::MissingDomainContext { .. } => "MissingDomainContext",
        error if is_schema_generation_validation_failure(error) => {
            "SchemaGenerationValidationFailed"
        }
        ScraperError::SchemaServiceError(_) => "SchemaServiceError",
        ScraperError::RemovedPageSchemaDatabaseError(_) => "RemovedPageSchemaDatabaseError",
        ScraperError::SchemaRegenerationExhausted { .. } => "SchemaRegenerationExhausted",
        // The label predates plausibility checks and stays stable for persisted rows.
        ScraperError::FreshSchemaImplausible { .. } => "FreshSchemaNormalizationFailed",
        ScraperError::MissingRequiredValue { .. } => "MissingRequiredValue",
        ScraperError::LlmBudgetExceeded { .. } => "LlmBudgetExceeded",
        ScraperError::NormalizationError(_) => "NormalizationError",
        ScraperError::RawNormalizationInput(_) => "RawNormalizationInput",
        ScraperError::SchemaFingerprint(_) => "SchemaFingerprint",
        ScraperError::PendingSchemaReview { .. } => "PendingSchemaReview",
    }
}

/// Model output that failed deterministic schema validation is a schema-generation
/// outcome, not an infrastructure failure; it receives the bounded schema cooldown.
fn is_schema_generation_validation_failure(error: &ScraperError) -> bool {
    matches!(
        error,
        ScraperError::SchemaServiceError(
            ProductListingSchemaServiceError::InvalidGeneratedSchema { .. }
                | ProductListingSchemaServiceError::StructuredResponseValidation { .. }
        )
    )
}

fn is_robots_unreachable(error: &ScraperError) -> bool {
    error.network_error_kind() == Some(NetworkErrorKind::RobotsUnreachable)
}

/// robots.txt refusals are the crawler honouring a shop's policy, not a
/// failure to report as a warning.
fn is_robots_refusal(error: &ScraperError) -> bool {
    error
        .network_error_kind()
        .is_some_and(NetworkErrorKind::is_robots_refusal)
}

fn crawler_url_error_kind(kind: NetworkErrorKind, status_code: Option<u16>) -> String {
    status_code
        .map(|status_code| format!("HTTP_{status_code}"))
        .unwrap_or_else(|| format!("{kind:?}"))
}

#[tracing::instrument(
    name = "crawler_scrape_domain_candidates",
    skip(candidates, ctx),
    fields(candidate_count = candidates.len())
)]
async fn scrape_domain_candidates(
    candidates: Vec<ScraperCandidate>,
    ctx: ScrapeDomainContext,
) -> ScrapeDomainOutcome {
    let mut outcome = ScrapeDomainOutcome {
        succeeded: 0,
        failed: 0,
        skipped: 0,
        deferred_urls: 0,
        deferred: false,
    };

    let Some(_domain_lock) =
        DomainLock::try_acquire(&ctx.lock_manager, DomainLockScope::Scraper, ctx.domain_id)
    else {
        debug!(
            domain_id = %ctx.domain_id,
            "Deferring scraper domain - scraper domain lock is currently held"
        );
        outcome.deferred = true;
        outcome.deferred_urls = candidates.len();
        return outcome;
    };

    let Some(mut metrics) = candidates.first().map(DomainBatchMetrics::of) else {
        return outcome;
    };
    let mut evidence = DomainBatchEvidence::default();
    // Every candidate was selected with the same domain snapshot; the batch
    // carries the snapshot forward from one fenced domain write to the next.
    let mut domain_health = candidates
        .first()
        .map(|candidate| candidate.domain_health.clone())
        .unwrap_or_default();
    let candidate_count = candidates.len();
    let batch_deadline = tokio::time::Instant::now() + ctx.batch_time_budget;
    for (candidate_index, mut candidate) in candidates.into_iter().enumerate() {
        if ctx.stop.is_requested() {
            let remaining = candidate_count - candidate_index;
            info!(
                event = "crawler.scraper.domain_batch_stopped",
                domain_id = %ctx.domain_id,
                deferred_urls = remaining,
                "Deferring remaining scraper URLs because the crawler is stopping"
            );
            outcome.deferred_urls += remaining;
            break;
        }
        // The first URL always runs, so every pass makes progress.
        if candidate_index > 0 && tokio::time::Instant::now() >= batch_deadline {
            let remaining = candidate_count - candidate_index;
            info!(
                event = "crawler.scraper.domain_batch_time_budget_reached",
                domain_id = %ctx.domain_id,
                deferred_urls = remaining,
                "Deferring remaining scraper URLs to a later pass"
            );
            outcome.deferred_urls += remaining;
            break;
        }
        let candidate_url = candidate.url.clone();
        candidate.domain_health = domain_health.clone();
        let candidate_outcome = scrape_candidate_in_batch(candidate, &ctx, &evidence).await;
        metrics.observe(&candidate_outcome);

        if candidate_outcome.errored {
            outcome.failed += 1;
        } else if let Some(pair) = candidate_outcome.capture {
            match enqueue_raw_capture(&ctx.command_tx, &metrics.listing_source_domain, pair).await {
                Ok(queue_wait) => {
                    outcome.succeeded += 1;

                    if queue_wait >= Duration::from_millis(10) {
                        warn!(
                            event = "crawler.raw_capture.enqueue_wait",
                            queue_wait_ms = queue_wait.as_millis(),
                            "Raw ProductListing capture queue applied backpressure"
                        );
                    }
                }
                Err(_) => {
                    error!("Command channel closed while scraper worker is running");
                    outcome.failed += 1;
                }
            }
        } else if candidate_outcome.deferred {
            outcome.deferred_urls += 1;
        } else if candidate_outcome.skipped {
            outcome.skipped += 1;
        } else {
            outcome.succeeded += 1;
        }

        evidence.observe(
            &candidate_url,
            &candidate_outcome.domain_health,
            candidate_outcome.http_failure_status,
        );
        if let Some(persisted) = candidate_outcome.persisted_domain_health {
            domain_health = persisted;
        }

        if candidate_outcome.circuit_opened {
            let remaining = candidate_count.saturating_sub(candidate_index + 1);
            warn!(
                event = "crawler.scraper.domain_circuit_batch_stopped",
                domain_id = %ctx.domain_id,
                attempted_urls = candidate_index + 1,
                skipped_urls = remaining,
                "Stopping remaining scraper URLs after a domain circuit failure"
            );
            outcome.skipped += remaining;
            break;
        }

        if matches!(
            candidate_outcome.domain_health,
            DomainFetchHealth::Responsive
        ) && domain_health.scrape_failure_streak > 0
        {
            match ctx
                .scraper_candidates
                .close_domain_circuit(&ctx.domain_id, &domain_health)
                .await
            {
                Ok(CrawlerUrlWriteOutcome::Applied) => {
                    info!(
                        event = "crawler.scraper.domain_circuit_closed",
                        domain_id = %ctx.domain_id,
                        previous_failure_streak = domain_health.scrape_failure_streak,
                        last_error_kind = ?domain_health.last_scrape_error_kind,
                        circuit_was_open = domain_health.next_scrape_at.is_some(),
                        "Closed scraper domain circuit after responsive fetch"
                    );
                    domain_health = DomainHealthSnapshot::default();
                }
                Ok(CrawlerUrlWriteOutcome::NoopStale) => debug!(
                    event = "crawler.scraper.domain_circuit_close_stale",
                    domain_id = %ctx.domain_id,
                    "Skipped stale scraper domain circuit close"
                ),
                Err(error) => warn!(
                    error = %error,
                    domain_id = %ctx.domain_id,
                    "Failed to close scraper domain circuit"
                ),
            }
        }

        // One unreachable robots.txt refuses the whole origin: the rest of
        // the batch waits for a later pass instead of each URL recording the
        // same refusal.
        if candidate_outcome.robots_unreachable {
            let remaining = candidate_count.saturating_sub(candidate_index + 1);
            info!(
                event = "crawler.scraper.robots_unreachable_batch_stopped",
                domain_id = %ctx.domain_id,
                deferred_urls = remaining,
                "Deferring remaining scraper URLs while robots.txt is unreachable"
            );
            outcome.deferred_urls += remaining;
            break;
        }
    }

    metrics.emit(&outcome);
    outcome
}

async fn run_raw_capture_collector(
    mut command_rx: mpsc::Receiver<QueuedRawCapture>,
    raw_capture: Arc<dyn ProductListingRawCaptureService>,
    scraper_candidates: Arc<dyn ScraperCandidateService>,
    capture_batch_size: usize,
    capture_max_batch_age: Duration,
) {
    let capture_batch_size = capture_batch_size.max(1);
    let capture_max_batch_age = capture_max_batch_age.max(Duration::from_millis(1));
    let mut pending = Vec::<QueuedRawCapture>::with_capacity(capture_batch_size);

    loop {
        if pending.is_empty() {
            match command_rx.recv().await {
                Some(item) => pending.push(item),
                None => break,
            }
        } else {
            let oldest_enqueued_at = pending
                .iter()
                .map(|item| item.enqueued_at)
                .min()
                .expect("pending batch is non-empty");
            let flush_deadline = oldest_enqueued_at + capture_max_batch_age;

            tokio::select! {
                received = command_rx.recv() => {
                    match received {
                        Some(item) => pending.push(item),
                        None => {
                            let batch = std::mem::take(&mut pending);
                            flush_batch(
                                &raw_capture,
                                &scraper_candidates,
                                batch,
                                command_rx.len(),
                            )
                            .await;
                            break;
                        }
                    }
                }
                _ = tokio::time::sleep_until(flush_deadline) => {
                    let batch = std::mem::take(&mut pending);
                    flush_batch(
                        &raw_capture,
                        &scraper_candidates,
                        batch,
                        command_rx.len(),
                    )
                    .await;
                }
            }
        }

        if pending.len() >= capture_batch_size {
            let batch = std::mem::take(&mut pending);
            flush_batch(&raw_capture, &scraper_candidates, batch, command_rx.len()).await;
        }
    }

    if !pending.is_empty() {
        flush_batch(&raw_capture, &scraper_candidates, pending, command_rx.len()).await;
    }
}

impl CrawlerCronJob {
    #[tracing::instrument(name = "crawler_run_scraper_once", skip(self))]
    pub(super) async fn run_scraper_once(&self) {
        let scraper_concurrency = self.config.scraper_concurrency;
        if scraper_concurrency == 0 {
            warn!(
                scraper_concurrency,
                "scraper_concurrency is 0, skipping scraper scheduler pass"
            );
            return;
        }
        if !self.admit_authoritative_scope_for_work("scraper").await {
            return;
        }

        let pass_start = tokio::time::Instant::now();
        self.emit_due_scraper_urls().await;
        info!(
            concurrency = scraper_concurrency,
            push_batch_size = self.config.effective_push_batch_size(),
            push_queue_capacity = self.config.effective_push_queue_capacity(),
            push_max_batch_age_ms = self.config.effective_push_max_batch_age().as_millis(),
            push_max_concurrency = self.config.effective_push_max_concurrency(),
            "Scraper scheduler pass starting"
        );
        let mut seen_domains: HashSet<crate::CrawlerDomainId> = HashSet::new();
        let mut active_domains: HashSet<crate::CrawlerDomainId> = HashSet::new();
        let mut pending_domains: VecDeque<(crate::CrawlerDomainId, Vec<ScraperCandidate>)> =
            VecDeque::new();
        let mut join_set: JoinSet<ScheduledScrapeDomainOutcome> = JoinSet::new();
        let (command_tx, command_rx) =
            mpsc::channel::<QueuedRawCapture>(self.config.effective_push_queue_capacity());

        let budget_exhausted_listing_sources = Arc::new(Mutex::new(HashSet::new()));

        let mut unique_listing_source_ids = HashSet::new();
        let mut total = 0usize;
        let mut succeeded = 0usize;
        let mut failed = 0usize;
        let mut skipped = 0usize;
        let mut started = false;
        let mut no_more_candidates = false;
        let mut deferred_domains = 0usize;
        let mut deferred_urls = 0usize;

        let raw_capture_collector = tokio::spawn(run_raw_capture_collector(
            command_rx,
            Arc::clone(&self.raw_capture),
            Arc::clone(&self.scraper_candidates),
            self.config.effective_push_batch_size(),
            self.config.effective_push_max_batch_age(),
        ));

        loop {
            while join_set.len() < scraper_concurrency && !self.stop.is_requested() {
                let pending_index = pending_domains
                    .iter()
                    .position(|(domain, _)| !active_domains.contains(domain));
                if let Some(pending_index) = pending_index {
                    let (domain, candidates) = pending_domains
                        .remove(pending_index)
                        .expect("pending domain index must remain valid");
                    let scraper = Arc::clone(&self.scraper_service);
                    let scraper_candidates = Arc::clone(&self.scraper_candidates);
                    let lock_manager = Arc::clone(&self.lock_manager);
                    let domain_tx = command_tx.clone();
                    let budget_exhausted_listing_sources =
                        Arc::clone(&budget_exhausted_listing_sources);
                    let batch_time_budget = self.config.scraper_domain_batch_time_budget;
                    let stop = self.stop.clone();
                    let span = tracing::info_span!("scrape_domain", domain_id = %domain);
                    active_domains.insert(domain);
                    total += candidates.len();

                    join_set.spawn(
                        async move {
                            let ctx = ScrapeDomainContext {
                                scraper,
                                scraper_candidates,
                                lock_manager,
                                command_tx: domain_tx,
                                budget_exhausted_listing_sources,
                                domain_id: domain,
                                batch_time_budget,
                                stop,
                            };

                            ScheduledScrapeDomainOutcome {
                                domain_id: domain,
                                outcome: scrape_domain_candidates(candidates, ctx).await,
                            }
                        }
                        .instrument(span),
                    );
                    continue;
                }

                if no_more_candidates {
                    break;
                }

                let mut excluded_domain_ids = seen_domains.clone();
                excluded_domain_ids.extend(active_domains.iter().copied());
                excluded_domain_ids.extend(pending_domains.iter().map(|(domain, _)| *domain));
                let excluded_domain_ids: Vec<crate::CrawlerDomainId> =
                    excluded_domain_ids.into_iter().collect();
                let candidates = match self
                    .scraper_candidates
                    .get_candidates(
                        self.config.effective_scraper_domain_batch_size() as i64,
                        self.config.scraper_urls_per_domain.max(1),
                        &excluded_domain_ids,
                    )
                    .await
                {
                    Ok(candidates) => candidates,
                    Err(e) => {
                        warn!(error = %e, "Failed to retrieve scraper candidates");
                        no_more_candidates = true;
                        break;
                    }
                };

                if candidates.is_empty() {
                    if !started && join_set.is_empty() && pending_domains.is_empty() {
                        debug!("No scraper candidates, skipping scheduler pass");
                        drop(command_tx);
                        if let Err(e) = raw_capture_collector.await {
                            error!(error = %e, "Scraper push collector task failed to join");
                        }
                        emit_pass_duration(CrawlerMetric::ScraperPassDuration, pass_start);
                        return;
                    }
                    no_more_candidates = true;
                    break;
                }

                if !started {
                    started = true;
                }

                let mut by_domain: HashMap<crate::CrawlerDomainId, Vec<ScraperCandidate>> =
                    HashMap::new();
                for candidate in candidates {
                    unique_listing_source_ids.insert(candidate.listing_source_id);
                    let domain = candidate.domain_id;
                    seen_domains.insert(domain);
                    by_domain.entry(domain).or_default().push(candidate);
                }

                if by_domain.is_empty() {
                    no_more_candidates = true;
                    break;
                }

                debug!(domains = by_domain.len(), "Candidates grouped by domain");
                pending_domains.extend(by_domain.into_iter());
            }

            if join_set.is_empty() {
                break;
            }

            match join_set.join_next().await {
                Some(Ok(scheduled)) => {
                    active_domains.remove(&scheduled.domain_id);
                    succeeded += scheduled.outcome.succeeded;
                    failed += scheduled.outcome.failed;
                    skipped += scheduled.outcome.skipped;
                    deferred_urls += scheduled.outcome.deferred_urls;
                    if scheduled.outcome.deferred {
                        deferred_domains += 1;
                    }
                }
                Some(Err(e)) => {
                    error!(error = %e, "Scraper domain worker task failed to join");
                    failed += 1;
                }
                None => break,
            }
        }

        drop(command_tx);

        if let Err(e) = raw_capture_collector.await {
            error!(error = %e, "Scraper push collector task failed to join");
            failed += 1;
        }

        let duration_ms = emit_pass_duration(CrawlerMetric::ScraperPassDuration, pass_start);
        // `total`, `succeeded`, `failed`, `skipped` and `deferred_urls` are all counted in
        // candidate URLs; `deferred_domains` counts whole domain batches that lost the
        // scraper domain lock and is reported separately so expected lock contention is
        // not mistaken for failures or meaningful skips.
        info!(
            total,
            succeeded,
            failed,
            skipped,
            deferred_urls,
            deferred_domains,
            duration_ms,
            "Scraper scheduler pass complete"
        );

        #[cfg(not(test))]
        {
            match self
                .scraper_candidates
                .get_listing_source_llm_usage(unique_listing_source_ids.into_iter().collect())
                .await
            {
                Ok(usages) => {
                    for usage in usages {
                        debug!(
                            listing_source_name = %usage.listing_source_name,
                            llm_calls_count = usage.llm_calls_count,
                            llm_calls_cap = self.config.scraper_max_llm_calls_per_listing_source,
                            llm_budget_exhausted = usage.llm_calls_count >= self.config.scraper_max_llm_calls_per_listing_source,
                            "ListingSource LLM usage summary"
                        );
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Failed to load per-ListingSource LLM usage summary");
                }
            }
        }

        self.scraper_perf.record(succeeded as u64, duration_ms);
    }

    async fn emit_due_scraper_urls(&self) {
        match self.scraper_candidates.count_due_urls().await {
            Ok(due_urls) => MetricRecord::global()
                .with_value(CrawlerMetric::ScraperDueUrls, due_urls)
                .emit(),
            Err(error) => warn!(error = %error, "Failed to count due scraper URLs"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::candidate_service::{
        DomainCircuitOpenOutcome, DomainHealthSnapshot, MockScraperCandidateService,
    };
    use crate::scraper::scraper_service::domain::product::record_transport_failure;
    use crate::scraper::scraper_service::{MockScraperService, ScrapedProduct};
    use crate::service::cron::config::CrawlerCronConfig;
    use crate::service::cron::test_support::{noop_listing_source_registration, scraper_candidate};
    use crate::service::listing_source_registration::{
        ListingSourceRegistrationService, ListingSourceSyncError,
        MockListingSourceRegistrationRepository, MockListingSourceRegistrationSource,
    };
    use crate::service::raw_capture::MockProductListingRawCaptureService;
    use crate::spider::candidate_service::MockSpiderCandidateService;
    use crate::spider::local_lock::LocalLockManager;
    use crate::spider::service::MockSpiderService;
    use listing_source_core::ListingSourceId;

    #[test]
    fn should_classify_invalid_generated_schemas_as_schema_generation_outcomes() {
        for error in [
            ProductListingSchemaServiceError::InvalidGeneratedSchema {
                source: application::error::box_error(std::io::Error::other("invalid rule")),
            },
            ProductListingSchemaServiceError::StructuredResponseValidation {
                source: application::error::box_error(std::io::Error::other("invalid")),
            },
        ] {
            let error = ScraperError::SchemaServiceError(error);
            assert!(is_schema_generation_validation_failure(&error));
            assert_eq!(
                scraper_error_kind(&error),
                "SchemaGenerationValidationFailed"
            );
        }

        let database = ScraperError::SchemaServiceError(
            ProductListingSchemaServiceError::DatabaseError(sqlx::Error::PoolTimedOut),
        );
        assert!(!is_schema_generation_validation_failure(&database));
        assert_eq!(scraper_error_kind(&database), "SchemaServiceError");
    }

    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    type ScraperCandidateResultFuture =
        Pin<Box<dyn Future<Output = Result<Vec<ScraperCandidate>, sqlx::Error>> + Send>>;

    fn empty_spider_dependencies() -> (MockSpiderCandidateService, MockSpiderService) {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(|_, _| Box::pin(async { Ok(vec![]) }));

        (spider_candidates, MockSpiderService::new())
    }

    fn no_raw_capture_service() -> Box<MockProductListingRawCaptureService> {
        let mut raw_capture = MockProductListingRawCaptureService::new();
        raw_capture.expect_capture().times(0);
        Box::new(raw_capture)
    }

    fn applied_domain_circuit_open_outcome() -> DomainCircuitOpenOutcome {
        DomainCircuitOpenOutcome {
            url_failure: CrawlerUrlWriteOutcome::Applied,
            domain_circuit: CrawlerUrlWriteOutcome::Applied,
            persisted_failure_streak: Some(1),
        }
    }

    fn successful_scrape_outcome() -> ScrapeOutcome {
        ScrapeOutcome {
            result: Ok(None),
            domain_health: DomainFetchHealth::NotObserved,
            fetch_failure: None,
        }
    }

    fn scrape_outcome(result: Result<Option<ScrapedProduct>, ScraperError>) -> ScrapeOutcome {
        ScrapeOutcome {
            result,
            domain_health: DomainFetchHealth::NotObserved,
            fetch_failure: None,
        }
    }

    fn item(listing_source_id: ListingSourceId, product_id: &str) -> ProductListingRawCaptureItem {
        let url = url::Url::parse(&format!("https://example.test/products/{product_id}"))
            .unwrap_or_else(|error| panic!("test URL: {error}"));
        let input = crawler_verified_removal_input(&url)
            .unwrap_or_else(|error| panic!("test removal input: {error}"));
        let provenance = crawler_provenance(None, None)
            .unwrap_or_else(|error| panic!("test provenance: {error}"));
        ProductListingRawCaptureItem::crawler(listing_source_id, url.as_str(), input, provenance)
    }

    fn meta(listing_source_id: ListingSourceId, url: &str, hash: &str) -> CandidateMeta {
        let url = url::Url::parse(url).unwrap();
        CandidateMeta {
            listing_source_id,
            original_url: url.clone(),
            effective_url: url,
            hash: hash.to_owned(),
            schema_fingerprint: "schema-fingerprint".to_owned(),
            raw_input_sha256: vec![3; 32],
            source_listing_id: "source-id".to_owned(),
            expected_last_captured_raw_input_sha256: None,
        }
    }

    fn queued(item: ProductListingRawCaptureItem, meta: CandidateMeta) -> QueuedRawCapture {
        QueuedRawCapture {
            request: (item, meta).into(),
            listing_source_domain: "example.test".to_owned(),
            enqueued_at: tokio::time::Instant::now(),
        }
    }

    #[tokio::test]
    async fn should_mark_only_the_matching_successful_push_input_as_scraped() {
        let first_listing_source_id = ListingSourceId::new();
        let second_listing_source_id = ListingSourceId::new();
        let first_url = url::Url::parse("https://first.example/product").unwrap();
        let observed_raw_input_sha256 = vec![9; 32];
        let observed_raw_input_sha256_for_mark = observed_raw_input_sha256.clone();
        let mut push_service = MockProductListingRawCaptureService::new();
        push_service.expect_capture().once().returning(|products| {
            assert_eq!(products.len(), 2);
            Box::pin(async {
                vec![
                    ProductListingRawCaptureOutcome::Persisted,
                    ProductListingRawCaptureOutcome::RetryableFailure,
                ]
            })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_as_scraped()
            .once()
            .withf(move |completion| {
                completion.listing_source_id == first_listing_source_id
                    && completion.url == first_url
                    && completion.hash == "first"
                    && completion.disposition == CrawlerDisposition::Active
                    && completion
                        .expected_last_captured_raw_input_sha256
                        .as_deref()
                        == Some(observed_raw_input_sha256_for_mark.as_slice())
            })
            .returning(|_| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

        let push_service: Arc<dyn ProductListingRawCaptureService> = Arc::new(push_service);
        let scraper_candidates: Arc<dyn ScraperCandidateService> = Arc::new(scraper_candidates);
        let mut first_meta = meta(
            first_listing_source_id,
            "https://first.example/product",
            "first",
        );
        first_meta.expected_last_captured_raw_input_sha256 = Some(observed_raw_input_sha256);

        flush_batch(
            &push_service,
            &scraper_candidates,
            vec![
                queued(item(first_listing_source_id, "same-product-id"), first_meta),
                queued(
                    item(second_listing_source_id, "same-product-id"),
                    meta(
                        second_listing_source_id,
                        "https://second.example/product",
                        "second",
                    ),
                ),
            ],
            0,
        )
        .await;
    }

    #[tokio::test]
    async fn should_not_mark_missing_source_capture_as_persisted_or_failed() {
        let listing_source_id = ListingSourceId::new();
        let mut raw_capture = MockProductListingRawCaptureService::new();
        raw_capture.expect_capture().once().returning(|products| {
            assert_eq!(products.len(), 1);
            Box::pin(async { vec![ProductListingRawCaptureOutcome::DiscardedMissingSource] })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates.expect_mark_as_scraped().never();
        scraper_candidates.expect_mark_scraper_failure().never();

        let raw_capture: Arc<dyn ProductListingRawCaptureService> = Arc::new(raw_capture);
        let scraper_candidates: Arc<dyn ScraperCandidateService> = Arc::new(scraper_candidates);
        flush_batch(
            &raw_capture,
            &scraper_candidates,
            vec![queued(
                item(listing_source_id, "deleted-source"),
                meta(
                    listing_source_id,
                    "https://deleted.example/product",
                    "deleted-source",
                ),
            )],
            0,
        )
        .await;
    }

    #[tokio::test]
    async fn should_not_move_a_redirected_url_when_raw_capture_fails() {
        let listing_source_id = ListingSourceId::new();
        let mut raw_capture = MockProductListingRawCaptureService::new();
        raw_capture.expect_capture().once().returning(|_| {
            Box::pin(async { vec![ProductListingRawCaptureOutcome::RetryableFailure] })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates.expect_mark_as_scraped().never();
        scraper_candidates.expect_move_redirected_url().never();

        let mut redirected = meta(listing_source_id, "https://example.com/product/old", "old");
        redirected.effective_url = url::Url::parse("https://example.com/product/new").unwrap();
        let raw_capture: Arc<dyn ProductListingRawCaptureService> = Arc::new(raw_capture);
        let scraper_candidates: Arc<dyn ScraperCandidateService> = Arc::new(scraper_candidates);
        flush_batch(
            &raw_capture,
            &scraper_candidates,
            vec![queued(item(listing_source_id, "SKU-123"), redirected)],
            0,
        )
        .await;
    }

    #[tokio::test]
    async fn should_default_missing_raw_capture_results_to_failure() {
        let first_listing_source_id = ListingSourceId::new();
        let second_listing_source_id = ListingSourceId::new();
        let first_url = url::Url::parse("https://first.example/product").unwrap();
        let mut push_service = MockProductListingRawCaptureService::new();
        push_service.expect_capture().once().returning(|products| {
            assert_eq!(products.len(), 2);
            Box::pin(async { vec![ProductListingRawCaptureOutcome::Persisted] })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_as_scraped()
            .once()
            .withf(move |completion| {
                completion.listing_source_id == first_listing_source_id
                    && completion.url == first_url
                    && completion.hash == "first"
                    && completion.expected_last_captured_raw_input_sha256.is_none()
            })
            .returning(|_| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

        let push_service: Arc<dyn ProductListingRawCaptureService> = Arc::new(push_service);
        let scraper_candidates: Arc<dyn ScraperCandidateService> = Arc::new(scraper_candidates);

        flush_batch(
            &push_service,
            &scraper_candidates,
            vec![
                queued(
                    item(first_listing_source_id, "first"),
                    meta(
                        first_listing_source_id,
                        "https://first.example/product",
                        "first",
                    ),
                ),
                queued(
                    item(second_listing_source_id, "second"),
                    meta(
                        second_listing_source_id,
                        "https://second.example/product",
                        "second",
                    ),
                ),
            ],
            0,
        )
        .await;
    }

    #[tokio::test]
    async fn should_apply_backpressure_when_raw_capture_queue_is_full() {
        let (command_tx, mut command_rx) = mpsc::channel::<QueuedRawCapture>(1);
        let listing_source_id = ListingSourceId::new();

        enqueue_raw_capture(
            &command_tx,
            "example.com",
            (
                item(listing_source_id, "first"),
                meta(
                    listing_source_id,
                    "https://example.com/product/first",
                    "first",
                ),
            ),
        )
        .await
        .expect("first enqueue must fit");

        let second_tx = command_tx.clone();
        let second = tokio::spawn(async move {
            enqueue_raw_capture(
                &second_tx,
                "example.com",
                (
                    item(listing_source_id, "second"),
                    meta(
                        listing_source_id,
                        "https://example.com/product/second",
                        "second",
                    ),
                ),
            )
            .await
        });

        tokio::task::yield_now().await;
        assert!(
            !second.is_finished(),
            "second enqueue must wait while the bounded queue is full"
        );

        assert!(command_rx.recv().await.is_some());

        let result = tokio::time::timeout(Duration::from_secs(1), second)
            .await
            .expect("second enqueue must unblock")
            .expect("second enqueue task must join");

        assert!(result.is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn should_flush_partial_batch_at_maximum_age() {
        let push_calls = Arc::new(AtomicUsize::new(0));
        let push_calls_for_mock = Arc::clone(&push_calls);

        let mut push_service = MockProductListingRawCaptureService::new();
        push_service
            .expect_capture()
            .once()
            .returning(move |products| {
                push_calls_for_mock.fetch_add(1, Ordering::SeqCst);
                let len = products.len();
                Box::pin(async move { vec![ProductListingRawCaptureOutcome::Persisted; len] })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_as_scraped()
            .once()
            .returning(|_| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

        let (tx, rx) = mpsc::channel(2);
        let collector = tokio::spawn(run_raw_capture_collector(
            rx,
            Arc::new(push_service),
            Arc::new(scraper_candidates),
            10,
            Duration::from_secs(5),
        ));

        let listing_source_id = ListingSourceId::new();
        enqueue_raw_capture(
            &tx,
            "example.com",
            (
                item(listing_source_id, "one"),
                meta(listing_source_id, "https://example.com/product/one", "one"),
            ),
        )
        .await
        .expect("enqueue must succeed");

        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::task::yield_now().await;

        assert_eq!(push_calls.load(Ordering::SeqCst), 1);

        drop(tx);
        collector.await.expect("collector task must join");
    }

    #[tokio::test]
    async fn should_flush_final_partial_batch_when_raw_capture_channel_closes() {
        let push_calls = Arc::new(AtomicUsize::new(0));
        let push_calls_for_mock = Arc::clone(&push_calls);

        let mut push_service = MockProductListingRawCaptureService::new();
        push_service
            .expect_capture()
            .once()
            .returning(move |products| {
                assert_eq!(products.len(), 2);
                push_calls_for_mock.fetch_add(1, Ordering::SeqCst);
                Box::pin(async {
                    vec![
                        ProductListingRawCaptureOutcome::Persisted,
                        ProductListingRawCaptureOutcome::Persisted,
                    ]
                })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_as_scraped()
            .times(2)
            .returning(|_| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

        let (tx, rx) = mpsc::channel(2);
        let collector = tokio::spawn(run_raw_capture_collector(
            rx,
            Arc::new(push_service),
            Arc::new(scraper_candidates),
            10,
            Duration::from_secs(5),
        ));

        let listing_source_id = ListingSourceId::new();
        enqueue_raw_capture(
            &tx,
            "example.com",
            (
                item(listing_source_id, "one"),
                meta(listing_source_id, "https://example.com/product/one", "one"),
            ),
        )
        .await
        .expect("first enqueue must succeed");
        enqueue_raw_capture(
            &tx,
            "example.com",
            (
                item(listing_source_id, "two"),
                meta(listing_source_id, "https://example.com/product/two", "two"),
            ),
        )
        .await
        .expect("second enqueue must succeed");

        drop(tx);
        collector.await.expect("collector task must join");

        assert_eq!(push_calls.load(Ordering::SeqCst), 1);
    }

    fn get_candidates_once_by_domain<F>(
        build_candidates: F,
    ) -> impl Fn(i64, i64, &[crate::CrawlerDomainId]) -> ScraperCandidateResultFuture
    + Send
    + Sync
    + 'static
    where
        F: Fn() -> Vec<ScraperCandidate> + Send + Sync + 'static,
    {
        move |_, _, excluded_domains| {
            let excluded_domains: HashSet<crate::CrawlerDomainId> =
                excluded_domains.iter().copied().collect();
            let candidates = build_candidates();
            Box::pin(async move {
                Ok(candidates
                    .into_iter()
                    .filter(|candidate| !excluded_domains.contains(&candidate.domain_id))
                    .collect())
            })
        }
    }

    fn expect_due_url_count(scraper_candidates: &mut MockScraperCandidateService, due_urls: u64) {
        scraper_candidates
            .expect_count_due_urls()
            .returning(move || Box::pin(async move { Ok(due_urls) }));
    }

    fn scraper_job(
        config: CrawlerCronConfig,
        mut scraper_candidates: MockScraperCandidateService,
        scraper_service: MockScraperService,
    ) -> CrawlerCronJob {
        expect_due_url_count(&mut scraper_candidates, 0);
        let (spider_candidates, spider_service) = empty_spider_dependencies();

        CrawlerCronJob::new(
            config,
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            no_raw_capture_service(),
        )
    }

    fn scrape_candidate_context(
        scraper_candidates: MockScraperCandidateService,
        scraper_service: MockScraperService,
    ) -> ScrapeDomainContext {
        let (command_tx, _command_rx) = tokio::sync::mpsc::channel(1);

        ScrapeDomainContext {
            scraper: Arc::new(scraper_service),
            scraper_candidates: Arc::new(scraper_candidates),
            lock_manager: Arc::new(LocalLockManager::new()),
            command_tx,
            budget_exhausted_listing_sources: Arc::new(Mutex::new(HashSet::new())),
            domain_id: crate::CrawlerDomainId::new(),
            batch_time_budget: CrawlerCronConfig::default().scraper_domain_batch_time_budget,
            stop: CronStop::default(),
        }
    }

    #[tokio::test]
    async fn should_defer_a_contended_scraper_domain_without_losing_candidates() {
        let mut candidate = scraper_candidate(
            "ListingSource",
            url::Url::parse("https://example.com/product/1").unwrap(),
        );
        candidate.is_domain_probe = true;
        let domain_id = candidate.domain_id;
        let lock_manager = Arc::new(LocalLockManager::new());
        let held_lock = DomainLock::try_acquire(&lock_manager, DomainLockScope::Scraper, domain_id)
            .expect("test must hold the domain lock before scheduling");

        let (command_tx, _command_rx) = tokio::sync::mpsc::channel(1);
        let first_context = ScrapeDomainContext {
            scraper: Arc::new(MockScraperService::new()),
            scraper_candidates: Arc::new(MockScraperCandidateService::new()),
            lock_manager: Arc::clone(&lock_manager),
            command_tx,
            budget_exhausted_listing_sources: Arc::new(Mutex::new(HashSet::new())),
            domain_id,
            batch_time_budget: CrawlerCronConfig::default().scraper_domain_batch_time_budget,
            stop: CronStop::default(),
        };

        let deferred = scrape_domain_candidates(vec![candidate], first_context).await;
        assert!(deferred.deferred);
        assert_eq!(deferred.deferred_urls, 1);
        assert_eq!(deferred.succeeded, 0);
        assert_eq!(deferred.failed, 0);
        assert_eq!(deferred.skipped, 0);

        drop(held_lock);

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .once()
            .returning(|_| Box::pin(async { successful_scrape_outcome() }));
        let (command_tx, _command_rx) = tokio::sync::mpsc::channel(1);
        let retry_context = ScrapeDomainContext {
            scraper: Arc::new(scraper_service),
            scraper_candidates: Arc::new(MockScraperCandidateService::new()),
            lock_manager,
            command_tx,
            budget_exhausted_listing_sources: Arc::new(Mutex::new(HashSet::new())),
            domain_id,
            batch_time_budget: CrawlerCronConfig::default().scraper_domain_batch_time_budget,
            stop: CronStop::default(),
        };

        let mut retry_candidate = scraper_candidate(
            "ListingSource",
            url::Url::parse("https://example.com/product/1").unwrap(),
        );
        retry_candidate.is_domain_probe = true;
        retry_candidate.domain_id = domain_id;
        let retried = scrape_domain_candidates(vec![retry_candidate], retry_context).await;
        assert!(!retried.deferred);
        assert_eq!(retried.skipped, 1);
    }

    #[tokio::test]
    async fn should_scrape_a_domain_while_the_spider_holds_its_own_domain_lock() {
        let url = url::Url::parse("https://example.com/product/1").unwrap();
        let candidate = scraper_candidate("ListingSource", url.clone());
        let domain_id = candidate.domain_id;
        let lock_manager = Arc::new(LocalLockManager::new());
        let _spider_lock =
            DomainLock::try_acquire(&lock_manager, DomainLockScope::Spider, domain_id)
                .expect("test must hold the spider domain lock before scheduling");

        let raw_input = crawler_verified_removal_input(&url)
            .unwrap_or_else(|error| panic!("test raw input: {error}"));
        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .once()
            .return_once(move |_| {
                Box::pin(async move {
                    scrape_outcome(Ok(Some(ScrapedProduct {
                        acquired_at: time::OffsetDateTime::now_utc(),
                        raw_input,
                        hash: "hash".to_owned(),
                        schema_fingerprint: "schema".to_owned(),
                        raw_input_sha256: vec![7; 32],
                        effective_url: url::Url::parse("https://example.com/product/1").unwrap(),
                        source_listing_id: "ListingSource".to_owned(),
                    })))
                })
            });
        let (command_tx, mut command_rx) = tokio::sync::mpsc::channel(1);
        let context = ScrapeDomainContext {
            scraper: Arc::new(scraper_service),
            scraper_candidates: Arc::new(MockScraperCandidateService::new()),
            lock_manager,
            command_tx,
            budget_exhausted_listing_sources: Arc::new(Mutex::new(HashSet::new())),
            domain_id,
            batch_time_budget: CrawlerCronConfig::default().scraper_domain_batch_time_budget,
            stop: CronStop::default(),
        };

        let outcome = scrape_domain_candidates(vec![candidate], context).await;
        assert!(!outcome.deferred);
        assert_eq!(outcome.deferred_urls, 0);
        assert_eq!(outcome.skipped, 0);
        assert_eq!(outcome.succeeded, 1);
        let queued = command_rx
            .try_recv()
            .expect("scrape under a held spider lock must queue its raw capture");
        let RawCaptureSuccessAction::MarkScraped(meta) = queued.request.on_success else {
            panic!("scrape under a held spider lock must queue a scrape completion");
        };
        assert_eq!(meta.original_url, url);
    }

    #[tokio::test]
    async fn should_count_listing_source_lock_contention_as_deferred_urls() {
        let candidate = scraper_candidate(
            "ListingSource",
            url::Url::parse("https://example.com/product/1").unwrap(),
        );
        let domain_id = candidate.domain_id;
        let lock_manager = Arc::new(LocalLockManager::new());
        let _listing_source_lock =
            ListingSourceLock::try_acquire(&lock_manager, candidate.listing_source_id)
                .expect("test must hold the ListingSource lock before scheduling");

        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().never();
        let (command_tx, _command_rx) = tokio::sync::mpsc::channel(1);
        let context = ScrapeDomainContext {
            scraper: Arc::new(scraper_service),
            scraper_candidates: Arc::new(MockScraperCandidateService::new()),
            lock_manager,
            command_tx,
            budget_exhausted_listing_sources: Arc::new(Mutex::new(HashSet::new())),
            domain_id,
            batch_time_budget: CrawlerCronConfig::default().scraper_domain_batch_time_budget,
            stop: CronStop::default(),
        };

        let outcome = scrape_domain_candidates(vec![candidate], context).await;
        assert!(!outcome.deferred);
        assert_eq!(outcome.deferred_urls, 1);
        assert_eq!(outcome.skipped, 0);
        assert_eq!(outcome.succeeded, 0);
        assert_eq!(outcome.failed, 0);
    }

    #[tokio::test]
    async fn should_skip_scraper_candidates_when_authoritative_scope_refresh_fails() {
        let mut source = MockListingSourceRegistrationSource::new();
        source
            .expect_fetch_registered_listing_sources()
            .once()
            .returning(|| {
                Box::pin(async {
                    Err(ListingSourceSyncError::FetchError(
                        "business unavailable".to_owned(),
                    ))
                })
            });
        let registration = ListingSourceRegistrationService::new(
            Box::new(source),
            Box::new(MockListingSourceRegistrationRepository::new()),
        );
        let (spider_candidates, spider_service) = empty_spider_dependencies();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(MockScraperCandidateService::new()),
            Box::new(MockScraperService::new()),
            registration,
            no_raw_capture_service(),
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_run_scraper_candidates_and_push_products() {
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(|| {
                vec![scraper_candidate(
                    "Test ListingSource",
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )]
            }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .returning(|_| Box::pin(async { successful_scrape_outcome() }));

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;

        assert_eq!(job.scraper_perf.snapshot(), (0, 0));
    }

    #[tokio::test]
    async fn should_apply_durable_retry_cooldown_after_final_fetch_failure() {
        let before = time::OffsetDateTime::now_utc();
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(|| {
                vec![scraper_candidate(
                    "Test ListingSource",
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )]
            }));
        scraper_candidates
            .expect_mark_fetch_failure_and_open_domain_circuit()
            .once()
            .withf(move |request| {
                let expected_cooldown = durable_retry_cooldown_for(NetworkErrorKind::Timeout);
                let expected_from =
                    before + time::Duration::seconds(expected_cooldown.as_secs() as i64);
                let expected_until = time::OffsetDateTime::now_utc()
                    + time::Duration::seconds(expected_cooldown.as_secs() as i64)
                    + time::Duration::seconds(1);
                request.url_failure.status_code.is_none()
                    && request.url_failure.next_retry_at >= expected_from
                    && request.url_failure.next_retry_at <= expected_until
                    && request
                        .url_failure
                        .expected_last_captured_raw_input_sha256
                        .is_none()
                    && request.expected_domain_health.scrape_failure_streak == 0
                    && request.domain_error_kind
                        == crate::network::policy::DomainFailureKind::Timeout
                    && request.domain_status_code.is_none()
                    // A single timeout is evidence, not an open circuit.
                    && request.next_scrape_at.is_none()
            })
            .returning(|_| Box::pin(async { Ok(applied_domain_circuit_open_outcome()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().returning(|request| {
            let url = request.url;
            Box::pin(async move {
                scrape_outcome(Err(ScraperError::HttpError {
                    url,
                    kind: crate::network::policy::NetworkErrorKind::Timeout,
                    details: "timeout".to_string(),
                }))
            })
        });

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;

        assert_eq!(job.scraper_perf.snapshot(), (0, 0));
    }

    /// Slow-host failures (timeouts, connection failures, DNS timeouts) open
    /// the domain circuit only on sustained evidence; see
    /// `docs/crawler/scheduling-and-retries.md`, scraper domain circuits.
    mod slow_host_circuits {
        use super::*;

        fn timeout_error(url: url::Url) -> ScraperError {
            ScraperError::HttpError {
                url,
                kind: NetworkErrorKind::Timeout,
                details: "timeout".to_owned(),
            }
        }

        /// Reports the streak the persisted write would hold, like the
        /// Postgres adapter does.
        fn applied_with_next_streak(
            request: &DomainCircuitOpenRequest,
        ) -> Result<DomainCircuitOpenOutcome, sqlx::Error> {
            Ok(DomainCircuitOpenOutcome {
                url_failure: CrawlerUrlWriteOutcome::Applied,
                domain_circuit: CrawlerUrlWriteOutcome::Applied,
                persisted_failure_streak: Some(next_domain_failure_streak(
                    &request.expected_domain_health,
                    request.domain_error_kind,
                )),
            })
        }

        /// A scraper that times out on every URL except those in `responsive`.
        fn scraper_timing_out_except(responsive: &[usize]) -> MockScraperService {
            let responsive = responsive.to_vec();
            let mut scraper_service = MockScraperService::new();
            scraper_service.expect_scrape().returning(move |request| {
                let index = request
                    .url
                    .path_segments()
                    .and_then(|mut segments| segments.next_back())
                    .and_then(|segment| segment.parse::<usize>().ok())
                    .expect("test URLs end in their index");
                let url = request.url;
                let responds = responsive.contains(&index);
                Box::pin(async move {
                    if responds {
                        ScrapeOutcome {
                            result: Ok(None),
                            domain_health: DomainFetchHealth::Responsive,
                            fetch_failure: None,
                        }
                    } else {
                        scrape_outcome(Err(timeout_error(url)))
                    }
                })
            });
            scraper_service
        }

        #[tokio::test]
        async fn should_continue_the_batch_after_a_single_timeout() {
            let (candidates, domain_id) = domain_batch("slow-once.example.com", 3);
            let mut scraper_candidates = MockScraperCandidateService::new();
            scraper_candidates
                .expect_mark_fetch_failure_and_open_domain_circuit()
                .once()
                .withf(|request| {
                    request.domain_error_kind == DomainFailureKind::Timeout
                        && request.next_scrape_at.is_none()
                })
                .returning(|request| {
                    let outcome = applied_with_next_streak(&request);
                    Box::pin(async move { outcome })
                });
            scraper_candidates
                .expect_close_domain_circuit()
                .once()
                .withf(|_, expected| {
                    expected.scrape_failure_streak == 1
                        && expected.last_scrape_error_kind.as_deref() == Some("TIMEOUT")
                        && expected.next_scrape_at.is_none()
                })
                .returning(|_, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

            let outcome = scrape_domain_candidates(
                candidates,
                domain_context(
                    scraper_candidates,
                    scraper_timing_out_except(&[2, 3]),
                    domain_id,
                    Duration::from_secs(60),
                ),
            )
            .await;

            assert_eq!(outcome.failed, 1);
            assert_eq!(outcome.skipped, 2, "the responsive URLs ran");
        }

        #[tokio::test]
        async fn should_open_the_circuit_on_the_second_consecutive_timeout() {
            let (candidates, domain_id) = domain_batch("slow-twice.example.com", 3);
            let mut scraper_candidates = MockScraperCandidateService::new();
            scraper_candidates
                .expect_mark_fetch_failure_and_open_domain_circuit()
                .times(2)
                .withf(|request| {
                    request.domain_error_kind == DomainFailureKind::Timeout
                        && match request.expected_domain_health.scrape_failure_streak {
                            0 => request.next_scrape_at.is_none(),
                            1 => {
                                request
                                    .expected_domain_health
                                    .last_scrape_error_kind
                                    .as_deref()
                                    == Some("TIMEOUT")
                                    && request.next_scrape_at.is_some()
                            }
                            _ => false,
                        }
                })
                .returning(|request| {
                    let outcome = applied_with_next_streak(&request);
                    Box::pin(async move { outcome })
                });

            let outcome = scrape_domain_candidates(
                candidates,
                domain_context(
                    scraper_candidates,
                    scraper_timing_out_except(&[]),
                    domain_id,
                    Duration::from_secs(60),
                ),
            )
            .await;

            assert_eq!(outcome.failed, 2);
            assert_eq!(outcome.skipped, 1, "the batch stops once the circuit opens");
        }

        #[tokio::test]
        async fn should_open_the_circuit_on_a_timeout_after_evidence_from_an_earlier_pass() {
            let (mut candidates, domain_id) = domain_batch("slow-again.example.com", 2);
            for candidate in &mut candidates {
                candidate.domain_health = DomainHealthSnapshot {
                    scrape_failure_streak: 1,
                    last_scrape_error_kind: Some("TIMEOUT".to_owned()),
                    next_scrape_at: None,
                };
            }
            let mut scraper_candidates = MockScraperCandidateService::new();
            scraper_candidates
                .expect_mark_fetch_failure_and_open_domain_circuit()
                .once()
                .withf(|request| {
                    request.expected_domain_health.scrape_failure_streak == 1
                        && request.next_scrape_at.is_some()
                })
                .returning(|request| {
                    let outcome = applied_with_next_streak(&request);
                    Box::pin(async move { outcome })
                });

            let outcome = scrape_domain_candidates(
                candidates,
                domain_context(
                    scraper_candidates,
                    scraper_timing_out_except(&[]),
                    domain_id,
                    Duration::from_secs(60),
                ),
            )
            .await;

            assert_eq!(outcome.failed, 1);
            assert_eq!(outcome.skipped, 1);
        }

        #[tokio::test]
        async fn should_open_the_circuit_when_half_of_the_batch_times_out() {
            let (candidates, domain_id) = domain_batch("slow-half.example.com", 7);
            let mut scraper_candidates = MockScraperCandidateService::new();
            // URLs 1 and 3 time out and are recorded; the responsive URLs 2 and 4
            // clear the evidence; URL 5 is the third timeout of five attempts.
            scraper_candidates
                .expect_mark_fetch_failure_and_open_domain_circuit()
                .times(3)
                .withf(|request| {
                    request.expected_domain_health.scrape_failure_streak == 0
                        && (request.url_failure.url.path() == "/product/5")
                            == request.next_scrape_at.is_some()
                })
                .returning(|request| {
                    let outcome = applied_with_next_streak(&request);
                    Box::pin(async move { outcome })
                });
            scraper_candidates
                .expect_close_domain_circuit()
                .times(2)
                .returning(|_, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

            let outcome = scrape_domain_candidates(
                candidates,
                domain_context(
                    scraper_candidates,
                    scraper_timing_out_except(&[2, 4]),
                    domain_id,
                    Duration::from_secs(60),
                ),
            )
            .await;

            assert_eq!(outcome.failed, 3);
            assert_eq!(
                outcome.skipped,
                2 + 2,
                "two responsive URLs, two never attempted"
            );
        }
    }

    #[tokio::test]
    async fn should_mark_removed_url_active_only_after_raw_capture() {
        let url = url::Url::parse("https://example.com/product/removed").unwrap();
        let candidate = scraper_candidate("ListingSource", url.clone());
        let listing_source_id = candidate.listing_source_id;
        let outcome = handle_verified_removal(&candidate);
        let request = outcome.capture.expect("removal must be queued for capture");

        let mut raw_capture = MockProductListingRawCaptureService::new();
        raw_capture
            .expect_capture()
            .once()
            .withf(move |observations| {
                observations.len() == 1
                    && observations[0].command.listing_source_id == listing_source_id
                    && observations[0].command.source_record_key
                        == "https://example.com/product/removed"
                    && observations[0].command.input.operation()
                        == product_listing_normalization::RawProductListingOperation::Delete
            })
            .returning(|_| Box::pin(async { vec![ProductListingRawCaptureOutcome::Persisted] }));

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_removed()
            .once()
            .withf(
                move |source_id, candidate_url, raw_input_sha256, expected_raw_input_sha256| {
                    *source_id == listing_source_id
                        && candidate_url.as_str() == "https://example.com/product/removed"
                        && raw_input_sha256.len() == 32
                        && expected_raw_input_sha256.is_none()
                },
            )
            .returning(|_, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

        let raw_capture: Arc<dyn ProductListingRawCaptureService> = Arc::new(raw_capture);
        let scraper_candidates: Arc<dyn ScraperCandidateService> = Arc::new(scraper_candidates);
        flush_batch(
            &raw_capture,
            &scraper_candidates,
            vec![QueuedRawCapture {
                request,
                listing_source_domain: "example.com".to_owned(),
                enqueued_at: tokio::time::Instant::now(),
            }],
            0,
        )
        .await;
    }

    /// A sold page used to be captured again to enter `DORMANT_SOLD`; every capture is
    /// now recorded `ACTIVE`, so an unchanged sold observation only refreshes metadata.
    #[tokio::test]
    async fn should_record_an_unchanged_sold_observation_active_without_raw_capture() {
        let url = url::Url::parse("https://example.com/product/sold").unwrap();
        let mut candidate = scraper_candidate("ListingSource", url.clone());
        candidate.last_captured_raw_input_sha256 = Some(vec![3; 32]);

        let raw_input = crawler_verified_removal_input(&url)
            .unwrap_or_else(|error| panic!("test raw input: {error}"));
        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .once()
            .return_once(move |request| {
                assert_eq!(
                    request.expected_last_captured_raw_input_sha256.as_deref(),
                    Some(&[3; 32][..])
                );
                Box::pin(async move {
                    scrape_outcome(Ok(Some(ScrapedProduct {
                        acquired_at: time::OffsetDateTime::now_utc(),
                        raw_input,
                        hash: "sold-hash".to_owned(),
                        schema_fingerprint: "sold-schema".to_owned(),
                        raw_input_sha256: vec![3; 32],
                        effective_url: url::Url::parse("https://example.com/product/sold").unwrap(),
                        source_listing_id: "ListingSource".to_owned(),
                    })))
                })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_as_scraped()
            .once()
            .withf(|completion| {
                completion.disposition == CrawlerDisposition::Active
                    && completion
                        .expected_last_captured_raw_input_sha256
                        .as_deref()
                        == Some(&[3_u8; 32][..])
            })
            .returning(|_| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));
        let ctx = scrape_candidate_context(scraper_candidates, scraper_service);

        let outcome = scrape_candidate(candidate, &ctx).await;

        assert!(outcome.capture.is_none());
        assert!(!outcome.errored);
        assert!(!outcome.skipped);
    }

    #[tokio::test]
    async fn should_capture_redirected_observation_on_the_original_raw_stream() {
        let acquired_at = time::OffsetDateTime::now_utc() - time::Duration::hours(1);
        let original_url = url::Url::parse("https://example.com/product/old").unwrap();
        let effective_url = url::Url::parse("https://example.com/product/new").unwrap();
        let candidate = scraper_candidate("ListingSource", original_url.clone());
        let raw_input = crawler_verified_removal_input(&effective_url)
            .unwrap_or_else(|error| panic!("test raw input: {error}"));

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_redirect_destination_is_available()
            .once()
            .withf({
                let effective_url = effective_url.clone();
                move |url| *url == effective_url
            })
            .returning(|_| Box::pin(async { Ok(true) }));
        scraper_candidates.expect_mark_as_scraped().never();
        let ctx = scrape_candidate_context(scraper_candidates, MockScraperService::new());

        let outcome = handle_successful_scrape(
            &candidate,
            &ctx,
            ScrapedProduct {
                acquired_at,
                raw_input,
                effective_url: effective_url.clone(),
                source_listing_id: "SKU-123".to_owned(),
                hash: "redirect-hash".to_owned(),
                schema_fingerprint: "redirect-schema".to_owned(),
                raw_input_sha256: vec![9; 32],
            },
        )
        .await;

        let request = outcome.capture.expect("redirect must queue raw capture");
        assert_eq!(Some(acquired_at), request.item.command.source_occurred_at);
        assert_eq!(
            request.item.command.source_record_key,
            original_url.as_str(),
            "the raw stream remains the pre-redirect crawler URL"
        );
        assert!(
            request
                .item
                .command
                .input
                .source_payload()
                .value()
                .to_string()
                .contains(effective_url.as_str()),
            "the captured raw input must retain the effective redirect URL"
        );
    }

    #[tokio::test]
    async fn should_keep_removed_url_active_when_raw_capture_fails() {
        let url = url::Url::parse("https://example.com/product/removed").unwrap();
        let mut candidate = scraper_candidate("ListingSource", url);
        let expected_last_captured_raw_input_sha256 = vec![5; 32];
        candidate.last_captured_raw_input_sha256 =
            Some(expected_last_captured_raw_input_sha256.clone());
        let outcome = handle_verified_removal(&candidate);
        let request = outcome.capture.expect("removal must be queued for capture");

        let mut raw_capture = MockProductListingRawCaptureService::new();
        raw_capture.expect_capture().once().returning(|_| {
            Box::pin(async { vec![ProductListingRawCaptureOutcome::RetryableFailure] })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_scraper_failure()
            .once()
            .withf(move |_, _, kind, message, received_raw_input_sha256| {
                kind == "RawCaptureFailed"
                    && message == "verified removal raw capture did not commit"
                    && *received_raw_input_sha256
                        == Some(expected_last_captured_raw_input_sha256.as_slice())
            })
            .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));
        scraper_candidates.expect_mark_removed().never();

        let raw_capture: Arc<dyn ProductListingRawCaptureService> = Arc::new(raw_capture);
        let scraper_candidates: Arc<dyn ScraperCandidateService> = Arc::new(scraper_candidates);
        flush_batch(
            &raw_capture,
            &scraper_candidates,
            vec![QueuedRawCapture {
                request,
                listing_source_domain: "example.com".to_owned(),
                enqueued_at: tokio::time::Instant::now(),
            }],
            0,
        )
        .await;
    }

    #[tokio::test]
    async fn should_mark_same_domain_500_fetch_failure() {
        let url = url::Url::parse("https://same-domain.com/product/1").unwrap();
        let mut candidate = scraper_candidate("ListingSource", url.clone());
        let expected_last_captured_raw_input_sha256 = vec![6; 32];
        candidate.last_captured_raw_input_sha256 =
            Some(expected_last_captured_raw_input_sha256.clone());
        let listing_source_id = candidate.listing_source_id;

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(
                move |received_listing_source_id,
                      received_url,
                      _,
                      _,
                      status_code,
                      _,
                      received_raw_input_sha256| {
                    *received_listing_source_id == listing_source_id
                        && received_url == &url
                        && *status_code == Some(500)
                        && *received_raw_input_sha256
                            == Some(expected_last_captured_raw_input_sha256.as_slice())
                },
            )
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().once().returning(|request| {
            let url = request.url;
            Box::pin(async move {
                scrape_outcome(Err(ScraperError::HttpError {
                    url,
                    kind: NetworkErrorKind::HttpStatus(500),
                    details: "internal server error".to_string(),
                }))
            })
        });

        let ctx = scrape_candidate_context(scraper_candidates, scraper_service);

        let outcome = scrape_candidate(candidate, &ctx).await;

        assert!(outcome.errored);
    }

    #[tokio::test]
    async fn should_record_a_robots_disallowed_url_without_opening_the_domain_circuit() {
        let before = time::OffsetDateTime::now_utc();
        let url = url::Url::parse("https://same-domain.com/product/1").unwrap();
        let candidate = scraper_candidate("ListingSource", url.clone());

        // The mock has no domain-circuit expectation: opening one would panic.
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(
                move |_, received_url, error_kind, _, status_code, next_retry_at, _| {
                    received_url == &url
                        && error_kind == "RobotsDisallowed"
                        && status_code.is_none()
                        && *next_retry_at >= before + time::Duration::hours(24)
                },
            )
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().once().returning(|request| {
            let url = request.url;
            Box::pin(async move {
                scrape_outcome(Err(ScraperError::HttpError {
                    url,
                    kind: NetworkErrorKind::RobotsDisallowed,
                    details: "robots.txt disallows".to_string(),
                }))
            })
        });

        let ctx = scrape_candidate_context(scraper_candidates, scraper_service);

        let outcome = scrape_candidate(candidate, &ctx).await;

        assert!(outcome.errored);
        // Nothing reached the shop: the refusal must neither open nor close
        // the domain circuit.
        assert!(matches!(
            outcome.domain_health,
            DomainFetchHealth::NotObserved
        ));
    }

    #[tokio::test]
    async fn should_mark_same_domain_429_fetch_failure() {
        let url = url::Url::parse("https://same-domain.com/product/1").unwrap();
        let candidate = scraper_candidate("ListingSource", url.clone());
        let listing_source_id = candidate.listing_source_id;

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_fetch_failure_and_open_domain_circuit()
            .once()
            .withf(move |request| {
                request.url_failure.listing_source_id == listing_source_id
                    && request.url_failure.url == url
                    && request.url_failure.error_kind == "HTTP_429"
                    && request.url_failure.status_code == Some(429)
                    && request
                        .url_failure
                        .expected_last_captured_raw_input_sha256
                        .is_none()
                    && request.expected_domain_health.scrape_failure_streak == 0
                    && request.domain_error_kind
                        == crate::network::policy::DomainFailureKind::Http429
                    && request.domain_status_code == Some(429)
            })
            .returning(|_| Box::pin(async { Ok(applied_domain_circuit_open_outcome()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().once().returning(|request| {
            let url = request.url;
            Box::pin(async move {
                scrape_outcome(Err(ScraperError::HttpError {
                    url,
                    kind: NetworkErrorKind::HttpStatus(429),
                    details: "too many requests".to_string(),
                }))
            })
        });

        let ctx = scrape_candidate_context(scraper_candidates, scraper_service);

        let outcome = scrape_candidate(candidate, &ctx).await;

        assert!(outcome.errored);
    }

    #[tokio::test]
    async fn should_continue_same_domain_after_500_failure() {
        let first_url = url::Url::parse("https://same-domain.com/product/1").unwrap();
        let second_url = url::Url::parse("https://same-domain.com/product/2").unwrap();
        let first_candidate_url = first_url.clone();
        let second_candidate_url = second_url.clone();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(move || {
                let first_candidate_url = first_candidate_url.clone();
                let second_candidate_url = second_candidate_url.clone();
                vec![
                    scraper_candidate("ListingSource", first_candidate_url),
                    scraper_candidate("ListingSource", second_candidate_url),
                ]
            }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(
                move |_, received_url, _, _, status_code, _, expected_raw_input_sha256| {
                    received_url == &first_url
                        && *status_code == Some(500)
                        && expected_raw_input_sha256.is_none()
                },
            )
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });
        scraper_candidates
            .expect_mark_as_scraped()
            .once()
            .returning(|_| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

        let scrape_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let scrape_count_for_mock = Arc::clone(&scrape_count);
        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(2)
            .returning(move |request| {
                let url = request.url;
                let attempt = scrape_count_for_mock.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    if attempt == 0 {
                        scrape_outcome(Err(ScraperError::HttpError {
                            url,
                            kind: crate::network::policy::NetworkErrorKind::HttpStatus(500),
                            details: "internal server error".to_string(),
                        }))
                    } else {
                        let raw_input = crawler_verified_removal_input(&url)
                            .unwrap_or_else(|error| panic!("test raw input: {error}"));
                        scrape_outcome(Ok(Some(ScrapedProduct {
                            acquired_at: time::OffsetDateTime::now_utc(),
                            raw_input,
                            hash: "success-hash".to_owned(),
                            schema_fingerprint: "success-schema".to_owned(),
                            raw_input_sha256: vec![2; 32],
                            effective_url: url.clone(),
                            source_listing_id: "ListingSource".to_owned(),
                        })))
                    }
                })
            });

        let mut raw_capture = MockProductListingRawCaptureService::new();
        raw_capture
            .expect_capture()
            .once()
            .returning(|observations| {
                Box::pin(async move {
                    vec![ProductListingRawCaptureOutcome::Persisted; observations.len()]
                })
            });
        let (spider_candidates, spider_service) = empty_spider_dependencies();
        expect_due_url_count(&mut scraper_candidates, 0);
        let job = CrawlerCronJob::new(
            CrawlerCronConfig {
                scraper_domain_delay: Duration::ZERO,
                ..CrawlerCronConfig::default()
            },
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            Box::new(raw_capture),
        );

        job.run_scraper_once().await;

        assert_eq!(scrape_count.load(Ordering::SeqCst), 2);
        assert_eq!(job.scraper_perf.snapshot().0, 1);
    }

    #[tokio::test]
    async fn should_open_a_circuit_after_three_distinct_server_failures() {
        let urls = (1..=4)
            .map(|index| {
                url::Url::parse(&format!("https://same-domain.com/product/{index}"))
                    .expect("test URL")
            })
            .collect::<Vec<_>>();
        let candidate_urls = urls.clone();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(move || {
                candidate_urls
                    .iter()
                    .cloned()
                    .map(|url| scraper_candidate("ListingSource", url))
                    .collect()
            }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .times(2)
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });
        scraper_candidates
            .expect_mark_fetch_failure_and_open_domain_circuit()
            .once()
            .withf(|request| {
                request.url_failure.error_kind == "HTTP_500"
                    && request.url_failure.status_code == Some(500)
                    && request.domain_error_kind
                        == crate::network::policy::DomainFailureKind::Http5xxBurst
            })
            .returning(|_| Box::pin(async { Ok(applied_domain_circuit_open_outcome()) }));

        let scrape_count = Arc::new(AtomicUsize::new(0));
        let scrape_count_for_mock = Arc::clone(&scrape_count);
        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(3)
            .returning(move |request| {
                let url = request.url;
                scrape_count_for_mock.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    scrape_outcome(Err(ScraperError::HttpError {
                        url,
                        kind: NetworkErrorKind::HttpStatus(500),
                        details: "internal server error".to_owned(),
                    }))
                })
            });

        let job = scraper_job(
            CrawlerCronConfig {
                scraper_domain_delay: Duration::ZERO,
                ..CrawlerCronConfig::default()
            },
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;

        assert_eq!(scrape_count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn should_count_distinct_5xx_urls_across_responsive_results() {
        let urls = (1..=4)
            .map(|index| url::Url::parse(&format!("https://burst.example.com/product/{index}")))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let domain_id = scraper_candidate("ListingSource", urls[0].clone()).domain_id;

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_fetch_failure()
            .times(2)
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });
        scraper_candidates
            .expect_mark_fetch_failure_and_open_domain_circuit()
            .once()
            .withf(|request| {
                request.url_failure.url.as_str().ends_with("/4")
                    && request.url_failure.error_kind == "HTTP_500"
                    && request.url_failure.status_code == Some(500)
                    && request.domain_error_kind
                        == crate::network::policy::DomainFailureKind::Http5xxBurst
            })
            .returning(|_| Box::pin(async { Ok(applied_domain_circuit_open_outcome()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(4)
            .returning(move |request| {
                let url = request.url;
                let status = url
                    .path_segments()
                    .and_then(|mut segments| segments.next_back())
                    .unwrap()
                    .parse::<u16>()
                    .unwrap();
                Box::pin(async move {
                    match status {
                        2 => scrape_outcome(Ok(None)),
                        3 => scrape_outcome(Err(ScraperError::HttpError {
                            url,
                            kind: NetworkErrorKind::HttpStatus(502),
                            details: "bad gateway".to_owned(),
                        })),
                        _ => scrape_outcome(Err(ScraperError::HttpError {
                            url,
                            kind: NetworkErrorKind::HttpStatus(500),
                            details: "internal server error".to_owned(),
                        })),
                    }
                })
            });

        let (command_tx, _command_rx) = mpsc::channel(1);
        let outcome = scrape_domain_candidates(
            urls.into_iter()
                .map(|url| scraper_candidate("ListingSource", url))
                .collect(),
            ScrapeDomainContext {
                scraper: Arc::new(scraper_service),
                scraper_candidates: Arc::new(scraper_candidates),
                lock_manager: Arc::new(LocalLockManager::new()),
                command_tx,
                budget_exhausted_listing_sources: Arc::new(Mutex::new(HashSet::new())),
                domain_id,
                batch_time_budget: CrawlerCronConfig::default().scraper_domain_batch_time_budget,
                stop: CronStop::default(),
            },
        )
        .await;

        assert_eq!(outcome.failed, 3);
        assert_eq!(outcome.succeeded, 0);
        assert_eq!(outcome.skipped, 1);
        assert_eq!(4, outcome.succeeded + outcome.failed + outcome.skipped);
    }

    #[tokio::test]
    async fn should_not_count_repeated_5xx_failures_for_the_same_url_twice() {
        let repeated_url =
            url::Url::parse("https://burst-duplicates.example.com/product/1").unwrap();
        let other_url = url::Url::parse("https://burst-duplicates.example.com/product/2").unwrap();
        let domain_id = scraper_candidate("ListingSource", repeated_url.clone()).domain_id;

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_fetch_failure()
            .times(3)
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

        let mut scraper_service = MockScraperService::new();
        let other_url_for_mock = other_url.clone();
        scraper_service
            .expect_scrape()
            .times(3)
            .returning(move |request| {
                let status = if request.url == other_url_for_mock {
                    502
                } else {
                    500
                };
                let url = request.url;
                Box::pin(async move {
                    scrape_outcome(Err(ScraperError::HttpError {
                        url,
                        kind: NetworkErrorKind::HttpStatus(status),
                        details: "server error".to_owned(),
                    }))
                })
            });

        let (command_tx, _command_rx) = mpsc::channel(1);
        let outcome = scrape_domain_candidates(
            vec![
                scraper_candidate("ListingSource", repeated_url.clone()),
                scraper_candidate("ListingSource", repeated_url),
                scraper_candidate("ListingSource", other_url),
            ],
            ScrapeDomainContext {
                scraper: Arc::new(scraper_service),
                scraper_candidates: Arc::new(scraper_candidates),
                lock_manager: Arc::new(LocalLockManager::new()),
                command_tx,
                budget_exhausted_listing_sources: Arc::new(Mutex::new(HashSet::new())),
                domain_id,
                batch_time_budget: CrawlerCronConfig::default().scraper_domain_batch_time_budget,
                stop: CronStop::default(),
            },
        )
        .await;

        assert_eq!(outcome.failed, 3);
        assert_eq!(outcome.skipped, 0);
    }

    /// Distinct URLs answering the same crawler-rejection status in a row
    /// open the domain circuit and skip the rest of the batch; a responsive
    /// outcome between two such answers starts the count again.
    #[tokio::test]
    async fn should_open_a_status_burst_after_three_distinct_urls_answer_405_in_a_row() {
        let urls = (1..=6)
            .map(|index| url::Url::parse(&format!("https://rejects.example.com/product/{index}")))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let domain_id = scraper_candidate("ListingSource", urls[0].clone()).domain_id;

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_fetch_failure()
            .times(3)
            .withf(|_, _, error_kind, _, status_code, _, _| {
                error_kind == "HTTP_405" && *status_code == Some(405)
            })
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });
        scraper_candidates
            .expect_mark_fetch_failure_and_open_domain_circuit()
            .once()
            .withf(|request| {
                request.url_failure.url.as_str().ends_with("/5")
                    && request.url_failure.status_code == Some(405)
                    && request.domain_error_kind == DomainFailureKind::HttpStatusBurst
                    && request.domain_status_code == Some(405)
                    && request.next_scrape_at.is_some()
            })
            .returning(|_| Box::pin(async { Ok(applied_domain_circuit_open_outcome()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(5)
            .returning(move |request| {
                let url = request.url;
                let responsive = url.path().ends_with("/2");
                Box::pin(async move {
                    if responsive {
                        scrape_outcome(Ok(None))
                    } else {
                        scrape_outcome(Err(ScraperError::HttpError {
                            url,
                            kind: NetworkErrorKind::HttpStatus(405),
                            details: "method not allowed".to_owned(),
                        }))
                    }
                })
            });

        let (command_tx, _command_rx) = mpsc::channel(1);
        let outcome = scrape_domain_candidates(
            urls.into_iter()
                .map(|url| scraper_candidate("ListingSource", url))
                .collect(),
            ScrapeDomainContext {
                scraper: Arc::new(scraper_service),
                scraper_candidates: Arc::new(scraper_candidates),
                lock_manager: Arc::new(LocalLockManager::new()),
                command_tx,
                budget_exhausted_listing_sources: Arc::new(Mutex::new(HashSet::new())),
                domain_id,
                batch_time_budget: CrawlerCronConfig::default().scraper_domain_batch_time_budget,
                stop: CronStop::default(),
            },
        )
        .await;

        assert_eq!(outcome.failed, 4);
        assert_eq!(
            outcome.skipped, 2,
            "the responsive URL and the one after the burst"
        );
        assert_eq!(outcome.deferred_urls, 0);
    }

    #[tokio::test]
    async fn should_count_remaining_candidates_as_skipped_after_domain_circuit_stop() {
        let urls = (1..=3)
            .map(|index| url::Url::parse(&format!("https://stop.example.com/product/{index}")))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let domain_id = scraper_candidate("ListingSource", urls[0].clone()).domain_id;

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_fetch_failure_and_open_domain_circuit()
            .once()
            .returning(|_| Box::pin(async { Ok(applied_domain_circuit_open_outcome()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().once().returning(|request| {
            let url = request.url;
            Box::pin(async move {
                scrape_outcome(Err(ScraperError::HttpError {
                    url,
                    kind: NetworkErrorKind::HttpStatus(429),
                    details: "too many requests".to_owned(),
                }))
            })
        });

        let (command_tx, _command_rx) = mpsc::channel(1);
        let outcome = scrape_domain_candidates(
            urls.into_iter()
                .map(|url| scraper_candidate("ListingSource", url))
                .collect(),
            ScrapeDomainContext {
                scraper: Arc::new(scraper_service),
                scraper_candidates: Arc::new(scraper_candidates),
                lock_manager: Arc::new(LocalLockManager::new()),
                command_tx,
                budget_exhausted_listing_sources: Arc::new(Mutex::new(HashSet::new())),
                domain_id,
                batch_time_budget: CrawlerCronConfig::default().scraper_domain_batch_time_budget,
                stop: CronStop::default(),
            },
        )
        .await;

        assert_eq!(outcome.failed, 1);
        assert_eq!(outcome.succeeded, 0);
        assert_eq!(outcome.skipped, 2);
        assert_eq!(3, outcome.succeeded + outcome.failed + outcome.skipped);
    }

    fn domain_batch(host: &str, count: usize) -> (Vec<ScraperCandidate>, crate::CrawlerDomainId) {
        let candidates: Vec<ScraperCandidate> = (1..=count)
            .map(|index| {
                let url = url::Url::parse(&format!("https://{host}/product/{index}")).unwrap();
                scraper_candidate("ListingSource", url)
            })
            .collect();
        let domain_id = candidates[0].domain_id;
        (candidates, domain_id)
    }

    fn domain_context(
        scraper_candidates: MockScraperCandidateService,
        scraper_service: MockScraperService,
        domain_id: crate::CrawlerDomainId,
        batch_time_budget: Duration,
    ) -> ScrapeDomainContext {
        let (command_tx, _command_rx) = mpsc::channel(1);
        ScrapeDomainContext {
            scraper: Arc::new(scraper_service),
            scraper_candidates: Arc::new(scraper_candidates),
            lock_manager: Arc::new(LocalLockManager::new()),
            command_tx,
            budget_exhausted_listing_sources: Arc::new(Mutex::new(HashSet::new())),
            domain_id,
            batch_time_budget,
            stop: CronStop::default(),
        }
    }

    #[tokio::test]
    async fn should_defer_the_rest_of_the_batch_after_robots_txt_is_unreachable() {
        let (candidates, domain_id) = domain_batch("unreachable.example.com", 3);
        // One failure row for the refused URL, none for the deferred rest.
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(|_, _, error_kind, _, _, _, _| error_kind == "RobotsUnreachable")
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });
        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().once().returning(|request| {
            let url = request.url;
            Box::pin(async move {
                scrape_outcome(Err(ScraperError::HttpError {
                    url,
                    kind: NetworkErrorKind::RobotsUnreachable,
                    details: "robots.txt is unreachable".to_owned(),
                }))
            })
        });

        let outcome = scrape_domain_candidates(
            candidates,
            domain_context(
                scraper_candidates,
                scraper_service,
                domain_id,
                CrawlerCronConfig::default().scraper_domain_batch_time_budget,
            ),
        )
        .await;

        assert_eq!(outcome.failed, 1);
        assert_eq!(outcome.deferred_urls, 2);
    }

    #[tokio::test]
    async fn should_defer_urls_left_when_the_domain_batch_time_budget_runs_out() {
        let (candidates, domain_id) = domain_batch("slow.example.com", 3);
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_as_scraped()
            .returning(|_| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));
        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .once()
            .returning(|_| Box::pin(async { scrape_outcome(Ok(None)) }));

        let outcome = scrape_domain_candidates(
            candidates,
            domain_context(
                scraper_candidates,
                scraper_service,
                domain_id,
                Duration::ZERO,
            ),
        )
        .await;

        assert_eq!(outcome.skipped, 1);
        assert_eq!(outcome.deferred_urls, 2);
    }

    #[tokio::test]
    async fn should_defer_urls_left_in_the_domain_batch_once_stopped() {
        let (candidates, domain_id) = domain_batch("stopping.example.com", 3);
        let stop = CronStop::default();
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_as_scraped()
            .returning(|_| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));
        let mut scraper_service = MockScraperService::new();
        let stop_during_scrape = stop.clone();
        scraper_service.expect_scrape().once().returning(move |_| {
            stop_during_scrape.request();
            Box::pin(async { scrape_outcome(Ok(None)) })
        });
        let mut context = domain_context(
            scraper_candidates,
            scraper_service,
            domain_id,
            CrawlerCronConfig::default().scraper_domain_batch_time_budget,
        );
        context.stop = stop;

        let outcome = scrape_domain_candidates(candidates, context).await;

        assert_eq!(outcome.skipped, 1);
        assert_eq!(outcome.deferred_urls, 2);
    }

    #[tokio::test]
    async fn should_continue_same_domain_after_retryable_network_failure() {
        let first_url = url::Url::parse("https://same-domain.com/product/1").unwrap();
        let second_url = url::Url::parse("https://same-domain.com/product/2").unwrap();
        let first_candidate_url = first_url.clone();
        let second_candidate_url = second_url.clone();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(move || {
                let first_candidate_url = first_candidate_url.clone();
                let second_candidate_url = second_candidate_url.clone();
                vec![
                    scraper_candidate("ListingSource", first_candidate_url),
                    scraper_candidate("ListingSource", second_candidate_url),
                ]
            }));
        scraper_candidates
            .expect_mark_fetch_failure_and_open_domain_circuit()
            .once()
            .withf(move |request| {
                request.url_failure.url == first_url
                    && request.url_failure.error_kind == "HTTP_429"
                    && request.url_failure.status_code == Some(429)
                    && request
                        .url_failure
                        .expected_last_captured_raw_input_sha256
                        .is_none()
                    && request.domain_error_kind
                        == crate::network::policy::DomainFailureKind::Http429
                    && request.domain_status_code == Some(429)
            })
            .returning(|_| Box::pin(async { Ok(applied_domain_circuit_open_outcome()) }));

        let scrape_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let scrape_count_for_mock = Arc::clone(&scrape_count);
        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(1)
            .returning(move |request| {
                let url = request.url;
                let attempt = scrape_count_for_mock.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    if attempt == 0 {
                        scrape_outcome(Err(ScraperError::HttpError {
                            url,
                            kind: crate::network::policy::NetworkErrorKind::HttpStatus(429),
                            details: "too many requests".to_string(),
                        }))
                    } else {
                        scrape_outcome(Ok(None))
                    }
                })
            });

        let job = scraper_job(
            CrawlerCronConfig {
                scraper_domain_delay: Duration::from_millis(1),
                ..CrawlerCronConfig::default()
            },
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
        assert_eq!(scrape_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn should_mark_fetch_failure_for_llm_budget_exceeded_error() {
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(|| {
                vec![scraper_candidate(
                    "Test ListingSource",
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )]
            }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().returning(|request| {
            let url = request.url;
            let listing_source_id = request.listing_source_id;
            Box::pin(async move {
                scrape_outcome(Err(ScraperError::LlmBudgetExceeded {
                    listing_source_id,
                    url,
                    max_calls: 5,
                }))
            })
        });

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_scrape_remaining_listing_source_candidates_when_schema_review_is_pending() {
        let listing_source_id = ListingSourceId::new();
        let first_url = url::Url::parse("https://example.com/product/1").unwrap();
        let second_url = url::Url::parse("https://example.com/product/2").unwrap();

        let first_url_for_candidates = first_url.clone();
        let second_url_for_candidates = second_url.clone();
        let waiting_url = first_url.clone();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(move || {
                let mut first =
                    scraper_candidate("Test ListingSource", first_url_for_candidates.clone());
                first.listing_source_id = listing_source_id;
                let mut second =
                    scraper_candidate("Test ListingSource", second_url_for_candidates.clone());
                second.listing_source_id = listing_source_id;
                vec![first, second]
            }));
        scraper_candidates
            .expect_mark_waiting_on_schema_review()
            .once()
            .withf(move |request| {
                request.url_failure.listing_source_id == listing_source_id
                    && request.url_failure.url == first_url
                    && request.url_failure.error_kind == "PendingSchemaReview"
                    && request
                        .url_failure
                        .expected_last_captured_raw_input_sha256
                        .is_none()
            })
            .returning(|_| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));
        scraper_candidates.expect_mark_fetch_failure().never();

        // The first page waits on the pending review; a cached schema still extracts the second.
        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(2)
            .returning(move |request| {
                let url = request.url;
                let waits = url == waiting_url;
                Box::pin(async move {
                    if waits {
                        scrape_outcome(Err(ScraperError::PendingSchemaReview {
                            url,
                            review_id: crate::CrawlerReviewId::new(),
                        }))
                    } else {
                        scrape_outcome(Ok(None))
                    }
                })
            });

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }

    mod schema_review_wait_backoff {
        use super::*;

        const BASE: Duration = SCHEMA_ERROR_RETRY_COOLDOWN;

        #[test]
        fn should_double_the_retry_delay_for_each_consecutive_wait() {
            assert_eq!(schema_review_wait_retry_delay(1), BASE);
            assert_eq!(schema_review_wait_retry_delay(2), BASE * 2);
            assert_eq!(schema_review_wait_retry_delay(3), BASE * 4);
        }

        #[test]
        fn should_cap_the_retry_delay_however_long_the_review_stays_pending() {
            assert_eq!(
                schema_review_wait_retry_delay(4),
                SCHEMA_REVIEW_WAIT_MAX_RETRY_DELAY
            );
            assert_eq!(
                schema_review_wait_retry_delay(40),
                SCHEMA_REVIEW_WAIT_MAX_RETRY_DELAY
            );
            assert_eq!(
                schema_review_wait_retry_delay(u32::MAX),
                SCHEMA_REVIEW_WAIT_MAX_RETRY_DELAY
            );
        }

        #[test]
        fn should_continue_the_streak_only_on_the_same_review() {
            let review_id = CrawlerReviewId::new();
            let first = SchemaReviewWait::after(None, review_id);
            let second = SchemaReviewWait::after(Some(first), review_id);
            let other_review = SchemaReviewWait::after(Some(second), CrawlerReviewId::new());

            assert_eq!(first.streak, 1);
            assert_eq!(second.streak, 2);
            assert_eq!(other_review.streak, 1);
        }

        /// Scrapes one candidate that waits on `review_id` and returns the persisted wait
        /// request.
        async fn persisted_wait(
            previous: Option<SchemaReviewWait>,
            review_id: CrawlerReviewId,
        ) -> SchemaReviewWaitRequest {
            let url = url::Url::parse("https://example.com/product/waiting").unwrap();
            let mut candidate = scraper_candidate("Test ListingSource", url);
            candidate.schema_review_wait = previous;
            let recorded = Arc::new(std::sync::Mutex::new(None));

            let mut scraper_candidates = MockScraperCandidateService::new();
            scraper_candidates
                .expect_mark_waiting_on_schema_review()
                .once()
                .returning({
                    let recorded = recorded.clone();
                    move |request| {
                        *recorded.lock().unwrap() = Some(request);
                        Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
                    }
                });
            scraper_candidates.expect_mark_fetch_failure().never();
            let mut scraper_service = MockScraperService::new();
            scraper_service
                .expect_scrape()
                .once()
                .returning(move |request| {
                    let url = request.url;
                    Box::pin(async move {
                        scrape_outcome(Err(ScraperError::PendingSchemaReview { url, review_id }))
                    })
                });

            let mut ctx = scrape_candidate_context(scraper_candidates, scraper_service);
            ctx.domain_id = candidate.domain_id;
            scrape_domain_candidates(vec![candidate], ctx).await;

            recorded.lock().unwrap().take().expect("wait was persisted")
        }

        fn assert_retry_after(
            request: &SchemaReviewWaitRequest,
            before: time::OffsetDateTime,
            delay: Duration,
        ) {
            let delay = time::Duration::seconds(delay.as_secs() as i64);
            let next_retry_at = request.url_failure.next_retry_at;
            assert!(
                next_retry_at >= before + delay,
                "{next_retry_at} is earlier than {delay}"
            );
            assert!(next_retry_at <= time::OffsetDateTime::now_utc() + delay);
        }

        #[tokio::test]
        async fn should_grow_the_retry_delay_when_the_page_waits_on_the_same_review_again() {
            let review_id = CrawlerReviewId::new();
            let before = time::OffsetDateTime::now_utc();

            let request = persisted_wait(
                Some(SchemaReviewWait {
                    review_id,
                    streak: 2,
                }),
                review_id,
            )
            .await;

            assert_eq!(
                request.wait,
                SchemaReviewWait {
                    review_id,
                    streak: 3
                }
            );
            assert_eq!(request.url_failure.error_kind, "PendingSchemaReview");
            assert_retry_after(&request, before, BASE * 4);
        }

        #[tokio::test]
        async fn should_restart_at_the_schema_cooldown_when_the_page_waits_on_a_new_review() {
            let review_id = CrawlerReviewId::new();
            let before = time::OffsetDateTime::now_utc();

            let request = persisted_wait(
                Some(SchemaReviewWait {
                    review_id: CrawlerReviewId::new(),
                    streak: 5,
                }),
                review_id,
            )
            .await;

            assert_eq!(
                request.wait,
                SchemaReviewWait {
                    review_id,
                    streak: 1
                }
            );
            assert_retry_after(&request, before, BASE);
        }

        #[tokio::test]
        async fn should_cap_the_retry_delay_when_the_review_stays_pending() {
            let review_id = CrawlerReviewId::new();
            let before = time::OffsetDateTime::now_utc();

            let request = persisted_wait(
                Some(SchemaReviewWait {
                    review_id,
                    streak: 12,
                }),
                review_id,
            )
            .await;

            assert_eq!(request.wait.streak, 13);
            assert_retry_after(&request, before, SCHEMA_REVIEW_WAIT_MAX_RETRY_DELAY);
        }
    }

    /// `FreshSchemaImplausible` must be handled identically to
    /// `SchemaRegenerationExhausted`: write a cooldown via `mark_fetch_failure`
    /// so the URL is held back until the backoff window expires.
    #[tokio::test]
    async fn should_mark_fetch_failure_for_implausible_fresh_schema_extraction() {
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(|| {
                vec![scraper_candidate(
                    "Test ListingSource",
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )]
            }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().returning(|request| {
            let url = request.url;
            Box::pin(async move {
                scrape_outcome(Err(ScraperError::FreshSchemaImplausible {
                    url,
                    findings: Default::default(),
                }))
            })
        });

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }

    /// A page that leaves a required value empty is recorded as scraped with its outcome:
    /// no retry cooldown, and no failure that keeps the URL due on every pass.
    #[tokio::test]
    async fn should_mark_scraped_without_capture_for_missing_required_value() {
        let url = url::Url::parse("https://example.com/product/1").unwrap();
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain({
                let url = url.clone();
                move || vec![scraper_candidate("Test ListingSource", url.clone())]
            }));
        scraper_candidates
            .expect_mark_scraped_without_capture()
            .once()
            .withf({
                let url = url.clone();
                move |_, received_url, kind, message, expected_raw_input_sha256| {
                    *received_url == url
                        && kind == "MissingRequiredValue"
                        && message.starts_with(
                            "Required value missing from 'https://example.com/product/1'",
                        )
                        && expected_raw_input_sha256.is_none()
                }
            })
            .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));
        scraper_candidates.expect_mark_fetch_failure().never();
        scraper_candidates.expect_mark_scraper_failure().never();

        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().once().returning(|request| {
            let url = request.url;
            Box::pin(async move {
                scrape_outcome(Err(ScraperError::MissingRequiredValue {
                    url,
                    findings: Default::default(),
                }))
            })
        });

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_mark_fetch_failure_for_schema_classification_rejection() {
        let before = time::OffsetDateTime::now_utc();
        let url = url::Url::parse("https://example.com/product/1").unwrap();
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain({
                let url = url.clone();
                move || vec![scraper_candidate("Test ListingSource", url.clone())]
            }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(
                move |_,
                      received_url,
                      kind,
                      _,
                      status_code,
                      next_retry_at,
                      expected_raw_input_sha256| {
                    let cooldown =
                        time::Duration::seconds(SCHEMA_ERROR_RETRY_COOLDOWN.as_secs() as i64);
                    *received_url == url
                        && kind == "SchemaClassificationRejected"
                        && status_code.is_none()
                        && *next_retry_at >= before + cooldown
                        && *next_retry_at <= time::OffsetDateTime::now_utc() + cooldown
                        && expected_raw_input_sha256.is_none()
                },
            )
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });
        scraper_candidates.expect_mark_scraper_failure().never();

        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().once().returning(|request| {
            let url = request.url;
            Box::pin(async move {
                scrape_outcome(Err(ScraperError::SchemaClassificationRejected {
                    url,
                    details: "removed classification requires HIGH confidence".to_string(),
                }))
            })
        });

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_scrape_candidates_from_multiple_domains() {
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(|| {
                vec![
                    scraper_candidate(
                        "ListingSource A",
                        url::Url::parse("https://domain-a.com/product/1").unwrap(),
                    ),
                    scraper_candidate(
                        "ListingSource B",
                        url::Url::parse("https://domain-b.com/product/2").unwrap(),
                    ),
                ]
            }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(2)
            .returning(|_| Box::pin(async { successful_scrape_outcome() }));

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_refill_scraper_domain_slot_while_slow_domain_is_running() {
        let slow_url = url::Url::parse("https://domain-a.com/product/1").unwrap();
        let fast_url = url::Url::parse("https://domain-b.com/product/1").unwrap();
        let refill_url = url::Url::parse("https://domain-c.com/product/1").unwrap();

        let slow_url_for_candidates = slow_url.clone();
        let fast_url_for_candidates = fast_url.clone();
        let refill_url_for_candidates = refill_url.clone();
        let slow_domain_id = scraper_candidate("Slow", slow_url.clone()).domain_id;
        let fast_domain_id = scraper_candidate("Fast", fast_url.clone()).domain_id;
        let refill_domain_id = scraper_candidate("Refill", refill_url.clone()).domain_id;
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(move |_, _, excluded_domains| {
                let excluded_domains = excluded_domains.to_vec();
                let slow_url = slow_url_for_candidates.clone();
                let fast_url = fast_url_for_candidates.clone();
                let refill_url = refill_url_for_candidates.clone();
                let slow_domain_id = slow_domain_id;
                let fast_domain_id = fast_domain_id;
                let refill_domain_id = refill_domain_id;
                Box::pin(async move {
                    if excluded_domains.is_empty() {
                        Ok(vec![
                            scraper_candidate("Slow", slow_url),
                            scraper_candidate("Fast", fast_url),
                        ])
                    } else if excluded_domains.contains(&slow_domain_id)
                        && excluded_domains.contains(&fast_domain_id)
                        && !excluded_domains.contains(&refill_domain_id)
                    {
                        Ok(vec![scraper_candidate("Refill", refill_url)])
                    } else {
                        Ok(vec![])
                    }
                })
            });

        let slow_running = Arc::new(AtomicBool::new(false));
        let refill_started_while_slow_running = Arc::new(AtomicBool::new(false));
        let release_slow = Arc::new(tokio::sync::Notify::new());
        let release_slow_for_mock = Arc::clone(&release_slow);
        let slow_running_for_mock = Arc::clone(&slow_running);
        let refill_started_for_mock = Arc::clone(&refill_started_while_slow_running);

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(3)
            .returning(move |request| {
                let url = request.url;
                let release_slow = Arc::clone(&release_slow_for_mock);
                let slow_running = Arc::clone(&slow_running_for_mock);
                let refill_started = Arc::clone(&refill_started_for_mock);
                let slow_url = slow_url.clone();
                let fast_url = fast_url.clone();
                let refill_url = refill_url.clone();
                Box::pin(async move {
                    if url == slow_url {
                        slow_running.store(true, Ordering::SeqCst);
                        release_slow.notified().await;
                        slow_running.store(false, Ordering::SeqCst);
                    } else if url == fast_url {
                        while !slow_running.load(Ordering::SeqCst) {
                            tokio::time::sleep(Duration::from_millis(1)).await;
                        }
                    } else if url == refill_url {
                        if slow_running.load(Ordering::SeqCst) {
                            refill_started.store(true, Ordering::SeqCst);
                        }
                        release_slow.notify_one();
                    }
                    scrape_outcome(Ok(None))
                })
            });

        let job = scraper_job(
            CrawlerCronConfig {
                scraper_concurrency: 2,
                scraper_urls_per_domain: 100,
                scraper_domain_delay: Duration::ZERO,
                ..CrawlerCronConfig::default()
            },
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;

        assert!(refill_started_while_slow_running.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn should_skip_same_listing_source_candidate_already_scraping_on_another_domain() {
        let listing_source_id = ListingSourceId::new();
        let first_url = url::Url::parse("https://domain-a.com/product/1").unwrap();
        let second_url = url::Url::parse("https://domain-b.com/product/2").unwrap();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(move || {
                let mut first = scraper_candidate("Same ListingSource", first_url.clone());
                first.listing_source_id = listing_source_id;
                let mut second = scraper_candidate("Same ListingSource", second_url.clone());
                second.listing_source_id = listing_source_id;
                vec![first, second]
            }));

        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().once().returning(|_| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                successful_scrape_outcome()
            })
        });

        let job = scraper_job(
            CrawlerCronConfig {
                scraper_concurrency: 2,
                scraper_domain_delay: Duration::ZERO,
                ..CrawlerCronConfig::default()
            },
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_skip_scraper_candidate_when_url_lock_is_already_held() {
        let locked_url = url::Url::parse("https://domain-a.com/product/1").unwrap();
        let open_url = url::Url::parse("https://domain-a.com/product/2").unwrap();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(move || {
                let locked_url = locked_url.clone();
                let open_url = open_url.clone();
                vec![
                    scraper_candidate("ListingSource A", locked_url),
                    scraper_candidate("ListingSource A", open_url),
                ]
            }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(1)
            .returning(|_| Box::pin(async { successful_scrape_outcome() }));

        let lock_manager = Arc::new(LocalLockManager::new());
        let prelocked = url::Url::parse("https://domain-a.com/product/1").unwrap();
        let _prelock = UrlLock::try_acquire(&lock_manager, &prelocked).unwrap();
        let (spider_candidates, spider_service) = empty_spider_dependencies();

        expect_due_url_count(&mut scraper_candidates, 0);
        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::clone(&lock_manager),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            no_raw_capture_service(),
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_scrape_all_urls_from_same_domain() {
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(|| {
                vec![
                    scraper_candidate(
                        "ListingSource",
                        url::Url::parse("https://same-domain.com/product/1").unwrap(),
                    ),
                    scraper_candidate(
                        "ListingSource",
                        url::Url::parse("https://same-domain.com/product/2").unwrap(),
                    ),
                    scraper_candidate(
                        "ListingSource",
                        url::Url::parse("https://same-domain.com/product/3").unwrap(),
                    ),
                ]
            }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(3)
            .returning(|_| Box::pin(async { successful_scrape_outcome() }));

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_not_close_probe_circuit_when_schema_error_precedes_primary_fetch() {
        let url = url::Url::parse("https://probe-local-error.example.com/product/1").unwrap();
        let mut candidate = scraper_candidate("ListingSource", url.clone());
        candidate.is_domain_probe = true;
        candidate.domain_health = DomainHealthSnapshot {
            scrape_failure_streak: 1,
            last_scrape_error_kind: Some("HTTP_429".to_owned()),
            next_scrape_at: Some(time::OffsetDateTime::now_utc() - time::Duration::minutes(1)),
        };

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_scraper_failure()
            .once()
            .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().once().returning(|_| {
            Box::pin(async move {
                ScrapeOutcome {
                    result: Err(ScraperError::SchemaServiceError(
                        ProductListingSchemaServiceError::DatabaseError(sqlx::Error::RowNotFound),
                    )),
                    domain_health: DomainFetchHealth::NotObserved,
                    fetch_failure: None,
                }
            })
        });

        let mut ctx = scrape_candidate_context(scraper_candidates, scraper_service);
        ctx.domain_id = candidate.domain_id;
        let outcome = scrape_domain_candidates(vec![candidate], ctx).await;

        assert_eq!(outcome.failed, 1);
        assert_eq!(outcome.skipped, 0);
    }

    #[tokio::test]
    async fn should_close_probe_circuit_after_responsive_fetch_and_later_pending_review() {
        let url = url::Url::parse("https://probe-responsive.example.com/product/1").unwrap();
        let mut candidate = scraper_candidate("ListingSource", url.clone());
        candidate.is_domain_probe = true;
        candidate.domain_health = DomainHealthSnapshot {
            scrape_failure_streak: 1,
            last_scrape_error_kind: Some("HTTP_429".to_owned()),
            next_scrape_at: Some(time::OffsetDateTime::now_utc() - time::Duration::minutes(1)),
        };
        let expected_domain_id = candidate.domain_id;

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_waiting_on_schema_review()
            .once()
            .returning(|_| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));
        scraper_candidates
            .expect_close_domain_circuit()
            .once()
            .withf(move |domain_id, snapshot| {
                *domain_id == expected_domain_id && snapshot.scrape_failure_streak == 1
            })
            .returning(|_, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().once().returning(|request| {
            let url = request.url.clone();
            Box::pin(async move {
                ScrapeOutcome {
                    result: Err(ScraperError::PendingSchemaReview {
                        url,
                        review_id: crate::CrawlerReviewId::new(),
                    }),
                    domain_health: DomainFetchHealth::Responsive,
                    fetch_failure: None,
                }
            })
        });

        let mut ctx = scrape_candidate_context(scraper_candidates, scraper_service);
        ctx.domain_id = candidate.domain_id;
        let outcome = scrape_domain_candidates(vec![candidate], ctx).await;

        assert_eq!(outcome.failed, 1);
        assert_eq!(outcome.skipped, 0);
    }

    #[tokio::test]
    async fn should_persist_schema_seed_failure_against_seed_url_and_fence() {
        let primary_url =
            url::Url::parse("https://seed-provenance.example.com/product/primary").unwrap();
        let seed_url = url::Url::parse("https://seed-provenance.example.com/product/seed").unwrap();
        let primary_fence = vec![1; 32];
        let seed_fence = vec![2; 32];
        let mut candidate = scraper_candidate("ListingSource", primary_url);
        candidate.last_captured_raw_input_sha256 = Some(primary_fence);
        let expected_domain_id = candidate.domain_id;
        let expected_listing_source_id = candidate.listing_source_id;
        let expected_seed_url = seed_url.clone();
        let expected_seed_fence = seed_fence.clone();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_fetch_failure_and_open_domain_circuit()
            .once()
            .withf(move |request| {
                request.url_failure.listing_source_id == expected_listing_source_id
                    && request.url_failure.url == expected_seed_url
                    && request.url_failure.error_kind == "HTTP_429"
                    && request.url_failure.status_code == Some(429)
                    && request.url_failure.expected_last_captured_raw_input_sha256
                        == Some(expected_seed_fence.clone())
                    && request.domain_id == expected_domain_id
            })
            .returning(|_| Box::pin(async { Ok(applied_domain_circuit_open_outcome()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().once().returning(move |_| {
            let seed_url = seed_url.clone();
            let seed_fence = seed_fence.clone();
            Box::pin(async move {
                record_transport_failure(
                    &seed_url,
                    FetchFailureSource::SchemaSeed,
                    Some(seed_fence.as_slice()),
                    NetworkErrorKind::HttpStatus(429),
                    Some(429),
                    None,
                    None,
                );
                let mut outcome = scrape_outcome(Err(ScraperError::HttpError {
                    url: seed_url,
                    kind: NetworkErrorKind::HttpStatus(429),
                    details: "seed rate limited".to_owned(),
                }));
                outcome.fetch_failure = current_fetch_failure();
                outcome
            })
        });

        let ctx = scrape_candidate_context(scraper_candidates, scraper_service);
        let outcome = scrape_candidate(candidate, &ctx).await;

        assert!(outcome.errored);
        assert!(matches!(
            outcome.domain_health,
            DomainFetchHealth::CircuitFailure {
                kind: crate::network::policy::DomainFailureKind::Http429,
                ..
            }
        ));
    }

    /// A sold page that the active-template schemas no longer extract, as on Hingstons
    /// Antiques, where the add-to-cart button that carried the identity disappears.
    mod sold_page_identity {
        use super::*;
        use crate::scraper::css_selector::product_schema::{
            ListingSourceProductSchema, ProductCssSelectorSchema,
        };
        use crate::scraper::css_selector::product_schema_service::MockProductListingSchemaService;
        use crate::scraper::css_selector::rule::{
            CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule,
        };
        use crate::scraper::scraper_service::html_fetcher::{FetchedHtml, MockHtmlFetcher};
        use crate::scraper::scraper_service::service::ScraperServiceImpl;
        use product_listing_core::listing_availability::ListingAvailability;
        use product_listing_normalization::{
            ListingAvailabilityQuickCheck, ProductListingRawValuesNormalizationOutcome,
            ProductListingRawValuesNormalizer, ProductListingRawValuesPatch,
        };

        const CAPTURED_IDENTITY: &str = "53036";

        fn rule(selector: &str, extract: ExtractionKind) -> ExtractionRule {
            ExtractionRule {
                selector: CssSelector::from(selector),
                additional_selectors: vec![],
                extract,
                cardinality: ExtractionCardinality::First,
            }
        }

        fn attribute(selector: &str, name: &str) -> ExtractionRule {
            rule(selector, ExtractionKind::Attribute { name: name.into() })
        }

        fn schema(identity: ExtractionRule, state_selector: &str) -> ProductCssSelectorSchema {
            ProductCssSelectorSchema {
                source_listing_id: Some(identity),
                title: rule("h1", ExtractionKind::Text),
                description: None,
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                state: rule(state_selector, ExtractionKind::Text),
                images: ExtractionRule {
                    cardinality: ExtractionCardinality::All,
                    ..attribute("div.product img", "src")
                },
                raw_attributes: Default::default(),
            }
        }

        /// The schema the listing was captured with: its identity and its state are the
        /// add-to-cart button, which a sold page no longer renders.
        fn active_template_schema() -> ProductCssSelectorSchema {
            schema(
                attribute("button.single_add_to_cart_button", "value"),
                "button.single_add_to_cart_button",
            )
        }

        fn sold_page() -> String {
            r#"<!DOCTYPE html>
            <html><body>
              <div class="product" id="product-53036" data-product_id="53036">
                <h1>Georgian Mahogany Chair</h1>
                <p class="stock out-of-stock">Sold</p>
                <img src="/images/chair-640x640.jpg">
              </div>
            </body></html>"#
                .to_owned()
        }

        fn sold_candidate() -> ScraperCandidate {
            let mut candidate = scraper_candidate(
                "Hingstons",
                url::Url::parse("https://example.com/product/georgian-chair").unwrap(),
            );
            candidate.last_source_listing_id = Some(CAPTURED_IDENTITY.to_owned());
            candidate
        }

        /// The ListingSource's cached schemas are the active-template schema and
        /// `sold_template`, and every fetch returns the sold page.
        fn context(
            candidate: &ScraperCandidate,
            sold_template: ProductCssSelectorSchema,
            scraper_candidates: MockScraperCandidateService,
        ) -> ScrapeDomainContext {
            let url = candidate.url.clone();
            let mut fetcher = MockHtmlFetcher::new();
            fetcher.expect_fetch().returning(move |_| {
                let url = url.clone();
                Box::pin(
                    async move { Ok(FetchedHtml::new(sold_page(), url, reqwest::StatusCode::OK)) },
                )
            });
            let cached = ListingSourceProductSchema {
                listing_source_id: candidate.listing_source_id,
                product_schemas: vec![active_template_schema(), sold_template],
                created: time::OffsetDateTime::now_utc(),
                updated: time::OffsetDateTime::now_utc(),
            };
            let mut schema_service = MockProductListingSchemaService::new();
            schema_service
                .expect_find_product_schema()
                .returning(move |_| {
                    let cached = cached.clone();
                    Box::pin(async move { Ok(Some(cached)) })
                });
            let scraper_candidates = Arc::new(scraper_candidates);
            let scraper = ScraperServiceImpl::new(
                Box::new(fetcher),
                Box::new(schema_service),
                scraper_candidates.clone(),
            );
            let (command_tx, _command_rx) = tokio::sync::mpsc::channel(1);
            ScrapeDomainContext {
                scraper: Arc::new(scraper),
                scraper_candidates,
                lock_manager: Arc::new(LocalLockManager::new()),
                command_tx,
                budget_exhausted_listing_sources: Arc::new(Mutex::new(HashSet::new())),
                domain_id: candidate.domain_id,
                batch_time_budget: CrawlerCronConfig::default().scraper_domain_batch_time_budget,
                stop: CronStop::default(),
            }
        }

        #[tokio::test]
        async fn should_capture_the_listing_as_sold_when_the_sold_template_derives_the_same_identity()
         {
            let candidate = sold_candidate();
            let mut scraper_candidates = MockScraperCandidateService::new();
            scraper_candidates.expect_mark_fetch_failure().never();
            let ctx = context(
                &candidate,
                schema(attribute("div.product", "data-product_id"), "p.stock"),
                scraper_candidates,
            );

            let outcome = scrape_candidate(candidate, &ctx).await;

            assert!(!outcome.errored && !outcome.skipped);
            let capture = outcome
                .capture
                .expect("the sold page must be captured under the listing's identity");
            match &capture.on_success {
                RawCaptureSuccessAction::MarkScraped(meta) => {
                    assert_eq!(meta.source_listing_id, CAPTURED_IDENTITY);
                }
                _ => panic!("a sold product page must be marked scraped after capture"),
            }
            let resolved = match ProductListingRawValuesNormalizer::new()
                .normalize(&capture.item.command.input)
            {
                ProductListingRawValuesNormalizationOutcome::Resolved(resolved) => resolved,
                other => panic!("the sold capture must normalize: {other:?}"),
            };
            assert_eq!(resolved.source_listing_id.as_ref(), CAPTURED_IDENTITY);
            assert_eq!(
                resolved.availability,
                ProductListingRawValuesPatch::Set(ListingAvailabilityQuickCheck::Resolved(
                    ListingAvailability::SoldOut
                ))
            );
        }

        #[tokio::test]
        async fn should_record_an_identity_conflict_with_the_schema_cooldown_when_the_sold_template_derives_another_identity()
         {
            let before = time::OffsetDateTime::now_utc();
            let candidate = sold_candidate();
            let url = candidate.url.clone();
            let mut scraper_candidates = MockScraperCandidateService::new();
            scraper_candidates
                .expect_mark_fetch_failure()
                .once()
                .withf(
                    move |_, received_url, kind, message, status_code, next_retry_at, _| {
                        let cooldown =
                            time::Duration::seconds(SCHEMA_ERROR_RETRY_COOLDOWN.as_secs() as i64);
                        *received_url == url
                            && kind == SOURCE_LISTING_ID_CONFLICT_ERROR_KIND
                            && message.contains("`product-53036`")
                            && message.contains("`53036`")
                            && status_code.is_none()
                            && *next_retry_at >= before + cooldown
                            && *next_retry_at <= time::OffsetDateTime::now_utc() + cooldown
                    },
                )
                .returning(|_, _, _, _, _, _, _| {
                    Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
                });
            scraper_candidates.expect_mark_as_scraped().never();
            let ctx = context(
                &candidate,
                schema(attribute("div.product", "id"), "p.stock"),
                scraper_candidates,
            );

            let outcome = scrape_candidate(candidate, &ctx).await;

            assert!(outcome.capture.is_none());
            assert!(outcome.errored, "an identity conflict is a failed scrape");
            assert!(
                !outcome.skipped,
                "an identity conflict has a durable outcome"
            );
        }
    }

    mod scraper_metrics {
        use super::*;
        use crate::metrics::test_support::capture_metrics;

        fn scraped_product(url: url::Url) -> ScrapedProduct {
            ScrapedProduct {
                acquired_at: time::OffsetDateTime::now_utc(),
                raw_input: crawler_verified_removal_input(&url)
                    .unwrap_or_else(|error| panic!("test raw input: {error}")),
                hash: "hash".to_owned(),
                schema_fingerprint: "schema".to_owned(),
                raw_input_sha256: vec![7; 32],
                effective_url: url,
                source_listing_id: "source-listing".to_owned(),
            }
        }

        #[tokio::test]
        async fn should_emit_scrape_counts_and_failure_kinds_when_a_domain_batch_finishes() {
            let (metrics, _guard) = capture_metrics();
            let (candidates, domain_id) = domain_batch("metrics.example.com", 2);
            let listing_source_id = candidates[0].listing_source_id.to_string();
            let failing_url = candidates[0].url.clone();
            let mut scraper_candidates = MockScraperCandidateService::new();
            scraper_candidates
                .expect_mark_fetch_failure()
                .returning(|_, _, _, _, _, _, _| {
                    Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
                });
            let mut scraper_service = MockScraperService::new();
            scraper_service.expect_scrape().returning(move |request| {
                let url = request.url;
                let result = if url == failing_url {
                    Err(ScraperError::HttpError {
                        url,
                        kind: NetworkErrorKind::HttpStatus(500),
                        details: "internal server error".to_owned(),
                    })
                } else {
                    Ok(Some(scraped_product(url)))
                };
                Box::pin(async move { scrape_outcome(result) })
            });
            let (command_tx, _command_rx) = mpsc::channel(2);
            let ctx = ScrapeDomainContext {
                command_tx,
                ..domain_context(
                    scraper_candidates,
                    scraper_service,
                    domain_id,
                    CrawlerCronConfig::default().scraper_domain_batch_time_budget,
                )
            };

            scrape_domain_candidates(candidates, ctx).await;

            let source = |metric| {
                metrics.values_with(metric, MetricDimension::ListingSourceId, &listing_source_id)
            };
            assert_eq!(source(CrawlerMetric::ScrapesSucceeded), vec![1]);
            assert_eq!(source(CrawlerMetric::ScrapesFailed), vec![1]);
            assert_eq!(source(CrawlerMetric::DomainCircuitOpened), vec![0]);
            assert_eq!(
                metrics.values_with(
                    CrawlerMetric::UrlFailures,
                    MetricDimension::FailureKind,
                    "HTTP_500"
                ),
                vec![1]
            );
            assert_eq!(
                metrics.values_with(
                    CrawlerMetric::ScrapesSucceeded,
                    MetricDimension::Domain,
                    "metrics.example.com"
                ),
                vec![1]
            );
        }

        #[tokio::test]
        async fn should_emit_a_circuit_open_when_the_domain_write_applies() {
            let (metrics, _guard) = capture_metrics();
            let (candidates, domain_id) = domain_batch("circuit.example.com", 2);
            let mut scraper_candidates = MockScraperCandidateService::new();
            scraper_candidates
                .expect_mark_fetch_failure_and_open_domain_circuit()
                .once()
                .returning(|_| Box::pin(async { Ok(applied_domain_circuit_open_outcome()) }));
            let mut scraper_service = MockScraperService::new();
            scraper_service.expect_scrape().once().returning(|request| {
                let url = request.url;
                Box::pin(async move {
                    scrape_outcome(Err(ScraperError::HttpError {
                        url,
                        kind: NetworkErrorKind::HttpStatus(429),
                        details: "too many requests".to_owned(),
                    }))
                })
            });

            scrape_domain_candidates(
                candidates,
                domain_context(
                    scraper_candidates,
                    scraper_service,
                    domain_id,
                    CrawlerCronConfig::default().scraper_domain_batch_time_budget,
                ),
            )
            .await;

            let opened: Vec<u64> = metrics
                .values(CrawlerMetric::DomainCircuitOpened)
                .into_iter()
                .map(|(_, value)| value)
                .collect();
            assert_eq!(opened, vec![1]);
            assert_eq!(
                metrics.values_with(
                    CrawlerMetric::UrlFailures,
                    MetricDimension::FailureKind,
                    "HTTP_429"
                ),
                vec![1]
            );
        }

        #[tokio::test]
        async fn should_not_emit_batch_metrics_when_the_domain_batch_is_deferred() {
            let (metrics, _guard) = capture_metrics();
            let (candidates, domain_id) = domain_batch("deferred.example.com", 1);
            let lock_manager = Arc::new(LocalLockManager::new());
            let _held = DomainLock::try_acquire(&lock_manager, DomainLockScope::Scraper, domain_id)
                .expect("test must hold the domain lock");
            let ctx = ScrapeDomainContext {
                lock_manager,
                ..domain_context(
                    MockScraperCandidateService::new(),
                    MockScraperService::new(),
                    domain_id,
                    CrawlerCronConfig::default().scraper_domain_batch_time_budget,
                )
            };

            scrape_domain_candidates(candidates, ctx).await;

            assert!(metrics.values(CrawlerMetric::ScrapesSucceeded).is_empty());
        }

        #[tokio::test]
        async fn should_emit_products_captured_per_source_when_a_capture_batch_flushes() {
            let (metrics, _guard) = capture_metrics();
            let persisted_source = ListingSourceId::new();
            let failed_source = ListingSourceId::new();
            let mut raw_capture = MockProductListingRawCaptureService::new();
            raw_capture.expect_capture().once().returning(|_| {
                Box::pin(async {
                    vec![
                        ProductListingRawCaptureOutcome::Persisted,
                        ProductListingRawCaptureOutcome::RetryableFailure,
                    ]
                })
            });
            let mut scraper_candidates = MockScraperCandidateService::new();
            scraper_candidates
                .expect_mark_as_scraped()
                .returning(|_| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));
            let raw_capture: Arc<dyn ProductListingRawCaptureService> = Arc::new(raw_capture);
            let scraper_candidates: Arc<dyn ScraperCandidateService> = Arc::new(scraper_candidates);

            flush_batch(
                &raw_capture,
                &scraper_candidates,
                vec![
                    queued(
                        item(persisted_source, "persisted"),
                        meta(persisted_source, "https://example.test/p/1", "one"),
                    ),
                    queued(
                        item(failed_source, "failed"),
                        meta(failed_source, "https://example.test/p/2", "two"),
                    ),
                ],
                0,
            )
            .await;

            let captured = |source: ListingSourceId| {
                metrics.values_with(
                    CrawlerMetric::ProductsCaptured,
                    MetricDimension::ListingSourceId,
                    &source.to_string(),
                )
            };
            assert_eq!(captured(persisted_source), vec![1]);
            assert_eq!(captured(failed_source), vec![0]);
        }

        #[tokio::test]
        async fn should_emit_due_urls_and_pass_duration_when_a_pass_finds_no_candidates() {
            let (metrics, _guard) = capture_metrics();
            let mut scraper_candidates = MockScraperCandidateService::new();
            scraper_candidates
                .expect_get_candidates()
                .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));
            expect_due_url_count(&mut scraper_candidates, 7);
            let (spider_candidates, spider_service) = empty_spider_dependencies();
            let job = CrawlerCronJob::new(
                CrawlerCronConfig::default(),
                Arc::new(LocalLockManager::new()),
                Box::new(spider_candidates),
                Box::new(spider_service),
                Box::new(scraper_candidates),
                Box::new(MockScraperService::new()),
                noop_listing_source_registration(),
                no_raw_capture_service(),
            );

            job.run_scraper_once().await;

            assert_eq!(
                metrics.values(CrawlerMetric::ScraperDueUrls),
                vec![(vec![], 7)]
            );
            assert_eq!(metrics.values(CrawlerMetric::ScraperPassDuration).len(), 1);
        }
    }
}
