use super::backoff::{ADAPTIVE_DOMAIN_SUCCESSES_BEFORE_DECAY, AdaptiveDomainBackoff};
use super::job::CrawlerCronJob;
use crate::network::policy::{NetworkErrorKind, is_retryable_network_failure, retry_cooldown_for};
use crate::scraper::candidate_service::{
    ProductSnapshot, ScraperCandidate, ScraperCandidateService,
};
use crate::scraper::scraper_service::{ScraperError, ScraperService};
use crate::service::product_push::{ProductPushService, normalize_to_upsert};
use crate::spider::advisory_lock::{LocalLockManager, ShopLock, UrlLock};
use common::shop_id::ShopId;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinSet;
use tracing::{Instrument, debug, error, info, warn};

/// Context for scraping a domain's candidates.
struct ScrapeDomainContext {
    scraper: Arc<dyn ScraperService>,
    scraper_candidates: Arc<dyn ScraperCandidateService>,
    lock_manager: Arc<LocalLockManager>,
    domain_delay: Duration,
    command_tx: mpsc::UnboundedSender<(
        product::service::product_command::UpsertProductCommand,
        CandidateMeta,
    )>,
    budget_exhausted_shops: Arc<Mutex<HashSet<ShopId>>>,
    schema_pending_shops: Arc<Mutex<HashSet<ShopId>>>,
}

/// Metadata carried alongside a [`UpsertProductCommand`] so the push-collector
/// can call [`ScraperCandidateService::mark_as_scraped`] only after the push
/// has been confirmed.
struct CandidateMeta {
    shop_id: common::shop_id::ShopId,
    url: url::Url,
    hash: String,
    snapshot: ProductSnapshot,
}

struct ScrapeCandidateOutcome {
    command: Option<(
        product::service::product_command::UpsertProductCommand,
        CandidateMeta,
    )>,
    errored: bool,
    skipped: bool,
    retryable_network_error: Option<NetworkErrorKind>,
}

struct ScrapeDomainOutcome {
    succeeded: usize,
    failed: usize,
    skipped: usize,
}

/// Pushes a batch of `(command, meta)` pairs to the product backend and then
/// calls [`ScraperCandidateService::mark_as_scraped`] for each command that
/// was successfully persisted.
///
/// The position correspondence between `commands` and `metas` is preserved so
/// that succeeded commands can be matched back to their metadata by index.
#[tracing::instrument(
    name = "crawler_flush_push_batch",
    skip(push_service, scraper_candidates, batch),
    fields(batch_size = batch.len())
)]
async fn flush_batch(
    push_service: &Arc<dyn ProductPushService>,
    scraper_candidates: &Arc<dyn ScraperCandidateService>,
    batch: Vec<(
        product::service::product_command::UpsertProductCommand,
        CandidateMeta,
    )>,
) {
    let (commands, metas): (Vec<_>, Vec<_>) = batch.into_iter().unzip();

    // Keep a copy of shops_product_ids in order so we can re-match after push.
    let ids_in_order: Vec<String> = commands
        .iter()
        .map(|c| c.shops_product_id.to_string())
        .collect();

    let succeeded = push_service.push(commands).await;
    let succeeded_ids: std::collections::HashSet<String> = succeeded
        .iter()
        .map(|c| c.shops_product_id.to_string())
        .collect();

    for (i, meta) in metas.into_iter().enumerate() {
        if succeeded_ids.contains(&ids_in_order[i])
            && let Err(e) = scraper_candidates
                .mark_as_scraped(&meta.shop_id, &meta.url, &meta.hash, &meta.snapshot)
                .await
        {
            warn!(shop_id = %meta.shop_id, error = %e, url = %meta.url, "Failed to mark product as scraped after push");
        }
    }
}

#[tracing::instrument(
    name = "crawler_scrape_candidate",
    skip(candidate, domain, ctx),
    fields(
        shop_id = %candidate.shop_id,
        domain = %domain,
        url = %candidate.url
    )
)]
async fn scrape_candidate(
    candidate: ScraperCandidate,
    domain: String,
    ctx: &ScrapeDomainContext,
) -> ScrapeCandidateOutcome {
    // Skip URLs from shops with already-exhausted budgets
    {
        let exhausted = ctx.budget_exhausted_shops.lock().await;
        if exhausted.contains(&candidate.shop_id) {
            debug!("Skipping URL — shop LLM budget already exhausted in this batch");
            return ScrapeCandidateOutcome {
                command: None,
                errored: false,
                skipped: true,
                retryable_network_error: None,
            };
        }
    }

    {
        let pending = ctx.schema_pending_shops.lock().await;
        if pending.contains(&candidate.shop_id) {
            debug!("Skipping URL because shop has pending schema review in this batch");
            return ScrapeCandidateOutcome {
                command: None,
                errored: false,
                skipped: true,
                retryable_network_error: None,
            };
        }
    }

    let Some(_lock) = UrlLock::try_acquire(&ctx.lock_manager, &candidate.url) else {
        warn!("Skipping URL — lock held by another worker");
        return ScrapeCandidateOutcome {
            command: None,
            errored: false,
            skipped: true,
            retryable_network_error: None,
        };
    };

    let Some(_shop_lock) = ShopLock::try_acquire(&ctx.lock_manager, candidate.shop_id) else {
        debug!("Skipping URL because another worker is scraping this shop");
        return ScrapeCandidateOutcome {
            command: None,
            errored: false,
            skipped: true,
            retryable_network_error: None,
        };
    };

    match ctx
        .scraper
        .scrape(
            &candidate.shop_id,
            &candidate.url,
            candidate.last_scraped_hash.as_deref(),
        )
        .await
    {
        Ok(Some(scraped)) => {
            let meta = CandidateMeta {
                shop_id: candidate.shop_id,
                url: candidate.url.clone(),
                hash: scraped.hash,
                snapshot: scraped.snapshot,
            };
            ScrapeCandidateOutcome {
                command: normalize_to_upsert(scraped.product, &candidate).map(|cmd| (cmd, meta)),
                errored: false,
                skipped: false,
                retryable_network_error: None,
            }
        }
        Ok(None) => ScrapeCandidateOutcome {
            command: None,
            errored: false,
            skipped: true,
            retryable_network_error: None,
        },
        Err(e) => {
            let error_message = e.to_string();
            let is_llm_budget_exceeded = matches!(&e, ScraperError::LlmBudgetExceeded { .. });
            let is_pending_schema_review = matches!(&e, ScraperError::PendingSchemaReview { .. });
            let retryable_network_error = match &e {
                ScraperError::HttpError { kind, .. } if is_retryable_network_failure(*kind) => {
                    Some(*kind)
                }
                _ => None,
            };

            if let ScraperError::HttpError { kind, .. } = &e {
                let cooldown = retry_cooldown_for(*kind);
                let next_retry_at = time::OffsetDateTime::now_utc()
                    + time::Duration::seconds(cooldown.as_secs() as i64);
                let status_code = match kind {
                    NetworkErrorKind::HttpStatus(code) => Some(*code as i32),
                    _ => None,
                };
                if let Err(mark_err) = ctx
                    .scraper_candidates
                    .mark_fetch_failure(
                        &candidate.shop_id,
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
                    | ScraperError::NormalizationFixExhausted { .. }
                    | ScraperError::LlmBudgetExceeded { .. }
                    | ScraperError::PendingSchemaReview { .. } => {
                        let cooldown = std::time::Duration::from_secs(30 * 60);
                        let next_retry_at = time::OffsetDateTime::now_utc()
                            + time::Duration::seconds(cooldown.as_secs() as i64);
                        if let Err(mark_err) = ctx
                            .scraper_candidates
                            .mark_fetch_failure(
                                &candidate.shop_id,
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
                                "Failed to persist schema-regeneration/normalization-fix/LLM-budget cooldown metadata"
                            );
                        }
                    }
                    _ => {
                        if let Err(mark_err) = ctx
                            .scraper_candidates
                            .mark_scraper_failure(
                                &candidate.shop_id,
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
                    shop_id, max_calls, ..
                } = &e
                {
                    let mut exhausted = ctx.budget_exhausted_shops.lock().await;
                    if exhausted.insert(*shop_id) {
                        info!(
                            shop_id = %shop_id,
                            max_calls,
                            "LLM call budget exhausted for shop; skipping remaining URLs in batch"
                        );
                    }
                }
            } else if is_pending_schema_review {
                let mut pending = ctx.schema_pending_shops.lock().await;
                if pending.insert(candidate.shop_id) {
                    info!(
                        shop_id = %candidate.shop_id,
                        "Product schema review pending for shop; skipping remaining URLs in batch"
                    );
                }
                warn!(error = %e, "Scraper run failed");
            } else {
                warn!(error = %e, "Scraper run failed");
            }

            ScrapeCandidateOutcome {
                command: None,
                errored: true,
                skipped: false,
                retryable_network_error,
            }
        }
    }
}

/// Returns a short, stable, machine-readable kind label for a [`ScraperError`].
///
/// These labels are persisted in `shop_urls.last_error_kind` so that
/// operators can filter / aggregate by error category without having to parse
/// the free-text message.  The `HttpError` variant is included for completeness
/// even though the caller currently only invokes this helper for non-HTTP errors.
fn scraper_error_kind(e: &ScraperError) -> &'static str {
    match e {
        ScraperError::HttpError { .. } => "HttpError",
        ScraperError::ProductRemoved { .. } => "ProductRemoved",
        ScraperError::NoHost { .. } => "NoHost",
        ScraperError::SchemaServiceError(_) => "SchemaServiceError",
        ScraperError::SchemaRegenerationExhausted { .. } => "SchemaRegenerationExhausted",
        ScraperError::NormalizationFixExhausted { .. } => "NormalizationFixExhausted",
        ScraperError::LlmBudgetExceeded { .. } => "LlmBudgetExceeded",
        ScraperError::NormalizationError(_) => "NormalizationError",
        ScraperError::PendingSchemaReview { .. } => "PendingSchemaReview",
    }
}

#[tracing::instrument(
    name = "crawler_scrape_domain_candidates",
    skip(candidates, ctx),
    fields(domain = %domain, candidate_count = candidates.len())
)]
async fn scrape_domain_candidates(
    domain: String,
    candidates: Vec<ScraperCandidate>,
    ctx: ScrapeDomainContext,
) -> ScrapeDomainOutcome {
    let mut outcome = ScrapeDomainOutcome {
        succeeded: 0,
        failed: 0,
        skipped: 0,
    };
    let mut domain_backoff = AdaptiveDomainBackoff::new(ctx.domain_delay);

    let len = candidates.len();
    for (idx, candidate) in candidates.into_iter().enumerate() {
        let url = candidate.url.clone();
        let candidate_outcome = scrape_candidate(candidate, domain.clone(), &ctx).await;

        if candidate_outcome.errored {
            outcome.failed += 1;
        } else if let Some(pair) = candidate_outcome.command {
            outcome.succeeded += 1;
            if ctx.command_tx.send(pair).is_err() {
                error!("Command channel closed while scraper worker is running");
                outcome.failed += 1;
                outcome.succeeded = outcome.succeeded.saturating_sub(1);
            }
        } else if candidate_outcome.skipped {
            outcome.skipped += 1;
        } else {
            outcome.succeeded += 1;
        }

        if let Some(kind) = candidate_outcome.retryable_network_error {
            let (previous_delay, new_delay) = domain_backoff.record_retryable_failure();
            info!(
                domain = %domain,
                url = %url,
                error_kind = ?kind,
                previous_delay_ms = previous_delay.as_millis(),
                new_delay_ms = new_delay.as_millis(),
                "Increased scraper domain delay after retryable network failure"
            );
        } else if !candidate_outcome.errored
            && let Some((previous_delay, new_delay)) = domain_backoff.record_clean_outcome()
        {
            info!(
                domain = %domain,
                url = %url,
                previous_delay_ms = previous_delay.as_millis(),
                new_delay_ms = new_delay.as_millis(),
                successes_before_decay = ADAPTIVE_DOMAIN_SUCCESSES_BEFORE_DECAY,
                "Reduced scraper domain delay after sustained domain recovery"
            );
        }

        let current_delay = domain_backoff.current_delay();
        if idx + 1 < len && !current_delay.is_zero() {
            tokio::time::sleep(current_delay).await;
        }
    }

    outcome
}

impl CrawlerCronJob {
    #[tracing::instrument(name = "crawler_run_scraper_once", skip(self))]
    pub(super) async fn run_scraper_once(&self) {
        let total_fetch = (self.config.scraper_concurrency as i64) * self.config.scraper_batch_size;

        let all_candidates = match self.scraper_candidates.get_candidates(total_fetch).await {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "Failed to retrieve scraper candidates");
                return;
            }
        };

        if all_candidates.is_empty() {
            debug!("No scraper candidates, skipping batch");
            return;
        }

        let total = all_candidates.len();
        let mut unique_shop_ids = std::collections::HashSet::new();
        for candidate in &all_candidates {
            unique_shop_ids.insert(candidate.shop_id);
        }
        let batch_start = tokio::time::Instant::now();
        let scraper_concurrency = self.config.scraper_concurrency;
        if scraper_concurrency == 0 {
            warn!(
                scraper_concurrency,
                "scraper_concurrency is 0, skipping scraper batch"
            );
            return;
        }

        info!(
            candidates = total,
            concurrency = scraper_concurrency,
            "Scraper batch starting"
        );

        let mut by_domain: HashMap<String, Vec<ScraperCandidate>> = HashMap::new();
        for candidate in all_candidates {
            let domain = candidate.url.host_str().unwrap_or("").to_string();
            by_domain.entry(domain).or_default().push(candidate);
        }

        debug!(domains = by_domain.len(), "Candidates grouped by domain");

        let domain_delay = self.config.scraper_domain_delay;
        let semaphore = Arc::new(Semaphore::new(scraper_concurrency));
        let mut join_set: JoinSet<ScrapeDomainOutcome> = JoinSet::new();
        let (command_tx, mut command_rx) = mpsc::unbounded_channel::<(
            product::service::product_command::UpsertProductCommand,
            CandidateMeta,
        )>();

        // Track shops with exhausted LLM budgets to avoid repeated logging
        let budget_exhausted_shops = Arc::new(Mutex::new(HashSet::new()));
        let schema_pending_shops = Arc::new(Mutex::new(HashSet::new()));

        let mut succeeded = 0usize;
        let mut failed = 0usize;
        let mut skipped = 0usize;
        let push_batch_size = self.config.push_batch_size;
        let push_service = Arc::clone(&self.product_push);
        let scraper_candidates_push = Arc::clone(&self.scraper_candidates);

        let push_collector = tokio::spawn(async move {
            let mut pending: Vec<(
                product::service::product_command::UpsertProductCommand,
                CandidateMeta,
            )> = Vec::new();

            while let Some(pair) = command_rx.recv().await {
                pending.push(pair);
                if pending.len() >= push_batch_size {
                    let batch = std::mem::take(&mut pending);
                    flush_batch(&push_service, &scraper_candidates_push, batch).await;
                }
            }

            if !pending.is_empty() {
                flush_batch(&push_service, &scraper_candidates_push, pending).await;
            }
        });

        for (domain, candidates) in by_domain {
            let scraper = Arc::clone(&self.scraper_service);
            let scraper_candidates = Arc::clone(&self.scraper_candidates);
            let lock_manager = Arc::clone(&self.lock_manager);
            let permit_pool = Arc::clone(&semaphore);
            let domain_tx = command_tx.clone();
            let budget_exhausted_shops = Arc::clone(&budget_exhausted_shops);
            let schema_pending_shops = Arc::clone(&schema_pending_shops);
            let span = tracing::info_span!(
                "scrape_domain",
                domain = %domain
            );

            join_set.spawn(
                async move {
                    let Ok(_permit) = permit_pool.acquire_owned().await else {
                        error!("Scraper semaphore closed unexpectedly");
                        return ScrapeDomainOutcome {
                            succeeded: 0,
                            failed: 1,
                            skipped: 0,
                        };
                    };

                    let ctx = ScrapeDomainContext {
                        scraper,
                        scraper_candidates,
                        lock_manager,
                        domain_delay,
                        command_tx: domain_tx,
                        budget_exhausted_shops,
                        schema_pending_shops,
                    };

                    scrape_domain_candidates(domain, candidates, ctx).await
                }
                .instrument(span),
            );
        }
        drop(command_tx);

        while let Some(joined) = join_set.join_next().await {
            let outcome = match joined {
                Ok(outcome) => outcome,
                Err(e) => {
                    error!(error = %e, "Scraper domain worker task failed to join");
                    failed += 1;
                    continue;
                }
            };

            succeeded += outcome.succeeded;
            failed += outcome.failed;
            skipped += outcome.skipped;
        }

        if let Err(e) = push_collector.await {
            error!(error = %e, "Scraper push collector task failed to join");
            failed += 1;
        }

        let duration_ms = batch_start.elapsed().as_millis() as u64;
        skipped += total.saturating_sub(succeeded + failed + skipped);
        info!(
            total,
            succeeded, failed, skipped, duration_ms, "Scraper batch complete"
        );

        #[cfg(not(test))]
        {
            match self
                .scraper_candidates
                .get_shop_llm_usage(unique_shop_ids.into_iter().collect())
                .await
            {
                Ok(usages) => {
                    for usage in usages {
                        info!(
                            shop_name = %usage.shop_name,
                            llm_calls_count = usage.llm_calls_count,
                            llm_calls_cap = self.config.scraper_max_llm_calls_per_shop,
                            llm_budget_exhausted = usage.llm_calls_count >= self.config.scraper_max_llm_calls_per_shop,
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
    use crate::service::cron::test_support::{noop_shop_registration, scraper_candidate};
    use crate::service::product_push::MockProductPushService;
    use crate::spider::advisory_lock::LocalLockManager;
    use crate::spider::candidate_service::MockSpiderCandidateService;
    use crate::spider::service::MockSpiderService;
    use common::shop_id::ShopId;
    use shop::core::shop_type::ShopType;
    use std::sync::atomic::Ordering;

    #[tokio::test]
    async fn should_run_scraper_candidates_and_push_products() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));

        let spider_service = MockSpiderService::new();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates.expect_get_candidates().returning(|_| {
            Box::pin(async {
                Ok(vec![scraper_candidate(
                    "Test Shop",
                    ShopType::CommercialDealer,
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )])
            })
        });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .returning(|_, _, _| Box::pin(async { Ok(None) }));

        let mut push_service = MockProductPushService::new();
        push_service.expect_push().times(0);

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_shop_registration(),
            Box::new(push_service),
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_mark_fetch_failure_for_retryable_scraper_http_error() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let spider_service = MockSpiderService::new();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates.expect_get_candidates().returning(|_| {
            Box::pin(async {
                Ok(vec![scraper_candidate(
                    "Test Shop",
                    ShopType::CommercialDealer,
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )])
            })
        });
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().returning(|_, url, _| {
            let url = url.clone();
            Box::pin(async move {
                Err(ScraperError::HttpError {
                    url,
                    kind: crate::network::policy::NetworkErrorKind::Timeout,
                    details: "timeout".to_string(),
                })
            })
        });

        let mut push_service = MockProductPushService::new();
        push_service.expect_push().times(0);

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_shop_registration(),
            Box::new(push_service),
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_continue_same_domain_after_retryable_network_failure() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let spider_service = MockSpiderService::new();

        let first_url = url::Url::parse("https://same-domain.com/product/1").unwrap();
        let second_url = url::Url::parse("https://same-domain.com/product/2").unwrap();
        let first_candidate_url = first_url.clone();
        let second_candidate_url = second_url.clone();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(move |_| {
                let first_candidate_url = first_candidate_url.clone();
                let second_candidate_url = second_candidate_url.clone();
                Box::pin(async move {
                    Ok(vec![
                        scraper_candidate("Shop", ShopType::CommercialDealer, first_candidate_url),
                        scraper_candidate("Shop", ShopType::CommercialDealer, second_candidate_url),
                    ])
                })
            });
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
            .returning(move |_, url, _| {
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

        let mut push_service = MockProductPushService::new();
        push_service.expect_push().times(0);

        let job = CrawlerCronJob::new(
            CrawlerCronConfig {
                scraper_domain_delay: Duration::from_millis(1),
                ..CrawlerCronConfig::default()
            },
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_shop_registration(),
            Box::new(push_service),
        );

        job.run_scraper_once().await;
        assert_eq!(scrape_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn should_mark_fetch_failure_for_llm_budget_exceeded_error() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let spider_service = MockSpiderService::new();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates.expect_get_candidates().returning(|_| {
            Box::pin(async {
                Ok(vec![scraper_candidate(
                    "Test Shop",
                    ShopType::CommercialDealer,
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )])
            })
        });
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .returning(|shop_id, url, _| {
                let url = url.clone();
                let shop_id = *shop_id;
                Box::pin(async move {
                    Err(ScraperError::LlmBudgetExceeded {
                        shop_id,
                        url,
                        max_calls: 5,
                    })
                })
            });

        let mut push_service = MockProductPushService::new();
        push_service.expect_push().times(0);

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_shop_registration(),
            Box::new(push_service),
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_skip_remaining_shop_candidates_when_schema_review_is_pending() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let spider_service = MockSpiderService::new();

        let shop_id = ShopId::new();
        let first_url = url::Url::parse("https://example.com/product/1").unwrap();
        let second_url = url::Url::parse("https://example.com/product/2").unwrap();

        let first_url_for_candidates = first_url.clone();
        let second_url_for_candidates = second_url.clone();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(move |_| {
                let mut first = scraper_candidate(
                    "Test Shop",
                    ShopType::CommercialDealer,
                    first_url_for_candidates.clone(),
                );
                first.shop_id = shop_id;
                let mut second = scraper_candidate(
                    "Test Shop",
                    ShopType::CommercialDealer,
                    second_url_for_candidates.clone(),
                );
                second.shop_id = shop_id;
                Box::pin(async move { Ok(vec![first, second]) })
            });
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(move |received_shop_id, received_url, kind, _, _, _| {
                *received_shop_id == shop_id
                    && received_url == &first_url
                    && kind == "PendingSchemaReview"
            })
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .once()
            .returning(|_, url, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::PendingSchemaReview {
                        url,
                        review_id: uuid::Uuid::new_v4(),
                    })
                })
            });

        let mut push_service = MockProductPushService::new();
        push_service.expect_push().times(0);

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_shop_registration(),
            Box::new(push_service),
        );

        job.run_scraper_once().await;
    }

    /// `NormalizationFixExhausted` must be handled identically to
    /// `SchemaRegenerationExhausted`: write a cooldown via `mark_fetch_failure`
    /// so the URL is held back until the backoff window expires.
    #[tokio::test]
    async fn should_mark_fetch_failure_for_normalization_fix_exhausted_error() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let spider_service = MockSpiderService::new();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates.expect_get_candidates().returning(|_| {
            Box::pin(async {
                Ok(vec![scraper_candidate(
                    "Test Shop",
                    ShopType::CommercialDealer,
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )])
            })
        });
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .returning(|_, url, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::NormalizationFixExhausted {
                        url,
                        attempts: 3,
                        last_norm_error: crate::scraper::normalization::product_normalization_service::NormalizationError::TitleEmpty,
                    })
                })
            });

        let mut push_service = MockProductPushService::new();
        push_service.expect_push().times(0);

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_shop_registration(),
            Box::new(push_service),
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_scrape_candidates_from_multiple_domains() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let spider_service = MockSpiderService::new();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates.expect_get_candidates().returning(|_| {
            Box::pin(async {
                Ok(vec![
                    scraper_candidate(
                        "Shop A",
                        ShopType::CommercialDealer,
                        url::Url::parse("https://domain-a.com/product/1").unwrap(),
                    ),
                    scraper_candidate(
                        "Shop B",
                        ShopType::CommercialDealer,
                        url::Url::parse("https://domain-b.com/product/2").unwrap(),
                    ),
                ])
            })
        });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(2)
            .returning(|_, _, _| Box::pin(async { Ok(None) }));

        let mut push_service = MockProductPushService::new();
        push_service.expect_push().times(0);

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_shop_registration(),
            Box::new(push_service),
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_skip_same_shop_candidate_already_scraping_on_another_domain() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let spider_service = MockSpiderService::new();

        let shop_id = ShopId::new();
        let first_url = url::Url::parse("https://domain-a.com/product/1").unwrap();
        let second_url = url::Url::parse("https://domain-b.com/product/2").unwrap();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(move |_| {
                let mut first =
                    scraper_candidate("Same Shop", ShopType::CommercialDealer, first_url.clone());
                first.shop_id = shop_id;
                let mut second =
                    scraper_candidate("Same Shop", ShopType::CommercialDealer, second_url.clone());
                second.shop_id = shop_id;
                Box::pin(async move { Ok(vec![first, second]) })
            });

        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().once().returning(|_, _, _| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                Ok(None)
            })
        });

        let mut push_service = MockProductPushService::new();
        push_service.expect_push().times(0);

        let job = CrawlerCronJob::new(
            CrawlerCronConfig {
                scraper_concurrency: 2,
                scraper_domain_delay: Duration::ZERO,
                ..CrawlerCronConfig::default()
            },
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_shop_registration(),
            Box::new(push_service),
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_skip_scraper_candidate_when_url_lock_is_already_held() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let spider_service = MockSpiderService::new();

        let locked_url = url::Url::parse("https://domain-a.com/product/1").unwrap();
        let open_url = url::Url::parse("https://domain-a.com/product/2").unwrap();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(move |_| {
                let locked_url = locked_url.clone();
                let open_url = open_url.clone();
                Box::pin(async move {
                    Ok(vec![
                        scraper_candidate("Shop A", ShopType::CommercialDealer, locked_url),
                        scraper_candidate("Shop A", ShopType::CommercialDealer, open_url),
                    ])
                })
            });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(1)
            .returning(|_, _, _| Box::pin(async { Ok(None) }));

        let mut push_service = MockProductPushService::new();
        push_service.expect_push().times(0);

        let lock_manager = Arc::new(LocalLockManager::new());
        let prelocked = url::Url::parse("https://domain-a.com/product/1").unwrap();
        let _prelock = UrlLock::try_acquire(&lock_manager, &prelocked).unwrap();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::clone(&lock_manager),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_shop_registration(),
            Box::new(push_service),
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_scrape_all_urls_from_same_domain() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let spider_service = MockSpiderService::new();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates.expect_get_candidates().returning(|_| {
            Box::pin(async {
                Ok(vec![
                    scraper_candidate(
                        "Shop",
                        ShopType::CommercialDealer,
                        url::Url::parse("https://same-domain.com/product/1").unwrap(),
                    ),
                    scraper_candidate(
                        "Shop",
                        ShopType::CommercialDealer,
                        url::Url::parse("https://same-domain.com/product/2").unwrap(),
                    ),
                    scraper_candidate(
                        "Shop",
                        ShopType::CommercialDealer,
                        url::Url::parse("https://same-domain.com/product/3").unwrap(),
                    ),
                ])
            })
        });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(3)
            .returning(|_, _, _| Box::pin(async { Ok(None) }));

        let mut push_service = MockProductPushService::new();
        push_service.expect_push().times(0);

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_shop_registration(),
            Box::new(push_service),
        );

        job.run_scraper_once().await;
    }
}
