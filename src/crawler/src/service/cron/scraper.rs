use super::job::CrawlerCronJob;
use crate::network::policy::{NetworkErrorKind, durable_retry_cooldown_for};
use crate::scraper::candidate_service::{
    ProductListingSnapshot, ScraperCandidate, ScraperCandidateService,
};
use crate::scraper::scraper_service::{ScraperError, ScraperService};
use crate::service::product_push::{
    ProductListingPushItem, ProductListingPushService, normalize_to_upsert,
};
use crate::spider::advisory_lock::{LocalLockManager, ShopLock, UrlLock};
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
    command_tx: mpsc::Sender<QueuedProductListingPush>,
    budget_exhausted_listing_sources: Arc<Mutex<HashSet<ListingSourceId>>>,
    schema_pending_listing_sources: Arc<Mutex<HashSet<ListingSourceId>>>,
}

/// Metadata carried alongside a [`UpsertProductListingCommand`] so the push-collector
/// can call [`ScraperCandidateService::mark_as_scraped`] only after the push
/// has been confirmed.
struct CandidateMeta {
    listing_source_id: listing_source_core::ListingSourceId,
    url: url::Url,
    hash: String,
    snapshot: ProductListingSnapshot,
}

struct QueuedProductListingPush {
    item: ProductListingPushItem,
    meta: CandidateMeta,
    enqueued_at: tokio::time::Instant,
}

struct ScrapeCandidateOutcome {
    command: Option<(ProductListingPushItem, CandidateMeta)>,
    errored: bool,
    skipped: bool,
}

struct ScrapeDomainOutcome {
    succeeded: usize,
    failed: usize,
    skipped: usize,
}

struct ScheduledScrapeDomainOutcome {
    domain: String,
    outcome: ScrapeDomainOutcome,
}

/// Pushes a batch of `(command, meta)` pairs to the product backend and then
/// calls [`ScraperCandidateService::mark_as_scraped`] for each command that
/// was successfully persisted.
///
/// The push result has one boolean per input position, so only the corresponding
/// crawler URL is marked scraped after its product command succeeds.
#[tracing::instrument(
    name = "crawler_flush_push_batch",
    skip(push_service, scraper_candidates, batch),
    fields(batch_size = batch.len())
)]
async fn flush_batch(
    push_service: &Arc<dyn ProductListingPushService>,
    scraper_candidates: &Arc<dyn ScraperCandidateService>,
    batch: Vec<QueuedProductListingPush>,
    queue_depth: usize,
) {
    let batch_size = batch.len();
    let oldest_item_age_ms = batch
        .iter()
        .map(|queued| queued.enqueued_at.elapsed().as_millis())
        .max()
        .unwrap_or(0);
    let (products, metas): (Vec<_>, Vec<_>) = batch
        .into_iter()
        .map(|queued| (queued.item, queued.meta))
        .unzip();

    let push_started_at = tokio::time::Instant::now();
    let mut succeeded = push_service.push(products).await;
    let upsert_latency_ms = push_started_at.elapsed().as_millis();
    let expected = metas.len();
    let actual = succeeded.len();

    if actual != expected {
        warn!(
            expected,
            actual,
            "ProductListing push returned an incomplete result; unmatched URLs will be retried"
        );
    }

    succeeded.truncate(expected);
    if succeeded.len() < expected {
        succeeded.resize(expected, false);
    }

    let persisted_count = succeeded.iter().filter(|succeeded| **succeeded).count();
    let persistence_failure_count = expected.saturating_sub(persisted_count);
    let mut mark_as_scraped_count = 0;
    let mut mark_as_scraped_failure_count = 0;

    for (meta, succeeded) in metas.into_iter().zip(succeeded) {
        if succeeded {
            match scraper_candidates
                .mark_as_scraped(
                    &meta.listing_source_id,
                    &meta.url,
                    &meta.hash,
                    &meta.snapshot,
                )
                .await
            {
                Ok(()) => mark_as_scraped_count += 1,
                Err(error) => {
                    mark_as_scraped_failure_count += 1;
                    warn!(listing_source_id = %meta.listing_source_id, error = %error, url = %meta.url, "Failed to mark product as scraped after push");
                }
            }
        }
    }

    info!(
        event = "crawler.product_push.batch",
        batch_size,
        queue_depth,
        oldest_item_age_ms,
        upsert_latency_ms,
        persisted_count,
        persistence_failure_count,
        mark_as_scraped_count,
        mark_as_scraped_failure_count,
        "Crawler product push batch complete"
    );
}

#[allow(clippy::result_large_err)]
async fn enqueue_product_push(
    command_tx: &mpsc::Sender<QueuedProductListingPush>,
    pair: (ProductListingPushItem, CandidateMeta),
) -> Result<Duration, mpsc::error::SendError<QueuedProductListingPush>> {
    let (item, meta) = pair;
    let queued = QueuedProductListingPush {
        item,
        meta,
        enqueued_at: tokio::time::Instant::now(),
    };
    let wait_started_at = tokio::time::Instant::now();

    command_tx.send(queued).await?;

    Ok(wait_started_at.elapsed())
}

#[tracing::instrument(
    name = "crawler_scrape_candidate",
    skip(candidate, ctx),
    fields(
        listing_source_id = %candidate.listing_source_id,
        url = %candidate.url
    )
)]
async fn scrape_candidate(
    candidate: ScraperCandidate,
    ctx: &ScrapeDomainContext,
) -> ScrapeCandidateOutcome {
    // Skip URLs from listing_sources with already-exhausted budgets
    {
        let exhausted = ctx.budget_exhausted_listing_sources.lock().await;
        if exhausted.contains(&candidate.listing_source_id) {
            debug!("Skipping URL — shop LLM budget already exhausted in this batch");
            return ScrapeCandidateOutcome {
                command: None,
                errored: false,
                skipped: true,
            };
        }
    }

    {
        let pending = ctx.schema_pending_listing_sources.lock().await;
        if pending.contains(&candidate.listing_source_id) {
            debug!("Skipping URL because shop has pending schema review in this batch");
            return ScrapeCandidateOutcome {
                command: None,
                errored: false,
                skipped: true,
            };
        }
    }

    let Some(_lock) = UrlLock::try_acquire(&ctx.lock_manager, &candidate.url) else {
        warn!("Skipping URL — lock held by another worker");
        return ScrapeCandidateOutcome {
            command: None,
            errored: false,
            skipped: true,
        };
    };

    let Some(_shop_lock) = ShopLock::try_acquire(&ctx.lock_manager, candidate.listing_source_id)
    else {
        debug!("Skipping URL because another worker is scraping this shop");
        return ScrapeCandidateOutcome {
            command: None,
            errored: false,
            skipped: true,
        };
    };

    match ctx
        .scraper
        .scrape(
            &candidate.listing_source_id,
            &candidate.url,
            candidate.url_pattern.as_deref(),
            candidate.last_scraped_hash.as_deref(),
        )
        .await
    {
        Ok(Some(scraped)) => {
            let raw_attributes = scraped.product.raw_attributes.clone();
            let meta = CandidateMeta {
                listing_source_id: candidate.listing_source_id,
                url: candidate.url.clone(),
                hash: scraped.hash,
                snapshot: scraped.snapshot,
            };
            ScrapeCandidateOutcome {
                command: normalize_to_upsert(scraped.product, &candidate).map(|command| {
                    (
                        ProductListingPushItem {
                            command,
                            raw_attributes,
                        },
                        meta,
                    )
                }),
                errored: false,
                skipped: false,
            }
        }
        Ok(None) => ScrapeCandidateOutcome {
            command: None,
            errored: false,
            skipped: true,
        },
        Err(e) => {
            let error_message = e.to_string();
            let is_llm_budget_exceeded = matches!(&e, ScraperError::LlmBudgetExceeded { .. });
            let is_pending_schema_review = matches!(&e, ScraperError::PendingSchemaReview { .. });

            if let ScraperError::HttpError { kind, .. } = &e {
                let cooldown = durable_retry_cooldown_for(*kind);
                let next_retry_at = time::OffsetDateTime::now_utc()
                    + time::Duration::seconds(cooldown.as_secs() as i64);
                let status_code = match kind {
                    NetworkErrorKind::HttpStatus(code) => Some(*code as i32),
                    _ => None,
                };
                if let Err(mark_err) = ctx
                    .scraper_candidates
                    .mark_fetch_failure(
                        &candidate.listing_source_id,
                        &candidate.url,
                        &format!("{kind:?}"),
                        &error_message,
                        status_code,
                        next_retry_at,
                    )
                    .await
                {
                    warn!(
                        error = %mark_err,
                        "Failed to persist scraper fetch failure metadata"
                    );
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
                        if let Err(mark_err) = ctx
                            .scraper_candidates
                            .mark_fetch_failure(
                                &candidate.listing_source_id,
                                &candidate.url,
                                error_kind,
                                &error_message,
                                None,
                                next_retry_at,
                            )
                            .await
                        {
                            warn!(
                                error = %mark_err,
                                "Failed to persist schema/classification cooldown metadata"
                            );
                        }
                    }
                    _ => {
                        if let Err(mark_err) = ctx
                            .scraper_candidates
                            .mark_scraper_failure(
                                &candidate.listing_source_id,
                                &candidate.url,
                                error_kind,
                                &error_message,
                            )
                            .await
                        {
                            warn!(
                                error = %mark_err,
                                "Failed to persist scraper failure metadata"
                            );
                        }
                    }
                }
            }

            // Log LLM budget exhaustion at INFO level only once per shop per batch
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
                            "LLM call budget exhausted for shop; skipping remaining URLs in batch"
                        );
                    }
                }
            } else if is_pending_schema_review {
                let mut pending = ctx.schema_pending_listing_sources.lock().await;
                if pending.insert(candidate.listing_source_id) {
                    info!(
                        listing_source_id = %candidate.listing_source_id,
                        "ProductListing schema review pending for shop; skipping remaining URLs in batch"
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
                command: None,
                errored: true,
                skipped: false,
            }
        }
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
        ScraperError::HttpError { .. } => "HttpError",
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
        ScraperError::PendingSchemaReview { .. } => "PendingSchemaReview",
    }
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

    for candidate in candidates {
        let candidate_outcome = scrape_candidate(candidate, &ctx).await;

        if candidate_outcome.errored {
            outcome.failed += 1;
        } else if let Some(pair) = candidate_outcome.command {
            match enqueue_product_push(&ctx.command_tx, pair).await {
                Ok(queue_wait) => {
                    outcome.succeeded += 1;

                    if queue_wait >= Duration::from_millis(10) {
                        warn!(
                            event = "crawler.product_push.enqueue_wait",
                            queue_wait_ms = queue_wait.as_millis(),
                            "ProductListing push queue applied backpressure"
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
    }

    outcome
}

async fn run_push_collector(
    mut command_rx: mpsc::Receiver<QueuedProductListingPush>,
    push_service: Arc<dyn ProductListingPushService>,
    scraper_candidates: Arc<dyn ScraperCandidateService>,
    push_batch_size: usize,
    push_max_batch_age: Duration,
) {
    let push_batch_size = push_batch_size.max(1);
    let push_max_batch_age = push_max_batch_age.max(Duration::from_millis(1));
    let mut pending = Vec::<QueuedProductListingPush>::with_capacity(push_batch_size);

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
            let flush_deadline = oldest_enqueued_at + push_max_batch_age;

            tokio::select! {
                received = command_rx.recv() => {
                    match received {
                        Some(item) => pending.push(item),
                        None => {
                            let batch = std::mem::take(&mut pending);
                            flush_batch(
                                &push_service,
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
                        &push_service,
                        &scraper_candidates,
                        batch,
                        command_rx.len(),
                    )
                    .await;
                }
            }
        }

        if pending.len() >= push_batch_size {
            let batch = std::mem::take(&mut pending);
            flush_batch(&push_service, &scraper_candidates, batch, command_rx.len()).await;
        }
    }

    if !pending.is_empty() {
        flush_batch(
            &push_service,
            &scraper_candidates,
            pending,
            command_rx.len(),
        )
        .await;
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

        let pass_start = tokio::time::Instant::now();
        info!(
            concurrency = scraper_concurrency,
            push_batch_size = self.config.effective_push_batch_size(),
            push_queue_capacity = self.config.effective_push_queue_capacity(),
            push_max_batch_age_ms = self.config.effective_push_max_batch_age().as_millis(),
            push_max_concurrency = self.config.effective_push_max_concurrency(),
            "Scraper scheduler pass starting"
        );
        let mut seen_domains: HashSet<String> = HashSet::new();
        let mut active_domains: HashSet<String> = HashSet::new();
        let mut pending_domains: VecDeque<(String, Vec<ScraperCandidate>)> = VecDeque::new();
        let mut join_set: JoinSet<ScheduledScrapeDomainOutcome> = JoinSet::new();
        let (command_tx, command_rx) =
            mpsc::channel::<QueuedProductListingPush>(self.config.effective_push_queue_capacity());

        let budget_exhausted_listing_sources = Arc::new(Mutex::new(HashSet::new()));
        let schema_pending_listing_sources = Arc::new(Mutex::new(HashSet::new()));

        let mut unique_listing_source_ids = HashSet::new();
        let mut total = 0usize;
        let mut succeeded = 0usize;
        let mut failed = 0usize;
        let mut skipped = 0usize;
        let mut started = false;
        let mut no_more_candidates = false;

        let push_collector = tokio::spawn(run_push_collector(
            command_rx,
            Arc::clone(&self.product_push),
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
                    let span = tracing::info_span!("scrape_domain", domain = %domain);
                    active_domains.insert(domain.clone());
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
                            };

                            ScheduledScrapeDomainOutcome {
                                domain,
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

                let mut excluded_domains: HashSet<String> = seen_domains.clone();
                excluded_domains.extend(active_domains.iter().cloned());
                excluded_domains.extend(
                    pending_domains
                        .iter()
                        .map(|(domain, _)| domain.to_ascii_lowercase()),
                );
                let excluded_domains: Vec<String> = excluded_domains.into_iter().collect();
                let candidates = match self
                    .scraper_candidates
                    .get_candidates(
                        self.config.effective_scraper_domain_batch_size() as i64,
                        self.config.scraper_urls_per_domain.max(1),
                        &excluded_domains,
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
                        if let Err(e) = push_collector.await {
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

                let mut by_domain: HashMap<String, Vec<ScraperCandidate>> = HashMap::new();
                for candidate in candidates {
                    unique_listing_source_ids.insert(candidate.listing_source_id);
                    let domain = candidate.url.host_str().unwrap_or("").to_ascii_lowercase();
                    seen_domains.insert(domain.clone());
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
                    active_domains.remove(&scheduled.domain);
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

        if let Err(e) = push_collector.await {
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
                            "Shop LLM usage summary"
                        );
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Failed to load per-shop LLM usage summary");
                }
            }
        }

        self.scraper_perf.record(total as u64, duration_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::candidate_service::MockScraperCandidateService;
    use crate::scraper::scraper_service::MockScraperService;
    use crate::service::cron::config::CrawlerCronConfig;
    use crate::service::cron::test_support::{noop_listing_source_registration, scraper_candidate};
    use crate::service::product_push::MockProductListingPushService;
    use crate::spider::advisory_lock::LocalLockManager;
    use crate::spider::candidate_service::MockSpiderCandidateService;
    use crate::spider::service::MockSpiderService;
    use listing_source_core::ListingSourceId;
    use product_listing_core::source_listing_id::SourceListingId;
    use product_listing_service::use_cases::commands::upsert_product_listing::UpsertProductListingCommand;
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

    fn no_push_service() -> Box<MockProductListingPushService> {
        let mut push_service = MockProductListingPushService::new();
        push_service.expect_push().times(0);
        Box::new(push_service)
    }

    fn item(listing_source_id: ListingSourceId, product_id: &str) -> ProductListingPushItem {
        ProductListingPushItem {
            command: UpsertProductListingCommand {
                listing_source_id: ListingSourceId::from(uuid::Uuid::from(listing_source_id)),
                source_listing_id: SourceListingId::from(product_id),
                title: None,
                description: None,
                price: application::patch_field::PatchField::Unchanged,
                price_estimate_min: application::patch_field::PatchField::Unchanged,
                price_estimate_max: application::patch_field::PatchField::Unchanged,
                availability: application::patch_field::PatchField::Unchanged,
                url: None,
                images: application::patch_field::PatchField::Unchanged,
                auction_start: application::patch_field::PatchField::Unchanged,
                auction_end: application::patch_field::PatchField::Unchanged,
            },
            raw_attributes: Default::default(),
        }
    }

    fn meta(listing_source_id: ListingSourceId, url: &str, hash: &str) -> CandidateMeta {
        CandidateMeta {
            listing_source_id,
            url: url::Url::parse(url).unwrap(),
            hash: hash.to_owned(),
            snapshot: ProductListingSnapshot {
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                url: url.to_owned(),
                images_hash: String::new(),
                auction_start: None,
                auction_end: None,
                availability: Some("AVAILABLE".to_owned()),
            },
        }
    }

    #[tokio::test]
    async fn should_mark_only_the_matching_successful_push_input_as_scraped() {
        let first_listing_source_id = ListingSourceId::new();
        let second_listing_source_id = ListingSourceId::new();
        let first_url = url::Url::parse("https://first.example/product").unwrap();
        let mut push_service = MockProductListingPushService::new();
        push_service.expect_push().once().returning(|products| {
            assert_eq!(products.len(), 2);
            Box::pin(async { vec![true, false] })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_as_scraped()
            .once()
            .withf(move |listing_source_id, url, hash, _| {
                *listing_source_id == first_listing_source_id
                    && url == &first_url
                    && hash == "first"
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let push_service: Arc<dyn ProductListingPushService> = Arc::new(push_service);
        let scraper_candidates: Arc<dyn ScraperCandidateService> = Arc::new(scraper_candidates);

        flush_batch(
            &push_service,
            &scraper_candidates,
            vec![
                QueuedProductListingPush {
                    item: item(first_listing_source_id, "same-product-id"),
                    meta: meta(
                        first_listing_source_id,
                        "https://first.example/product",
                        "first",
                    ),
                    enqueued_at: tokio::time::Instant::now(),
                },
                QueuedProductListingPush {
                    item: item(second_listing_source_id, "same-product-id"),
                    meta: meta(
                        second_listing_source_id,
                        "https://second.example/product",
                        "second",
                    ),
                    enqueued_at: tokio::time::Instant::now(),
                },
            ],
            0,
        )
        .await;
    }

    #[tokio::test]
    async fn should_default_missing_product_push_results_to_failure() {
        let first_listing_source_id = ListingSourceId::new();
        let second_listing_source_id = ListingSourceId::new();
        let first_url = url::Url::parse("https://first.example/product").unwrap();
        let mut push_service = MockProductListingPushService::new();
        push_service.expect_push().once().returning(|products| {
            assert_eq!(products.len(), 2);
            Box::pin(async { vec![true] })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_as_scraped()
            .once()
            .withf(move |listing_source_id, url, hash, _| {
                *listing_source_id == first_listing_source_id
                    && url == &first_url
                    && hash == "first"
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let push_service: Arc<dyn ProductListingPushService> = Arc::new(push_service);
        let scraper_candidates: Arc<dyn ScraperCandidateService> = Arc::new(scraper_candidates);

        flush_batch(
            &push_service,
            &scraper_candidates,
            vec![
                QueuedProductListingPush {
                    item: item(first_listing_source_id, "first"),
                    meta: meta(
                        first_listing_source_id,
                        "https://first.example/product",
                        "first",
                    ),
                    enqueued_at: tokio::time::Instant::now(),
                },
                QueuedProductListingPush {
                    item: item(second_listing_source_id, "second"),
                    meta: meta(
                        second_listing_source_id,
                        "https://second.example/product",
                        "second",
                    ),
                    enqueued_at: tokio::time::Instant::now(),
                },
            ],
            0,
        )
        .await;
    }

    #[tokio::test]
    async fn should_apply_backpressure_when_product_push_queue_is_full() {
        let (command_tx, mut command_rx) = mpsc::channel::<QueuedProductListingPush>(1);
        let listing_source_id = ListingSourceId::new();

        enqueue_product_push(
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
            enqueue_product_push(
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

        let mut push_service = MockProductListingPushService::new();
        push_service
            .expect_push()
            .once()
            .returning(move |products| {
                push_calls_for_mock.fetch_add(1, Ordering::SeqCst);
                let len = products.len();
                Box::pin(async move { vec![true; len] })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_as_scraped()
            .once()
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let (tx, rx) = mpsc::channel(2);
        let collector = tokio::spawn(run_push_collector(
            rx,
            Arc::new(push_service),
            Arc::new(scraper_candidates),
            10,
            Duration::from_secs(5),
        ));

        let listing_source_id = ListingSourceId::new();
        enqueue_product_push(
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
    async fn should_flush_final_partial_batch_when_product_push_channel_closes() {
        let push_calls = Arc::new(AtomicUsize::new(0));
        let push_calls_for_mock = Arc::clone(&push_calls);

        let mut push_service = MockProductListingPushService::new();
        push_service
            .expect_push()
            .once()
            .returning(move |products| {
                assert_eq!(products.len(), 2);
                push_calls_for_mock.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { vec![true, true] })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_as_scraped()
            .times(2)
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let (tx, rx) = mpsc::channel(2);
        let collector = tokio::spawn(run_push_collector(
            rx,
            Arc::new(push_service),
            Arc::new(scraper_candidates),
            10,
            Duration::from_secs(5),
        ));

        let listing_source_id = ListingSourceId::new();
        enqueue_product_push(
            &tx,
            (
                item(listing_source_id, "one"),
                meta(listing_source_id, "https://example.com/product/one", "one"),
            ),
        )
        .await
        .expect("first enqueue must succeed");
        enqueue_product_push(
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
    ) -> impl Fn(i64, i64, &[String]) -> ScraperCandidateResultFuture + Send + Sync + 'static
    where
        F: Fn() -> Vec<ScraperCandidate> + Send + Sync + 'static,
    {
        move |_, _, excluded_domains| {
            let excluded_domains: HashSet<String> = excluded_domains.iter().cloned().collect();
            let candidates = build_candidates();
            Box::pin(async move {
                Ok(candidates
                    .into_iter()
                    .filter(|candidate| {
                        candidate
                            .url
                            .host_str()
                            .map(|domain| !excluded_domains.contains(&domain.to_ascii_lowercase()))
                            .unwrap_or(false)
                    })
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
            no_push_service(),
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
        }
    }

    #[tokio::test]
    async fn should_run_scraper_candidates_and_push_products() {
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(|| {
                vec![scraper_candidate(
                    "Test Shop",
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )]
            }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .returning(|_, _, _, _| Box::pin(async { Ok(None) }));

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_apply_durable_retry_cooldown_after_final_fetch_failure() {
        let before = time::OffsetDateTime::now_utc();
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(|| {
                vec![scraper_candidate(
                    "Test Shop",
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )]
            }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(move |_, _, _, _, status_code, next_retry_at| {
                let expected_cooldown = durable_retry_cooldown_for(NetworkErrorKind::Timeout);
                let expected_from =
                    before + time::Duration::seconds(expected_cooldown.as_secs() as i64);
                let expected_until = time::OffsetDateTime::now_utc()
                    + time::Duration::seconds(expected_cooldown.as_secs() as i64)
                    + time::Duration::seconds(1);
                status_code.is_none()
                    && *next_retry_at >= expected_from
                    && *next_retry_at <= expected_until
            })
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().returning(|_, url, _, _| {
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
    }

    #[tokio::test]
    async fn should_mark_same_domain_500_fetch_failure() {
        let url = url::Url::parse("https://same-domain.com/product/1").unwrap();
        let candidate = scraper_candidate("Shop", url.clone());
        let listing_source_id = candidate.listing_source_id;

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(
                move |received_listing_source_id, received_url, _, _, status_code, _| {
                    *received_listing_source_id == listing_source_id
                        && received_url == &url
                        && *status_code == Some(500)
                },
            )
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .once()
            .returning(|_, url, _, _| {
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
        let candidate = scraper_candidate("Shop", url.clone());
        let listing_source_id = candidate.listing_source_id;

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(
                move |received_listing_source_id, received_url, _, _, status_code, _| {
                    *received_listing_source_id == listing_source_id
                        && received_url == &url
                        && *status_code == Some(429)
                },
            )
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .once()
            .returning(|_, url, _, _| {
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
                    scraper_candidate("Shop", first_candidate_url),
                    scraper_candidate("Shop", second_candidate_url),
                ]
            }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(move |_, received_url, _, _, status_code, _| {
                received_url == &first_url && *status_code == Some(500)
            })
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(()) }));

        let scrape_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let scrape_count_for_mock = Arc::clone(&scrape_count);
        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(2)
            .returning(move |_, url, _, _| {
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
                        Ok(None)
                    }
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

        assert_eq!(scrape_count.load(Ordering::SeqCst), 2);
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
                    scraper_candidate("Shop", first_candidate_url),
                    scraper_candidate("Shop", second_candidate_url),
                ]
            }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(move |_, received_url, _, _, status_code, _| {
                received_url == &first_url && *status_code == Some(429)
            })
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(()) }));

        let scrape_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let scrape_count_for_mock = Arc::clone(&scrape_count);
        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(2)
            .returning(move |_, url, _, _| {
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
        assert_eq!(scrape_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn should_mark_fetch_failure_for_llm_budget_exceeded_error() {
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(|| {
                vec![scraper_candidate(
                    "Test Shop",
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )]
            }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .returning(|listing_source_id, url, _, _| {
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
    async fn should_skip_remaining_shop_candidates_when_schema_review_is_pending() {
        let listing_source_id = ListingSourceId::new();
        let first_url = url::Url::parse("https://example.com/product/1").unwrap();
        let second_url = url::Url::parse("https://example.com/product/2").unwrap();

        let first_url_for_candidates = first_url.clone();
        let second_url_for_candidates = second_url.clone();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(move || {
                let mut first = scraper_candidate("Test Shop", first_url_for_candidates.clone());
                first.listing_source_id = listing_source_id;
                let mut second = scraper_candidate("Test Shop", second_url_for_candidates.clone());
                second.listing_source_id = listing_source_id;
                vec![first, second]
            }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(
                move |received_listing_source_id, received_url, kind, _, _, _| {
                    *received_listing_source_id == listing_source_id
                        && received_url == &first_url
                        && kind == "PendingSchemaReview"
                },
            )
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .once()
            .returning(|_, url, _, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::PendingSchemaReview {
                        url,
                        review_id: uuid::Uuid::new_v4(),
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
                    "Test Shop",
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )]
            }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .returning(|_, url, _, _| {
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
                move || vec![scraper_candidate("Test Shop", url.clone())]
            }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(
                move |_, received_url, kind, _, status_code, next_retry_at| {
                    *received_url == url
                        && kind == "SchemaClassificationRejected"
                        && status_code.is_none()
                        && *next_retry_at > before
                },
            )
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(()) }));
        scraper_candidates.expect_mark_scraper_failure().never();

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .once()
            .returning(|_, url, _, _| {
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
                        "Shop A",
                        url::Url::parse("https://domain-a.com/product/1").unwrap(),
                    ),
                    scraper_candidate(
                        "Shop B",
                        url::Url::parse("https://domain-b.com/product/2").unwrap(),
                    ),
                ]
            }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(2)
            .returning(|_, _, _, _| Box::pin(async { Ok(None) }));

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
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(move |_, _, excluded_domains| {
                let excluded_domains = excluded_domains.to_vec();
                let slow_url = slow_url_for_candidates.clone();
                let fast_url = fast_url_for_candidates.clone();
                let refill_url = refill_url_for_candidates.clone();
                Box::pin(async move {
                    if excluded_domains.is_empty() {
                        Ok(vec![
                            scraper_candidate("Slow", slow_url),
                            scraper_candidate("Fast", fast_url),
                        ])
                    } else if excluded_domains.contains(&"domain-a.com".to_string())
                        && excluded_domains.contains(&"domain-b.com".to_string())
                        && !excluded_domains.contains(&"domain-c.com".to_string())
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
            .returning(move |_, url, _, _| {
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
    async fn should_skip_same_shop_candidate_already_scraping_on_another_domain() {
        let listing_source_id = ListingSourceId::new();
        let first_url = url::Url::parse("https://domain-a.com/product/1").unwrap();
        let second_url = url::Url::parse("https://domain-b.com/product/2").unwrap();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(move || {
                let mut first = scraper_candidate("Same Shop", first_url.clone());
                first.listing_source_id = listing_source_id;
                let mut second = scraper_candidate("Same Shop", second_url.clone());
                second.listing_source_id = listing_source_id;
                vec![first, second]
            }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .once()
            .returning(|_, _, _, _| {
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
                    scraper_candidate("Shop A", locked_url),
                    scraper_candidate("Shop A", open_url),
                ]
            }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(1)
            .returning(|_, _, _, _| Box::pin(async { Ok(None) }));

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
            no_push_service(),
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
                        "Shop",
                        url::Url::parse("https://same-domain.com/product/1").unwrap(),
                    ),
                    scraper_candidate(
                        "Shop",
                        url::Url::parse("https://same-domain.com/product/2").unwrap(),
                    ),
                    scraper_candidate(
                        "Shop",
                        url::Url::parse("https://same-domain.com/product/3").unwrap(),
                    ),
                ]
            }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(3)
            .returning(|_, _, _, _| Box::pin(async { Ok(None) }));

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }
}
