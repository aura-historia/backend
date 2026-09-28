use super::job::CrawlerCronJob;
use crate::network::policy::{
    NetworkErrorKind, domain_cooldown, domain_failure_kind, durable_retry_cooldown_for,
};
use crate::scraper::candidate_service::{
    ScraperCandidate, ScraperCandidateService, next_domain_failure_streak,
};
use crate::scraper::raw_input::{crawler_provenance, crawler_verified_removal_input};
use crate::scraper::scraper_service::domain::product::with_scrape_observation;
use crate::scraper::scraper_service::{
    DomainFetchHealth, ScrapeMode, ScrapeOutcome, ScraperError, ScraperService,
};
use crate::service::raw_capture::{
    ProductListingRawCaptureItem, ProductListingRawCaptureOutcome, ProductListingRawCaptureService,
};
use crate::spider::advisory_lock::{DomainLock, ListingSourceLock, LocalLockManager, UrlLock};
use crate::spider::classification::url_metadata::{CrawlerDisposition, CrawlerUrlWriteOutcome};
use listing_source_core::ListingSourceId;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tracing::{Instrument, debug, error, info, warn};

/// Context for scraping a domain's candidates.
struct ScrapeDomainContext {
    scraper: Arc<dyn ScraperService>,
    scraper_candidates: Arc<dyn ScraperCandidateService>,
    lock_manager: Arc<LocalLockManager>,
    command_tx: mpsc::Sender<QueuedRawCapture>,
    budget_exhausted_listing_sources: Arc<Mutex<HashSet<ListingSourceId>>>,
    schema_pending_listing_sources: Arc<Mutex<HashSet<ListingSourceId>>>,
    domain_id: crate::CrawlerDomainId,
}

/// Metadata applied only after durable raw capture succeeds.
struct CandidateMeta {
    listing_source_id: listing_source_core::ListingSourceId,
    url: url::Url,
    hash: String,
    schema_fingerprint: String,
    raw_input_sha256: Vec<u8>,
    disposition: CrawlerDisposition,
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
    enqueued_at: tokio::time::Instant,
}

#[derive(Default)]
struct ScrapeCandidateOutcome {
    capture: Option<RawCaptureRequest>,
    errored: bool,
    skipped: bool,
    domain_health: DomainFetchHealth,
    server_failure_status: Option<u16>,
}

struct ScrapeDomainOutcome {
    succeeded: usize,
    failed: usize,
    skipped: usize,
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
                match scraper_candidates
                    .mark_as_scraped(
                        &meta.listing_source_id,
                        &meta.url,
                        &meta.hash,
                        &meta.schema_fingerprint,
                        &meta.raw_input_sha256,
                        meta.disposition,
                        meta.expected_last_captured_raw_input_sha256.as_deref(),
                    )
                    .await
                {
                    Ok(CrawlerUrlWriteOutcome::Applied) => {
                        mark_as_scraped_count += 1;
                        if meta.disposition != CrawlerDisposition::Active {
                            info!(
                                metric = "crawler_disposition_transition",
                                crawler_disposition_transitions = 1_u64,
                                listing_source_id = %meta.listing_source_id,
                                crawler_disposition = meta.disposition.as_str(),
                                "crawler URL entered dormant disposition after durable raw capture"
                            );
                        }
                    }
                    Ok(CrawlerUrlWriteOutcome::NoopStale) => {
                        stale_completion_count += 1;
                        debug!(
                            listing_source_id = %meta.listing_source_id,
                            url = %meta.url,
                            crawler_url_write_outcome = "stale_noop",
                            "Skipped stale crawler scrape completion after durable raw capture"
                        );
                    }
                    Err(error) => {
                        mark_as_scraped_failure_count += 1;
                        warn!(listing_source_id = %meta.listing_source_id, error = %error, url = %meta.url, "Failed to mark product as scraped after raw capture");
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

#[allow(clippy::result_large_err)]
async fn enqueue_raw_capture(
    command_tx: &mpsc::Sender<QueuedRawCapture>,
    request: impl Into<RawCaptureRequest>,
) -> Result<Duration, mpsc::error::SendError<QueuedRawCapture>> {
    let queued = QueuedRawCapture {
        request: request.into(),
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
                &candidate.url,
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
    scrape_candidate_with_options(candidate, ctx, false).await
}

async fn scrape_candidate_with_options(
    candidate: ScraperCandidate,
    ctx: &ScrapeDomainContext,
    open_5xx_burst: bool,
) -> ScrapeCandidateOutcome {
    // Skip URLs from listing_sources with already-exhausted budgets
    {
        let exhausted = ctx.budget_exhausted_listing_sources.lock().await;
        if exhausted.contains(&candidate.listing_source_id) {
            debug!("Skipping URL — ListingSource LLM budget already exhausted in this batch");
            return ScrapeCandidateOutcome {
                capture: None,
                errored: false,
                skipped: true,
                ..Default::default()
            };
        }
    }

    {
        let pending = ctx.schema_pending_listing_sources.lock().await;
        if pending.contains(&candidate.listing_source_id) {
            debug!("Skipping URL because ListingSource has pending schema review in this batch");
            return ScrapeCandidateOutcome {
                capture: None,
                errored: false,
                skipped: true,
                ..Default::default()
            };
        }
    }

    let Some(_lock) = UrlLock::try_acquire(&ctx.lock_manager, &candidate.url) else {
        warn!("Skipping URL — lock held by another worker");
        return ScrapeCandidateOutcome {
            capture: None,
            errored: false,
            skipped: true,
            ..Default::default()
        };
    };

    let Some(_listing_source_lock) =
        ListingSourceLock::try_acquire(&ctx.lock_manager, candidate.listing_source_id)
    else {
        debug!("Skipping URL because another worker is scraping this ListingSource");
        return ScrapeCandidateOutcome {
            capture: None,
            errored: false,
            skipped: true,
            ..Default::default()
        };
    };

    let is_domain_probe = candidate.is_domain_probe;
    let (scrape_outcome, observed_domain_health) =
        with_scrape_observation(candidate.domain_id, async {
            if is_domain_probe {
                ctx.scraper
                    .scrape_with_mode(
                        &candidate.listing_source_id,
                        &candidate.url,
                        candidate.url_pattern.as_deref(),
                        candidate.last_scraped_hash.as_deref(),
                        candidate.last_scraped_schema_fingerprint.as_deref(),
                        candidate.last_captured_raw_input_sha256.as_deref(),
                        candidate.fallback_currency,
                        ScrapeMode::DomainProbe,
                    )
                    .await
            } else {
                let result = match candidate.fallback_currency {
                    Some(fallback_currency) => {
                        ctx.scraper
                            .scrape_with_fallback_currency(
                                &candidate.listing_source_id,
                                &candidate.url,
                                candidate.url_pattern.as_deref(),
                                candidate.last_scraped_hash.as_deref(),
                                candidate.last_scraped_schema_fingerprint.as_deref(),
                                candidate.last_captured_raw_input_sha256.as_deref(),
                                Some(fallback_currency),
                            )
                            .await
                    }
                    None => {
                        ctx.scraper
                            .scrape(
                                &candidate.listing_source_id,
                                &candidate.url,
                                candidate.url_pattern.as_deref(),
                                candidate.last_scraped_hash.as_deref(),
                                candidate.last_scraped_schema_fingerprint.as_deref(),
                                candidate.last_captured_raw_input_sha256.as_deref(),
                            )
                            .await
                    }
                };
                ScrapeOutcome {
                    result,
                    domain_health: DomainFetchHealth::NotObserved,
                }
            }
        })
        .await;
    let mut domain_health = observed_domain_health
        .or_else(|| {
            (!is_domain_probe).then(|| match &scrape_outcome.result {
                Ok(_) => DomainFetchHealth::Responsive,
                Err(error) => {
                    crate::scraper::scraper_service::domain::product::domain_health_for_scraper_error(
                        error,
                    )
                }
            })
        })
        .unwrap_or(scrape_outcome.domain_health);
    let scrape_result = scrape_outcome.result;
    let mut server_failure_status = None;

    let mut outcome = match scrape_result {
        Ok(Some(scraped)) => {
            let disposition = match &scraped.availability {
                product_listing_normalization::ListingAvailabilityQuickCheck::Resolved(
                    product_listing_core::listing_availability::ListingAvailability::SoldOut,
                ) => CrawlerDisposition::DormantSold,
                product_listing_normalization::ListingAvailabilityQuickCheck::Resolved(_)
                | product_listing_normalization::ListingAvailabilityQuickCheck::NoAssertion
                | product_listing_normalization::ListingAvailabilityQuickCheck::Unsupported => {
                    CrawlerDisposition::Active
                }
            };
            let provenance = match crawler_provenance(
                Some(scraped.hash.as_str()),
                Some(scraped.schema_fingerprint.as_str()),
            ) {
                Ok(provenance) => provenance,
                Err(error) => {
                    warn!(error = %error, listing_source_id = %candidate.listing_source_id, "Failed to build crawler raw-capture provenance");
                    return ScrapeCandidateOutcome {
                        capture: None,
                        errored: true,
                        skipped: false,
                        ..Default::default()
                    };
                }
            };
            let meta = CandidateMeta {
                listing_source_id: candidate.listing_source_id,
                url: candidate.url.clone(),
                hash: scraped.hash,
                schema_fingerprint: scraped.schema_fingerprint,
                raw_input_sha256: scraped.raw_input_sha256,
                disposition,
                expected_last_captured_raw_input_sha256: candidate
                    .last_captured_raw_input_sha256
                    .clone(),
            };

            if candidate.last_captured_raw_input_sha256.as_deref()
                == Some(meta.raw_input_sha256.as_slice())
                && disposition == CrawlerDisposition::Active
            {
                match ctx
                    .scraper_candidates
                    .mark_as_scraped(
                        &meta.listing_source_id,
                        &meta.url,
                        &meta.hash,
                        &meta.schema_fingerprint,
                        &meta.raw_input_sha256,
                        meta.disposition,
                        meta.expected_last_captured_raw_input_sha256.as_deref(),
                    )
                    .await
                {
                    Ok(CrawlerUrlWriteOutcome::Applied) => ScrapeCandidateOutcome {
                        capture: None,
                        errored: false,
                        skipped: false,
                        ..Default::default()
                    },
                    Ok(CrawlerUrlWriteOutcome::NoopStale) => {
                        debug!(
                            listing_source_id = %meta.listing_source_id,
                            url = %meta.url,
                            crawler_url_write_outcome = "stale_noop",
                            "Skipped stale active crawler scrape completion"
                        );
                        ScrapeCandidateOutcome {
                            capture: None,
                            errored: false,
                            skipped: true,
                            ..Default::default()
                        }
                    }
                    Err(error) => {
                        warn!(error = %error, url = %meta.url, "Failed to persist unchanged raw scrape metadata");
                        ScrapeCandidateOutcome {
                            capture: None,
                            errored: true,
                            skipped: false,
                            ..Default::default()
                        }
                    }
                }
            } else {
                ScrapeCandidateOutcome {
                    capture: Some(RawCaptureRequest {
                        item: ProductListingRawCaptureItem::crawler(
                            candidate.listing_source_id,
                            &candidate.url,
                            scraped.raw_input,
                            provenance,
                        ),
                        on_success: RawCaptureSuccessAction::MarkScraped(meta),
                        on_failure: RawCaptureFailureAction::None,
                    }),
                    errored: false,
                    skipped: false,
                    ..Default::default()
                }
            }
        }
        Ok(None) => ScrapeCandidateOutcome {
            capture: None,
            errored: false,
            skipped: true,
            ..Default::default()
        },
        Err(ScraperError::ProductListingRemoved { .. }) => handle_verified_removal(&candidate),
        Err(e) => {
            let error_message = e.to_string();
            let is_llm_budget_exceeded = matches!(&e, ScraperError::LlmBudgetExceeded { .. });
            let is_pending_schema_review = matches!(&e, ScraperError::PendingSchemaReview { .. });

            let http_failure = match &e {
                ScraperError::HttpError { kind, .. } => Some((*kind, None, None)),
                ScraperError::HttpErrorWithMetadata(metadata) => {
                    Some((metadata.kind, metadata.status_code, metadata.retry_after))
                }
                _ => None,
            };

            if let Some((kind, metadata_status_code, retry_after)) = http_failure {
                let cooldown = durable_retry_cooldown_for(kind);
                let next_retry_at = time::OffsetDateTime::now_utc()
                    + time::Duration::seconds(cooldown.as_secs() as i64);
                let status_code = match kind {
                    NetworkErrorKind::HttpStatus(code) => Some(code as i32),
                    _ => None,
                }
                .or(metadata_status_code.map(i32::from));
                server_failure_status = status_code
                    .filter(|status| matches!(*status, 500 | 502))
                    .and_then(|status| u16::try_from(status).ok());
                let url_error_kind = crawler_url_error_kind(kind, status_code);
                let domain_kind = domain_failure_kind(kind).or_else(|| {
                    (open_5xx_burst && server_failure_status.is_some())
                        .then_some(crate::network::policy::DomainFailureKind::Http5xxBurst)
                });
                if let Some(domain_kind) = domain_kind {
                    let failure_streak =
                        next_domain_failure_streak(&candidate.domain_health, domain_kind);
                    let domain_cooldown = domain_cooldown(domain_kind, failure_streak, retry_after);
                    let next_scrape_at = time::OffsetDateTime::now_utc()
                        + time::Duration::seconds(domain_cooldown.as_secs() as i64);
                    if domain_kind == crate::network::policy::DomainFailureKind::Http5xxBurst {
                        domain_health = DomainFetchHealth::CircuitFailure {
                            kind: domain_kind,
                            status_code: status_code.and_then(|value| u16::try_from(value).ok()),
                            retry_after,
                        };
                    }
                    let result = ctx
                        .scraper_candidates
                        .mark_fetch_failure_and_open_domain_circuit(
                            &candidate.listing_source_id,
                            &candidate.url,
                            &url_error_kind,
                            &error_message,
                            status_code,
                            next_retry_at,
                            candidate.last_captured_raw_input_sha256.as_deref(),
                            &candidate.domain_id,
                            &candidate.domain_health,
                            domain_kind,
                            status_code,
                            next_scrape_at,
                        )
                        .await;
                    match result {
                        Ok(domain_open_outcome) => {
                            if domain_open_outcome.url_failure == CrawlerUrlWriteOutcome::NoopStale
                            {
                                debug!(
                                    listing_source_id = %candidate.listing_source_id,
                                    url = %candidate.url,
                                    crawler_url_write_outcome = "stale_noop",
                                    "Skipped stale crawler fetch failure metadata"
                                );
                            }

                            match domain_open_outcome.domain_circuit {
                                CrawlerUrlWriteOutcome::Applied => {
                                    let persisted_failure_streak = domain_open_outcome
                                        .persisted_failure_streak
                                        .expect("applied domain circuit must report its streak");
                                    warn!(
                                        event = "crawler.scraper.domain_circuit_opened",
                                        domain_id = %candidate.domain_id,
                                        listing_source_domain = %candidate.listing_source_domain,
                                        error_kind = %domain_kind.as_str(),
                                        status_code = ?status_code,
                                        failure_streak = persisted_failure_streak,
                                        retry_after_seconds = retry_after.map(|value| value.as_secs()),
                                        next_scrape_at = %next_scrape_at,
                                        "Opened scraper domain circuit"
                                    );
                                }
                                CrawlerUrlWriteOutcome::NoopStale => debug!(
                                    event = "crawler.scraper.domain_circuit_open_stale",
                                    domain_id = %candidate.domain_id,
                                    listing_source_domain = %candidate.listing_source_domain,
                                    error_kind = %domain_kind.as_str(),
                                    "Skipped stale scraper domain circuit opening"
                                ),
                            }
                        }
                        Err(error) => warn!(
                            event = "crawler.scraper.domain_circuit_open_failed",
                            error = %error,
                            domain_id = %candidate.domain_id,
                            listing_source_domain = %candidate.listing_source_domain,
                            error_kind = %domain_kind.as_str(),
                            "Failed to persist scraper domain circuit opening"
                        ),
                    }
                } else {
                    match ctx
                        .scraper_candidates
                        .mark_fetch_failure(
                            &candidate.listing_source_id,
                            &candidate.url,
                            &url_error_kind,
                            &error_message,
                            status_code,
                            next_retry_at,
                            candidate.last_captured_raw_input_sha256.as_deref(),
                        )
                        .await
                    {
                        Ok(CrawlerUrlWriteOutcome::Applied) => {}
                        Ok(CrawlerUrlWriteOutcome::NoopStale) => {
                            debug!(
                                listing_source_id = %candidate.listing_source_id,
                                url = %candidate.url,
                                crawler_url_write_outcome = "stale_noop",
                                "Skipped stale crawler fetch failure metadata"
                            );
                        }
                        Err(mark_err) => {
                            warn!(
                                error = %mark_err,
                                "Failed to persist scraper fetch failure metadata"
                            );
                        }
                    }
                }
            } else {
                // Non-HTTP errors: schema failures, normalization errors, etc.
                // These do not affect retry scheduling but are persisted for observability.
                let error_kind = scraper_error_kind(&e);
                match &e {
                    ScraperError::SchemaRegenerationExhausted { .. }
                    | ScraperError::FreshSchemaNormalizationFailed { .. }
                    | ScraperError::SchemaClassificationRejected { .. }
                    | ScraperError::LlmBudgetExceeded { .. }
                    | ScraperError::PendingSchemaReview { .. } => {
                        let cooldown = std::time::Duration::from_secs(30 * 60);
                        let next_retry_at = time::OffsetDateTime::now_utc()
                            + time::Duration::seconds(cooldown.as_secs() as i64);
                        match ctx
                            .scraper_candidates
                            .mark_fetch_failure(
                                &candidate.listing_source_id,
                                &candidate.url,
                                error_kind,
                                &error_message,
                                None,
                                next_retry_at,
                                candidate.last_captured_raw_input_sha256.as_deref(),
                            )
                            .await
                        {
                            Ok(CrawlerUrlWriteOutcome::Applied) => {}
                            Ok(CrawlerUrlWriteOutcome::NoopStale) => {
                                debug!(
                                    listing_source_id = %candidate.listing_source_id,
                                    url = %candidate.url,
                                    crawler_url_write_outcome = "stale_noop",
                                    "Skipped stale crawler schema/classification cooldown metadata"
                                );
                            }
                            Err(mark_err) => {
                                warn!(
                                    error = %mark_err,
                                    "Failed to persist schema/classification cooldown metadata"
                                );
                            }
                        }
                    }
                    _ => {
                        match ctx
                            .scraper_candidates
                            .mark_scraper_failure(
                                &candidate.listing_source_id,
                                &candidate.url,
                                error_kind,
                                &error_message,
                                candidate.last_captured_raw_input_sha256.as_deref(),
                            )
                            .await
                        {
                            Ok(CrawlerUrlWriteOutcome::Applied) => {}
                            Ok(CrawlerUrlWriteOutcome::NoopStale) => {
                                debug!(
                                    listing_source_id = %candidate.listing_source_id,
                                    url = %candidate.url,
                                    crawler_url_write_outcome = "stale_noop",
                                    "Skipped stale crawler failure metadata"
                                );
                            }
                            Err(mark_err) => {
                                warn!(
                                    error = %mark_err,
                                    "Failed to persist scraper failure metadata"
                                );
                            }
                        }
                    }
                }
            }

            // Log LLM budget exhaustion at INFO level only once per ListingSource per batch
            if is_llm_budget_exceeded {
                if let ScraperError::LlmBudgetExceeded {
                    listing_source_id,
                    max_calls,
                    ..
                } = &e
                {
                    let mut exhausted = ctx.budget_exhausted_listing_sources.lock().await;
                    if exhausted.insert(*listing_source_id) {
                        info!(
                            listing_source_id = %listing_source_id,
                            max_calls,
                            "LLM call budget exhausted for ListingSource; skipping remaining URLs in batch"
                        );
                    }
                }
            } else if is_pending_schema_review {
                let mut pending = ctx.schema_pending_listing_sources.lock().await;
                if pending.insert(candidate.listing_source_id) {
                    info!(
                        listing_source_id = %candidate.listing_source_id,
                        "ProductListing schema review pending for ListingSource; skipping remaining URLs in batch"
                    );
                }
                warn!(error = %e, "Scraper run failed");
            } else if matches!(
                &e,
                ScraperError::ProductListingRemoved { .. } | ScraperError::NotProductPage { .. }
            ) {
                debug!(error = %e, "Scraper run failed");
            } else {
                warn!(error = %e, "Scraper run failed");
            }

            ScrapeCandidateOutcome {
                capture: None,
                errored: true,
                skipped: false,
                ..Default::default()
            }
        }
    };
    outcome.domain_health = domain_health;
    outcome.server_failure_status = server_failure_status;
    outcome
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
        ScraperError::SchemaServiceError(_) => "SchemaServiceError",
        ScraperError::RemovedPageSchemaDatabaseError(_) => "RemovedPageSchemaDatabaseError",
        ScraperError::SchemaRegenerationExhausted { .. } => "SchemaRegenerationExhausted",
        ScraperError::FreshSchemaNormalizationFailed { .. } => "FreshSchemaNormalizationFailed",
        ScraperError::LlmBudgetExceeded { .. } => "LlmBudgetExceeded",
        ScraperError::NormalizationError(_) => "NormalizationError",
        ScraperError::RawNormalizationInput(_) => "RawNormalizationInput",
        ScraperError::SchemaFingerprint(_) => "SchemaFingerprint",
        ScraperError::PendingSchemaReview { .. } => "PendingSchemaReview",
    }
}

fn crawler_url_error_kind(kind: NetworkErrorKind, status_code: Option<i32>) -> String {
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
    };

    let Some(_domain_lock) = DomainLock::try_acquire(&ctx.lock_manager, ctx.domain_id) else {
        warn!(domain_id = %ctx.domain_id, "Skipping scraper domain - lock held by another worker");
        outcome.skipped = candidates.len();
        return outcome;
    };

    let mut server_failure_urls = HashSet::new();
    let candidate_count = candidates.len();
    for (candidate_index, candidate) in candidates.into_iter().enumerate() {
        let candidate_url = candidate.url.clone();
        let open_5xx_burst =
            !server_failure_urls.contains(&candidate_url) && server_failure_urls.len() >= 2;
        let expected_domain_health = candidate.domain_health.clone();
        let candidate_outcome =
            scrape_candidate_with_options(candidate, &ctx, open_5xx_burst).await;

        if candidate_outcome.errored {
            outcome.failed += 1;
        } else if let Some(pair) = candidate_outcome.capture {
            match enqueue_raw_capture(&ctx.command_tx, pair).await {
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
        } else if candidate_outcome.skipped {
            outcome.skipped += 1;
        } else {
            outcome.succeeded += 1;
        }

        match candidate_outcome.server_failure_status {
            Some(500 | 502) => {
                server_failure_urls.insert(candidate_url);
            }
            Some(_) => {}
            None => {}
        }

        if matches!(
            candidate_outcome.domain_health,
            DomainFetchHealth::Responsive
        ) && expected_domain_health.scrape_failure_streak > 0
        {
            match ctx
                .scraper_candidates
                .close_domain_circuit(&ctx.domain_id, &expected_domain_health)
                .await
            {
                Ok(CrawlerUrlWriteOutcome::Applied) => info!(
                    event = "crawler.scraper.domain_circuit_closed",
                    domain_id = %ctx.domain_id,
                    previous_failure_streak = expected_domain_health.scrape_failure_streak,
                    last_error_kind = ?expected_domain_health.last_scrape_error_kind,
                    "Closed scraper domain circuit after responsive fetch"
                ),
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

        if let DomainFetchHealth::CircuitFailure { .. } = &candidate_outcome.domain_health {
            warn!(
                event = "crawler.scraper.domain_circuit_batch_stopped",
                domain_id = %ctx.domain_id,
                "Stopping remaining scraper URLs after a domain circuit failure"
            );
            outcome.skipped += candidate_count.saturating_sub(candidate_index + 1);
            break;
        }
    }

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
        let schema_pending_listing_sources = Arc::new(Mutex::new(HashSet::new()));

        let mut unique_listing_source_ids = HashSet::new();
        let mut total = 0usize;
        let mut succeeded = 0usize;
        let mut failed = 0usize;
        let mut skipped = 0usize;
        let mut started = false;
        let mut no_more_candidates = false;

        let raw_capture_collector = tokio::spawn(run_raw_capture_collector(
            command_rx,
            Arc::clone(&self.raw_capture),
            Arc::clone(&self.scraper_candidates),
            self.config.effective_push_batch_size(),
            self.config.effective_push_max_batch_age(),
        ));

        loop {
            while join_set.len() < scraper_concurrency {
                if let Some((domain, candidates)) = pending_domains
                    .iter()
                    .position(|(domain, _)| !active_domains.contains(domain))
                    .and_then(|idx| pending_domains.remove(idx))
                {
                    let scraper = Arc::clone(&self.scraper_service);
                    let scraper_candidates = Arc::clone(&self.scraper_candidates);
                    let lock_manager = Arc::clone(&self.lock_manager);
                    let domain_tx = command_tx.clone();
                    let budget_exhausted_listing_sources =
                        Arc::clone(&budget_exhausted_listing_sources);
                    let schema_pending_listing_sources =
                        Arc::clone(&schema_pending_listing_sources);
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
                                schema_pending_listing_sources,
                                domain_id: domain,
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
                pending_domains.extend(by_domain);
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

        let duration_ms = pass_start.elapsed().as_millis() as u64;
        info!(
            total,
            succeeded, failed, skipped, duration_ms, "Scraper scheduler pass complete"
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::candidate_service::{
        DomainCircuitOpenOutcome, DomainHealthSnapshot, MockScraperCandidateService,
    };
    use crate::scraper::css_selector::product_schema_service::ProductListingSchemaServiceError;
    use crate::scraper::scraper_service::{MockScraperService, ScrapedProduct};
    use crate::service::cron::config::CrawlerCronConfig;
    use crate::service::cron::test_support::{noop_listing_source_registration, scraper_candidate};
    use crate::service::listing_source_registration::{
        ListingSourceRegistrationService, ListingSourceSyncError,
        MockListingSourceRegistrationRepository, MockListingSourceRegistrationSource,
    };
    use crate::service::raw_capture::MockProductListingRawCaptureService;
    use crate::spider::advisory_lock::LocalLockManager;
    use crate::spider::candidate_service::MockSpiderCandidateService;
    use crate::spider::service::MockSpiderService;
    use listing_source_core::ListingSourceId;

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

    fn item(listing_source_id: ListingSourceId, product_id: &str) -> ProductListingRawCaptureItem {
        let url = url::Url::parse(&format!("https://example.test/products/{product_id}"))
            .unwrap_or_else(|error| panic!("test URL: {error}"));
        let input = crawler_verified_removal_input(&url)
            .unwrap_or_else(|error| panic!("test removal input: {error}"));
        let provenance = crawler_provenance(None, None)
            .unwrap_or_else(|error| panic!("test provenance: {error}"));
        ProductListingRawCaptureItem::crawler(listing_source_id, &url, input, provenance)
    }

    fn meta(listing_source_id: ListingSourceId, url: &str, hash: &str) -> CandidateMeta {
        CandidateMeta {
            listing_source_id,
            url: url::Url::parse(url).unwrap(),
            hash: hash.to_owned(),
            schema_fingerprint: "schema-fingerprint".to_owned(),
            raw_input_sha256: vec![3; 32],
            disposition: CrawlerDisposition::Active,
            expected_last_captured_raw_input_sha256: None,
        }
    }

    fn queued(item: ProductListingRawCaptureItem, meta: CandidateMeta) -> QueuedRawCapture {
        QueuedRawCapture {
            request: (item, meta).into(),
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
            .withf(
                move |listing_source_id,
                      url,
                      hash,
                      _,
                      _,
                      _,
                      expected_last_captured_raw_input_sha256| {
                    *listing_source_id == first_listing_source_id
                        && url == &first_url
                        && hash == "first"
                        && expected_last_captured_raw_input_sha256.as_deref()
                            == Some(observed_raw_input_sha256_for_mark.as_slice())
                },
            )
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

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
            .withf(
                move |listing_source_id,
                      url,
                      hash,
                      _,
                      _,
                      _,
                      expected_last_captured_raw_input_sha256| {
                    *listing_source_id == first_listing_source_id
                        && url == &first_url
                        && hash == "first"
                        && expected_last_captured_raw_input_sha256.is_none()
                },
            )
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

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
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

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
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

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
            (
                item(listing_source_id, "one"),
                meta(listing_source_id, "https://example.com/product/one", "one"),
            ),
        )
        .await
        .expect("first enqueue must succeed");
        enqueue_raw_capture(
            &tx,
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

    fn scraper_job(
        config: CrawlerCronConfig,
        scraper_candidates: MockScraperCandidateService,
        scraper_service: MockScraperService,
    ) -> CrawlerCronJob {
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
            schema_pending_listing_sources: Arc::new(Mutex::new(HashSet::new())),
            domain_id: crate::CrawlerDomainId::new(),
        }
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
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(None) }));

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
            .withf(
                move |_,
                      _,
                      _,
                      _,
                      status_code,
                      next_retry_at,
                      expected_raw_input_sha256,
                      _,
                      expected_domain_health,
                      domain_error_kind,
                      domain_status_code,
                      _| {
                    let expected_cooldown = durable_retry_cooldown_for(NetworkErrorKind::Timeout);
                    let expected_from =
                        before + time::Duration::seconds(expected_cooldown.as_secs() as i64);
                    let expected_until = time::OffsetDateTime::now_utc()
                        + time::Duration::seconds(expected_cooldown.as_secs() as i64)
                        + time::Duration::seconds(1);
                    status_code.is_none()
                        && *next_retry_at >= expected_from
                        && *next_retry_at <= expected_until
                        && expected_raw_input_sha256.is_none()
                        && expected_domain_health.scrape_failure_streak == 0
                        && *domain_error_kind == crate::network::policy::DomainFailureKind::Timeout
                        && domain_status_code.is_none()
                },
            )
            .returning(|_, _, _, _, _, _, _, _, _, _, _, _| {
                Box::pin(async { Ok(applied_domain_circuit_open_outcome()) })
            });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .returning(|_, url, _, _, _, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::HttpError {
                        url,
                        kind: crate::network::policy::NetworkErrorKind::Timeout,
                        details: "timeout".to_string(),
                    })
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
                enqueued_at: tokio::time::Instant::now(),
            }],
            0,
        )
        .await;
    }

    #[tokio::test]
    async fn should_queue_sold_observation_when_previous_raw_input_hash_matches() {
        let url = url::Url::parse("https://example.com/product/sold").unwrap();
        let mut candidate = scraper_candidate("ListingSource", url.clone());
        candidate.last_captured_raw_input_sha256 = Some(vec![3; 32]);

        let raw_input = crawler_verified_removal_input(&url)
            .unwrap_or_else(|error| panic!("test raw input: {error}"));
        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().once().return_once(move |_, _, _, _, _, expected_raw_input_sha256| {
            assert_eq!(expected_raw_input_sha256, Some(&[3; 32][..]));
            Box::pin(async move {
                Ok(Some(ScrapedProduct {
                    raw_input,
                    availability: product_listing_normalization::ListingAvailabilityQuickCheck::Resolved(
                        product_listing_core::listing_availability::ListingAvailability::SoldOut,
                    ),
                    hash: "sold-hash".to_owned(),
                    schema_fingerprint: "sold-schema".to_owned(),
                    raw_input_sha256: vec![3; 32],
                }))
            })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates.expect_mark_as_scraped().never();
        let ctx = scrape_candidate_context(scraper_candidates, scraper_service);

        let outcome = scrape_candidate(candidate, &ctx).await;

        let request = outcome.capture.expect("sold scrape must queue raw capture");
        let RawCaptureSuccessAction::MarkScraped(meta) = request.on_success else {
            panic!("sold scrape must queue a scrape completion");
        };
        assert_eq!(
            meta.expected_last_captured_raw_input_sha256.as_deref(),
            Some(&[3_u8; 32][..])
        );
        assert!(!outcome.errored);
        assert!(!outcome.skipped);
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
        scraper_service
            .expect_scrape()
            .once()
            .returning(|_, url, _, _, _, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::HttpError {
                        url,
                        kind: NetworkErrorKind::HttpStatus(500),
                        details: "internal server error".to_string(),
                    })
                })
            });

        let ctx = scrape_candidate_context(scraper_candidates, scraper_service);

        let outcome = scrape_candidate(candidate, &ctx).await;

        assert!(outcome.errored);
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
            .withf(
                move |received_listing_source_id,
                      received_url,
                      error_kind,
                      _,
                      status_code,
                      _,
                      expected_raw_input_sha256,
                      _,
                      expected_domain_health,
                      domain_error_kind,
                      domain_status_code,
                      _| {
                    *received_listing_source_id == listing_source_id
                        && received_url == &url
                        && error_kind == "HTTP_429"
                        && *status_code == Some(429)
                        && expected_raw_input_sha256.is_none()
                        && expected_domain_health.scrape_failure_streak == 0
                        && *domain_error_kind == crate::network::policy::DomainFailureKind::Http429
                        && *domain_status_code == Some(429)
                },
            )
            .returning(|_, _, _, _, _, _, _, _, _, _, _, _| {
                Box::pin(async { Ok(applied_domain_circuit_open_outcome()) })
            });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .once()
            .returning(|_, url, _, _, _, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::HttpError {
                        url,
                        kind: NetworkErrorKind::HttpStatus(429),
                        details: "too many requests".to_string(),
                    })
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
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

        let scrape_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let scrape_count_for_mock = Arc::clone(&scrape_count);
        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(2)
            .returning(move |_, url, _, _, _, _| {
                let url = url.clone();
                let attempt = scrape_count_for_mock.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    if attempt == 0 {
                        Err(ScraperError::HttpError {
                            url,
                            kind: crate::network::policy::NetworkErrorKind::HttpStatus(500),
                            details: "internal server error".to_string(),
                        })
                    } else {
                        let raw_input = crawler_verified_removal_input(&url)
                            .unwrap_or_else(|error| panic!("test raw input: {error}"));
                        Ok(Some(ScrapedProduct {
                            raw_input,
                            availability: product_listing_normalization::ListingAvailabilityQuickCheck::Resolved(
                                product_listing_core::listing_availability::ListingAvailability::SoldOut,
                            ),
                            hash: "success-hash".to_owned(),
                            schema_fingerprint: "success-schema".to_owned(),
                            raw_input_sha256: vec![2; 32],
                        }))
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
            .withf(
                |_, _, error_kind, _, status_code, _, _, _, _, domain_error_kind, _, _| {
                    error_kind == "HTTP_500"
                        && *status_code == Some(500)
                        && *domain_error_kind
                            == crate::network::policy::DomainFailureKind::Http5xxBurst
                },
            )
            .returning(|_, _, _, _, _, _, _, _, _, _, _, _| {
                Box::pin(async { Ok(applied_domain_circuit_open_outcome()) })
            });

        let scrape_count = Arc::new(AtomicUsize::new(0));
        let scrape_count_for_mock = Arc::clone(&scrape_count);
        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(3)
            .returning(move |_, url, _, _, _, _| {
                let url = url.clone();
                scrape_count_for_mock.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    Err(ScraperError::HttpError {
                        url,
                        kind: NetworkErrorKind::HttpStatus(500),
                        details: "internal server error".to_owned(),
                    })
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
            .withf(
                |_, received_url, error_kind, _, status_code, _, _, _, _, domain_kind, _, _| {
                    received_url.as_str().ends_with("/4")
                        && error_kind == "HTTP_500"
                        && *status_code == Some(500)
                        && *domain_kind == crate::network::policy::DomainFailureKind::Http5xxBurst
                },
            )
            .returning(|_, _, _, _, _, _, _, _, _, _, _, _| {
                Box::pin(async { Ok(applied_domain_circuit_open_outcome()) })
            });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(4)
            .returning(move |_, url, _, _, _, _| {
                let url = url.clone();
                let status = url
                    .path_segments()
                    .and_then(|mut segments| segments.next_back())
                    .unwrap()
                    .parse::<u16>()
                    .unwrap();
                Box::pin(async move {
                    match status {
                        2 => Ok(None),
                        3 => Err(ScraperError::HttpError {
                            url,
                            kind: NetworkErrorKind::HttpStatus(502),
                            details: "bad gateway".to_owned(),
                        }),
                        _ => Err(ScraperError::HttpError {
                            url,
                            kind: NetworkErrorKind::HttpStatus(500),
                            details: "internal server error".to_owned(),
                        }),
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
                schema_pending_listing_sources: Arc::new(Mutex::new(HashSet::new())),
                domain_id,
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
            .returning(move |_, url, _, _, _, _| {
                let status = if url == &other_url_for_mock { 502 } else { 500 };
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::HttpError {
                        url,
                        kind: NetworkErrorKind::HttpStatus(status),
                        details: "server error".to_owned(),
                    })
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
                schema_pending_listing_sources: Arc::new(Mutex::new(HashSet::new())),
                domain_id,
            },
        )
        .await;

        assert_eq!(outcome.failed, 3);
        assert_eq!(outcome.skipped, 0);
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
            .returning(|_, _, _, _, _, _, _, _, _, _, _, _| {
                Box::pin(async { Ok(applied_domain_circuit_open_outcome()) })
            });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .once()
            .returning(|_, url, _, _, _, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::HttpError {
                        url,
                        kind: NetworkErrorKind::HttpStatus(429),
                        details: "too many requests".to_owned(),
                    })
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
                schema_pending_listing_sources: Arc::new(Mutex::new(HashSet::new())),
                domain_id,
            },
        )
        .await;

        assert_eq!(outcome.failed, 1);
        assert_eq!(outcome.succeeded, 0);
        assert_eq!(outcome.skipped, 2);
        assert_eq!(3, outcome.succeeded + outcome.failed + outcome.skipped);
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
            .withf(
                move |_,
                      received_url,
                      error_kind,
                      _,
                      status_code,
                      _,
                      expected_raw_input_sha256,
                      _,
                      _,
                      domain_error_kind,
                      domain_status_code,
                      _| {
                    received_url == &first_url
                        && error_kind == "HTTP_429"
                        && *status_code == Some(429)
                        && expected_raw_input_sha256.is_none()
                        && *domain_error_kind == crate::network::policy::DomainFailureKind::Http429
                        && *domain_status_code == Some(429)
                },
            )
            .returning(|_, _, _, _, _, _, _, _, _, _, _, _| {
                Box::pin(async { Ok(applied_domain_circuit_open_outcome()) })
            });

        let scrape_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let scrape_count_for_mock = Arc::clone(&scrape_count);
        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(1)
            .returning(move |_, url, _, _, _, _| {
                let url = url.clone();
                let attempt = scrape_count_for_mock.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    if attempt == 0 {
                        Err(ScraperError::HttpError {
                            url,
                            kind: crate::network::policy::NetworkErrorKind::HttpStatus(429),
                            details: "too many requests".to_string(),
                        })
                    } else {
                        Ok(None)
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
        scraper_service
            .expect_scrape()
            .returning(|listing_source_id, url, _, _, _, _| {
                let url = url.clone();
                let listing_source_id = *listing_source_id;
                Box::pin(async move {
                    Err(ScraperError::LlmBudgetExceeded {
                        listing_source_id,
                        url,
                        max_calls: 5,
                    })
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
    async fn should_skip_remaining_listing_source_candidates_when_schema_review_is_pending() {
        let listing_source_id = ListingSourceId::new();
        let first_url = url::Url::parse("https://example.com/product/1").unwrap();
        let second_url = url::Url::parse("https://example.com/product/2").unwrap();

        let first_url_for_candidates = first_url.clone();
        let second_url_for_candidates = second_url.clone();

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
            .expect_mark_fetch_failure()
            .once()
            .withf(
                move |received_listing_source_id,
                      received_url,
                      kind,
                      _,
                      _,
                      _,
                      expected_raw_input_sha256| {
                    *received_listing_source_id == listing_source_id
                        && received_url == &first_url
                        && kind == "PendingSchemaReview"
                        && expected_raw_input_sha256.is_none()
                },
            )
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .once()
            .returning(|_, url, _, _, _, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::PendingSchemaReview {
                        url,
                        review_id: crate::CrawlerReviewId::new(),
                    })
                })
            });

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }

    /// `FreshSchemaNormalizationFailed` must be handled identically to
    /// `SchemaRegenerationExhausted`: write a cooldown via `mark_fetch_failure`
    /// so the URL is held back until the backoff window expires.
    #[tokio::test]
    async fn should_mark_fetch_failure_for_fresh_schema_normalization_failure() {
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
        scraper_service
            .expect_scrape()
            .returning(|_, url, _, _, _, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::FreshSchemaNormalizationFailed {
                        url,
                        attempts: 3,
                        last_norm_error: Box::new(
                            crate::scraper::normalization::product_normalization_service::NormalizationError::TitleEmpty,
                        ),
                    })
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
                    *received_url == url
                        && kind == "SchemaClassificationRejected"
                        && status_code.is_none()
                        && *next_retry_at > before
                        && expected_raw_input_sha256.is_none()
                },
            )
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });
        scraper_candidates.expect_mark_scraper_failure().never();

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .once()
            .returning(|_, url, _, _, _, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::SchemaClassificationRejected {
                        url,
                        details: "removed classification requires HIGH confidence".to_string(),
                    })
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
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(None) }));

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
            .returning(move |_, url, _, _, _, _| {
                let url = url.clone();
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
                    Ok(None)
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
        scraper_service
            .expect_scrape()
            .once()
            .returning(|_, _, _, _, _, _| {
                Box::pin(async {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    Ok(None)
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
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(None) }));

        let lock_manager = Arc::new(LocalLockManager::new());
        let prelocked = url::Url::parse("https://domain-a.com/product/1").unwrap();
        let _prelock = UrlLock::try_acquire(&lock_manager, &prelocked).unwrap();
        let (spider_candidates, spider_service) = empty_spider_dependencies();

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
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(None) }));

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
        scraper_service
            .expect_scrape_with_mode()
            .once()
            .returning(|_, _, _, _, _, _, _, _| {
                Box::pin(async move {
                    ScrapeOutcome {
                        result: Err(ScraperError::SchemaServiceError(
                            ProductListingSchemaServiceError::DatabaseError(
                                sqlx::Error::RowNotFound,
                            ),
                        )),
                        domain_health: DomainFetchHealth::NotObserved,
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
            .expect_mark_fetch_failure()
            .once()
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });
        scraper_candidates
            .expect_close_domain_circuit()
            .once()
            .withf(move |domain_id, snapshot| {
                *domain_id == expected_domain_id && snapshot.scrape_failure_streak == 1
            })
            .returning(|_, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape_with_mode()
            .once()
            .returning(|_, url, _, _, _, _, _, _| {
                let url = url.clone();
                Box::pin(async move {
                    ScrapeOutcome {
                        result: Err(ScraperError::PendingSchemaReview {
                            url,
                            review_id: crate::CrawlerReviewId::new(),
                        }),
                        domain_health: DomainFetchHealth::Responsive,
                    }
                })
            });

        let mut ctx = scrape_candidate_context(scraper_candidates, scraper_service);
        ctx.domain_id = candidate.domain_id;
        let outcome = scrape_domain_candidates(vec![candidate], ctx).await;

        assert_eq!(outcome.failed, 1);
        assert_eq!(outcome.skipped, 0);
    }
}
