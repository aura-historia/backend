use super::job::CrawlerCronJob;
use crate::CrawlerDomainId;
use crate::network::policy::{NetworkErrorKind, durable_retry_cooldown_for, exponential_cooldown};
use crate::spider::candidate_service::{SPIDER_RECRAWL_INTERVAL, SpiderCandidate};
use crate::spider::classification::url_pattern_service::UrlPatternServiceError;
use crate::spider::discovery::website_spider::{
    CrawlCooldownProfile, CrawlFailureKind, SpiderDiscoveryError,
};
use crate::spider::local_lock::{DomainLock, DomainLockScope};
use crate::spider::service::{SpiderService, SpiderServiceError};
#[cfg(test)]
use listing_source_core::ListingSourceId;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;
use tracing::{Instrument, debug, error, info, warn};

const CRAWL_RETRY_COOLDOWN: Duration = Duration::from_secs(6 * 60 * 60);
const TRANSIENT_CRAWL_LONG_COOLDOWN: Duration = Duration::from_secs(24 * 60 * 60);
const RECOVERABLE_CRAWL_LONG_COOLDOWN: Duration = Duration::from_secs(3 * 24 * 60 * 60);
const DURABLE_BLOCK_CRAWL_LONG_COOLDOWN: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const LONG_COOLDOWN_FAILURE_COUNT: i32 = 3;
const TRUNCATED_CRAWL_RETRY_COOLDOWN: Duration = Duration::from_secs(24 * 60 * 60);

fn long_cooldown(profile: CrawlCooldownProfile) -> Duration {
    match profile {
        CrawlCooldownProfile::Transient => TRANSIENT_CRAWL_LONG_COOLDOWN,
        CrawlCooldownProfile::Recoverable => RECOVERABLE_CRAWL_LONG_COOLDOWN,
        CrawlCooldownProfile::DurableBlock => DURABLE_BLOCK_CRAWL_LONG_COOLDOWN,
    }
}

fn cooldown_for_failure_count(profile: CrawlCooldownProfile, failure_count: i32) -> Duration {
    match failure_count >= LONG_COOLDOWN_FAILURE_COUNT {
        true => long_cooldown(profile),
        false => CRAWL_RETRY_COOLDOWN,
    }
}

/// A crawl cut short by the wall-clock budget is retried sooner than the
/// normal recrawl interval. The delay doubles with each consecutive
/// truncation and is capped at that interval, so a domain too large for one
/// crawl settles into the normal schedule instead of looping.
fn truncated_crawl_cooldown(truncation_count: i32) -> Duration {
    let exponent = u32::try_from(truncation_count.saturating_sub(1))
        .unwrap_or(0)
        .min(31);
    TRUNCATED_CRAWL_RETRY_COOLDOWN
        .saturating_mul(1_u32 << exponent)
        .min(SPIDER_RECRAWL_INTERVAL)
}

fn next_truncation_count(candidate: &SpiderCandidate) -> i32 {
    candidate.crawl_truncation_count.max(0).saturating_add(1)
}

fn next_failure_count(candidate: &SpiderCandidate, error_kind: &str) -> i32 {
    if candidate.last_crawl_error_kind.as_deref() == Some(error_kind) {
        candidate.crawl_failure_count.max(0).saturating_add(1)
    } else {
        1
    }
}

/// Typed crawl failure behind a spider run error, whether Spider or the root
/// preflight detected it.
fn crawl_failure_kind(error: &SpiderServiceError) -> Option<CrawlFailureKind> {
    match error {
        SpiderServiceError::CrawlFailed { kind, .. }
        | SpiderServiceError::Discovery(SpiderDiscoveryError::Preflight { kind, .. }) => {
            Some(*kind)
        }
        _ => None,
    }
}

fn crawl_cooldown_profile(error: &SpiderServiceError) -> Option<CrawlCooldownProfile> {
    match error {
        SpiderServiceError::InsufficientInferenceSample { .. } => {
            Some(CrawlCooldownProfile::Recoverable)
        }
        _ => crawl_failure_kind(error).map(CrawlFailureKind::cooldown_profile),
    }
}

/// Cooldown for the catch-all `spider_run_error` case (internal or config
/// faults without a crawl profile). Starts at the generic durable retry
/// cooldown and escalates like the scraper domain circuit, so a persistent
/// fault cannot retry every few minutes forever.
fn unclassified_cooldown_for_failure_count(failure_count: i32) -> Duration {
    let failure_streak = u32::try_from(failure_count).unwrap_or(1);
    exponential_cooldown(
        durable_retry_cooldown_for(NetworkErrorKind::Unknown),
        failure_streak,
    )
}

fn cooldown_for_spider_failure(error: &SpiderServiceError, failure_count: i32) -> Duration {
    crawl_cooldown_profile(error)
        .map(|profile| cooldown_for_failure_count(profile, failure_count))
        .unwrap_or_else(|| unclassified_cooldown_for_failure_count(failure_count))
}

/// Persisted `last_crawl_error_kind` identifier; these strings are a stable
/// contract with existing rows.
fn error_kind_for_spider_error(error: &SpiderServiceError) -> &'static str {
    match error {
        SpiderServiceError::UrlPattern(UrlPatternServiceError::PendingReview { .. }) => {
            "PendingUrlPatternReview"
        }
        SpiderServiceError::InsufficientInferenceSample { .. } => "InsufficientInferenceSample",
        _ => crawl_failure_kind(error).map_or("spider_run_error", CrawlFailureKind::as_str),
    }
}

struct SpiderSlotOutcome {
    domain_id: CrawlerDomainId,
    succeeded: bool,
    truncated: bool,
    skipped: bool,
    deferred: bool,
}

fn spawn_spider_candidate(
    join_set: &mut JoinSet<SpiderSlotOutcome>,
    candidate: SpiderCandidate,
    spider_candidates: Arc<dyn crate::spider::candidate_service::SpiderCandidateService>,
    spider_service: Arc<dyn SpiderService>,
    lock_manager: Arc<crate::spider::local_lock::LocalLockManager>,
    threshold: usize,
) {
    let crawl_root_url = if candidate.listing_source_domain.starts_with("http") {
        candidate.listing_source_domain.clone()
    } else {
        format!("https://{}", candidate.listing_source_domain)
    };
    let domain_id = candidate.domain_id;
    let span = tracing::info_span!(
        "spider_candidate",
        listing_source_id = %candidate.listing_source_id,
        domain_id = %candidate.domain_id,
        crawl_root_url = %crawl_root_url
    );

    join_set.spawn(
        async move {
            let Some(_lock) = DomainLock::try_acquire(
                &lock_manager,
                DomainLockScope::Spider,
                candidate.domain_id,
            ) else {
                debug!(
                    listing_source_id = %candidate.listing_source_id,
                    domain_id = %candidate.domain_id,
                    "Deferring domain - lock held by another spider worker"
                );
                return SpiderSlotOutcome {
                    domain_id,
                    succeeded: false,
                    truncated: false,
                    skipped: false,
                    deferred: true,
                };
            };

            match spider_service
                .run(
                    &candidate.listing_source_id,
                    &candidate.domain_id,
                    &crawl_root_url,
                    threshold,
                )
                .await
            {
                Ok(result) if result.completion.is_full() => {
                    if let Err(err) = spider_candidates
                        .mark_crawl_completed(&candidate.domain_id, result.completion)
                        .await
                    {
                        warn!(
                            error = ?err,
                            domain = %candidate.listing_source_domain,
                            "Failed to persist crawl completion metadata"
                        );
                    }
                    SpiderSlotOutcome {
                        domain_id,
                        succeeded: true,
                        truncated: false,
                        skipped: false,
                        deferred: false,
                    }
                }
                Ok(result) => {
                    // Failure metadata stays untouched: a truncated crawl
                    // neither proves recovery nor counts as a failure.
                    let truncation_count = next_truncation_count(&candidate);
                    let cooldown = truncated_crawl_cooldown(truncation_count);
                    let next_crawl_at = time::OffsetDateTime::now_utc()
                        + time::Duration::seconds(cooldown.as_secs() as i64);

                    if let Err(err) = spider_candidates
                        .mark_crawl_truncated(&candidate.domain_id, truncation_count, next_crawl_at)
                        .await
                    {
                        warn!(
                            error = ?err,
                            domain = %candidate.listing_source_domain,
                            "Failed to persist crawl truncation metadata"
                        );
                    }
                    warn!(
                        domain = %candidate.listing_source_domain,
                        completion = %result.completion,
                        total_crawled = result.total_links,
                        product_urls_count = result.product_urls_count,
                        truncation_count,
                        crawl_failure_count = candidate.crawl_failure_count,
                        next_crawl_at = %next_crawl_at,
                        cooldown_seconds = cooldown.as_secs(),
                        "Spider run truncated by hard timeout"
                    );
                    SpiderSlotOutcome {
                        domain_id,
                        succeeded: false,
                        truncated: true,
                        skipped: false,
                        deferred: false,
                    }
                }
                Err(e) => {
                    let error_kind = error_kind_for_spider_error(&e);
                    let failure_count = next_failure_count(&candidate, error_kind);
                    let cooldown = cooldown_for_spider_failure(&e, failure_count);
                    let next_crawl_at = time::OffsetDateTime::now_utc()
                        + time::Duration::seconds(cooldown.as_secs() as i64);

                    if let Err(err) = spider_candidates
                        .mark_crawl_failure(
                            &candidate.domain_id,
                            error_kind,
                            failure_count,
                            next_crawl_at,
                        )
                        .await
                    {
                        warn!(
                            error = ?err,
                            domain = %candidate.listing_source_domain,
                            "Failed to persist crawl failure metadata"
                        );
                    }
                    match &e {
                        crate::spider::service::SpiderServiceError::InsufficientInferenceSample {
                            stage,
                            sample_size,
                            min_sample_size,
                            ..
                        } => warn!(
                            domain = %candidate.listing_source_domain,
                            error = %e,
                            error_kind,
                            failure_count,
                            stage,
                            sample_size = *sample_size,
                            min_sample_size = *min_sample_size,
                            next_crawl_at = %next_crawl_at,
                            cooldown_seconds = cooldown.as_secs(),
                            "Spider run failed"
                        ),
                        crate::spider::service::SpiderServiceError::CrawlFailed {
                            total_links,
                            diagnostics,
                            ..
                        } => warn!(
                            domain = %candidate.listing_source_domain,
                            error = %e,
                            error_kind,
                            failure_count,
                            total_crawled = *total_links,
                            min_required_links = 2,
                            http_status = ?diagnostics.http_status,
                            final_url = ?diagnostics.final_url,
                            redirect_url = ?diagnostics.redirect_url,
                            diagnostic_reason = ?diagnostics.diagnostic_reason,
                            initial_html_length = ?diagnostics.initial_html_length,
                            initial_should_retry = diagnostics.initial_should_retry,
                            initial_anti_bot = ?diagnostics.initial_anti_bot,
                            pages_received = diagnostics.pages_received,
                            accepted = diagnostics.accepted,
                            rejected_host = diagnostics.rejected_host,
                            rejected_blacklist = diagnostics.rejected_blacklist,
                            rejected_duplicate = diagnostics.rejected_duplicate,
                            robots_blocked = diagnostics.robots_blocked,
                            first_robots_blocked_url = ?diagnostics.first_robots_blocked_url,
                            robots_denied = diagnostics.robots_denied,
                            lagged = diagnostics.lagged,
                            next_crawl_at = %next_crawl_at,
                            cooldown_seconds = cooldown.as_secs(),
                            "Spider run failed"
                        ),
                        crate::spider::service::SpiderServiceError::Discovery(
                            SpiderDiscoveryError::Preflight { reason, source, .. },
                        ) => warn!(
                            domain = %candidate.listing_source_domain,
                            error = %e,
                            error_kind,
                            failure_count,
                            reason,
                            source = ?source,
                            next_crawl_at = %next_crawl_at,
                            cooldown_seconds = cooldown.as_secs(),
                            "Spider run failed"
                        ),
                        _ => warn!(
                            domain = %candidate.listing_source_domain,
                            error = %e,
                            error_kind,
                            failure_count,
                            next_crawl_at = %next_crawl_at,
                            cooldown_seconds = cooldown.as_secs(),
                            "Spider run failed"
                        ),
                    }
                    SpiderSlotOutcome {
                        domain_id,
                        succeeded: false,
                        truncated: false,
                        skipped: false,
                        deferred: false,
                    }
                }
            }
        }
        .instrument(span),
    );
}

impl CrawlerCronJob {
    #[tracing::instrument(name = "crawler_run_spider_once", skip(self))]
    pub(super) async fn run_spider_once(&self) {
        let spider_concurrency = self.config.spider_concurrency;
        if spider_concurrency == 0 {
            warn!(
                spider_concurrency,
                "spider_concurrency is 0, skipping spider scheduler pass"
            );
            return;
        }
        if !self.admit_authoritative_scope_for_work("spider").await {
            return;
        }

        let pass_start = tokio::time::Instant::now();
        let mut excluded_domain_ids: HashSet<CrawlerDomainId> = HashSet::new();
        let mut join_set: JoinSet<SpiderSlotOutcome> = JoinSet::new();
        let mut total = 0usize;
        let mut succeeded = 0usize;
        let mut truncated = 0usize;
        let mut failed = 0usize;
        let mut skipped = 0usize;
        let mut started = false;
        let mut fetch_failed = false;
        let mut no_more_candidates = false;
        let mut deferred = 0usize;

        loop {
            while join_set.len() < spider_concurrency {
                if no_more_candidates || fetch_failed {
                    break;
                }

                let open_slots = spider_concurrency - join_set.len();
                let limit = (open_slots as i64).max(1);
                let excluded: Vec<CrawlerDomainId> = excluded_domain_ids.iter().copied().collect();
                let candidates = match self
                    .spider_candidates
                    .get_candidates(limit, &excluded)
                    .await
                {
                    Ok(candidates) => candidates,
                    Err(e) => {
                        warn!(error = %e, "Failed to retrieve spider candidates");
                        fetch_failed = true;
                        break;
                    }
                };

                if candidates.is_empty() {
                    no_more_candidates = true;
                    if !started && join_set.is_empty() {
                        debug!("No spider candidates, skipping scheduler pass");
                        return;
                    }
                    break;
                }

                if !started {
                    info!(
                        concurrency = spider_concurrency,
                        "Spider scheduler pass starting"
                    );
                    started = true;
                }

                let mut scheduled_any = false;
                for candidate in candidates {
                    if join_set.len() >= spider_concurrency {
                        break;
                    }
                    if !excluded_domain_ids.insert(candidate.domain_id) {
                        continue;
                    }
                    scheduled_any = true;
                    total += 1;
                    spawn_spider_candidate(
                        &mut join_set,
                        candidate,
                        Arc::clone(&self.spider_candidates),
                        Arc::clone(&self.spider_service),
                        Arc::clone(&self.lock_manager),
                        self.config.spider_classify_threshold,
                    );
                }

                if !scheduled_any {
                    break;
                }
            }

            if join_set.is_empty() {
                break;
            }

            match join_set.join_next().await {
                Some(Ok(outcome)) => {
                    excluded_domain_ids.insert(outcome.domain_id);
                    if outcome.deferred {
                        deferred += 1;
                    } else if outcome.succeeded {
                        succeeded += 1;
                    } else if outcome.truncated {
                        truncated += 1;
                    } else if outcome.skipped {
                        skipped += 1;
                    } else {
                        failed += 1;
                    }
                }
                Some(Err(e)) => {
                    error!(error = %e, "Spider worker task failed to join");
                    failed += 1;
                }
                None => break,
            }
        }

        let duration_ms = pass_start.elapsed().as_millis() as u64;
        info!(
            total,
            succeeded,
            truncated,
            failed,
            skipped,
            deferred,
            duration_ms,
            "Spider scheduler pass complete"
        );

        self.spider_perf.record(total as u64, duration_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::policy::MAX_EXPONENTIAL_COOLDOWN;
    use crate::scraper::candidate_service::MockScraperCandidateService;
    use crate::scraper::scraper_service::MockScraperService;
    use crate::service::cron::config::CrawlerCronConfig;
    use crate::service::cron::test_support::{noop_listing_source_registration, noop_raw_capture};
    use crate::service::listing_source_registration::{
        ListingSourceRegistrationService, ListingSourceSyncError,
        MockListingSourceRegistrationRepository, MockListingSourceRegistrationSource,
    };
    use crate::spider::candidate_service::{MockSpiderCandidateService, SpiderCandidate};
    use crate::spider::discovery::website_spider::{CrawlCompletion, CrawlDiagnostics};
    use crate::spider::local_lock::LocalLockManager;
    use crate::spider::service::{MockSpiderService, SpiderRunResult};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::Notify;

    fn spider_candidate(domain_id: CrawlerDomainId) -> SpiderCandidate {
        SpiderCandidate {
            listing_source_id: ListingSourceId::new(),
            domain_id,
            listing_source_domain: "example.com".to_string(),
            crawl_failure_count: 0,
            last_crawl_error_kind: None,
            crawl_truncation_count: 0,
        }
    }

    fn next_crawl_at_is_about(next_crawl_at: time::OffsetDateTime, cooldown: Duration) -> bool {
        let seconds = (next_crawl_at - time::OffsetDateTime::now_utc()).whole_seconds();
        let expected = cooldown.as_secs() as i64;
        seconds >= expected - 60 && seconds <= expected + 60
    }

    #[test]
    fn next_failure_count_increments_for_same_error_kind() {
        let mut candidate = spider_candidate(CrawlerDomainId::new());
        candidate.crawl_failure_count = 2;
        candidate.last_crawl_error_kind = Some("InsufficientInferenceSample".to_string());

        assert_eq!(
            next_failure_count(&candidate, "InsufficientInferenceSample"),
            3
        );
    }

    #[test]
    fn next_failure_count_resets_for_changed_error_kind() {
        let mut candidate = spider_candidate(CrawlerDomainId::new());
        candidate.crawl_failure_count = 2;
        candidate.last_crawl_error_kind = Some("TinyCrawl".to_string());

        assert_eq!(
            next_failure_count(&candidate, "InsufficientInferenceSample"),
            1
        );
    }

    fn crawl_failed(kind: CrawlFailureKind) -> SpiderServiceError {
        SpiderServiceError::CrawlFailed {
            crawl_root_url: "https://example.com/".to_string(),
            kind,
            total_links: 1,
            diagnostics: Box::new(CrawlDiagnostics {
                failure_kind: Some(kind),
                diagnostic_reason: Some("test_diagnostic".to_string()),
                ..CrawlDiagnostics::default()
            }),
        }
    }

    fn preflight_failed(kind: CrawlFailureKind) -> SpiderServiceError {
        SpiderServiceError::Discovery(SpiderDiscoveryError::Preflight {
            kind,
            reason: "test_preflight",
            source: None,
        })
    }

    fn insufficient_sample() -> SpiderServiceError {
        SpiderServiceError::InsufficientInferenceSample {
            crawl_root_url: "https://example.com/".to_string(),
            stage: "end_of_crawl",
            sample_size: 2,
            min_sample_size: 20,
        }
    }

    const ALL_KINDS: [CrawlFailureKind; 14] = [
        CrawlFailureKind::EmptyCrawl,
        CrawlFailureKind::TinyCrawl,
        CrawlFailureKind::NoAcceptedPages,
        CrawlFailureKind::RobotsBlocked,
        CrawlFailureKind::RateLimited,
        CrawlFailureKind::CloudflareChallenge,
        CrawlFailureKind::BotProtection,
        CrawlFailureKind::ConnectError,
        CrawlFailureKind::ServerError,
        CrawlFailureKind::TlsError,
        CrawlFailureKind::RedirectProblem,
        CrawlFailureKind::InvalidUrl,
        CrawlFailureKind::AccessDenied,
        CrawlFailureKind::JavascriptRequired,
    ];

    #[test]
    fn cooldown_should_use_durable_retry_cooldown_for_first_two_grouped_failures() {
        for error in ALL_KINDS
            .into_iter()
            .flat_map(|kind| [crawl_failed(kind), preflight_failed(kind)])
            .chain([insufficient_sample()])
        {
            assert_eq!(cooldown_for_spider_failure(&error, 1), CRAWL_RETRY_COOLDOWN);
            assert_eq!(cooldown_for_spider_failure(&error, 2), CRAWL_RETRY_COOLDOWN);
        }
    }

    #[test]
    fn cooldown_should_use_transient_long_cooldown_for_flaky_failures() {
        for kind in [
            CrawlFailureKind::EmptyCrawl,
            CrawlFailureKind::TinyCrawl,
            CrawlFailureKind::NoAcceptedPages,
            CrawlFailureKind::RateLimited,
            CrawlFailureKind::CloudflareChallenge,
            CrawlFailureKind::BotProtection,
            CrawlFailureKind::ConnectError,
            CrawlFailureKind::ServerError,
        ] {
            assert_eq!(
                cooldown_for_spider_failure(&crawl_failed(kind), LONG_COOLDOWN_FAILURE_COUNT),
                TRANSIENT_CRAWL_LONG_COOLDOWN
            );
        }
    }

    #[test]
    fn cooldown_should_use_recoverable_long_cooldown_for_site_or_sample_failures() {
        for error in [
            insufficient_sample(),
            crawl_failed(CrawlFailureKind::RobotsBlocked),
            crawl_failed(CrawlFailureKind::TlsError),
            crawl_failed(CrawlFailureKind::RedirectProblem),
            crawl_failed(CrawlFailureKind::InvalidUrl),
            preflight_failed(CrawlFailureKind::TlsError),
            preflight_failed(CrawlFailureKind::RedirectProblem),
            preflight_failed(CrawlFailureKind::InvalidUrl),
        ] {
            assert_eq!(
                cooldown_for_spider_failure(&error, LONG_COOLDOWN_FAILURE_COUNT),
                RECOVERABLE_CRAWL_LONG_COOLDOWN
            );
        }
    }

    #[test]
    fn cooldown_should_use_durable_block_long_cooldown_for_explicit_blocks() {
        for kind in [
            CrawlFailureKind::AccessDenied,
            CrawlFailureKind::JavascriptRequired,
        ] {
            assert_eq!(
                cooldown_for_spider_failure(&crawl_failed(kind), LONG_COOLDOWN_FAILURE_COUNT),
                DURABLE_BLOCK_CRAWL_LONG_COOLDOWN
            );
        }
    }

    fn unclassified_errors() -> [SpiderServiceError; 3] {
        [
            SpiderServiceError::Database(sqlx::Error::RowNotFound),
            SpiderServiceError::Discovery(SpiderDiscoveryError::Config(
                "SPIDER_MAX_SIZE_BYTES must be set".to_string(),
            )),
            SpiderServiceError::Discovery(SpiderDiscoveryError::Client(
                application::error::static_error("tls backend unavailable"),
            )),
        ]
    }

    #[test]
    fn cooldown_should_keep_generic_fallback_for_first_unknown_failure() {
        for error in unclassified_errors() {
            assert_eq!(error_kind_for_spider_error(&error), "spider_run_error");
            assert_eq!(
                cooldown_for_spider_failure(&error, 1),
                durable_retry_cooldown_for(NetworkErrorKind::Unknown)
            );
        }
    }

    #[test]
    fn cooldown_should_escalate_unknown_failures_exponentially_up_to_one_day() {
        let base = durable_retry_cooldown_for(NetworkErrorKind::Unknown);
        for error in unclassified_errors() {
            assert_eq!(cooldown_for_spider_failure(&error, 0), base);
            assert_eq!(cooldown_for_spider_failure(&error, 2), base * 2);
            assert_eq!(
                cooldown_for_spider_failure(&error, LONG_COOLDOWN_FAILURE_COUNT),
                base * 4
            );
            assert_eq!(cooldown_for_spider_failure(&error, 6), base * 32);
            assert_eq!(cooldown_for_spider_failure(&error, 9), base * 256);
            assert_eq!(
                cooldown_for_spider_failure(&error, 10),
                MAX_EXPONENTIAL_COOLDOWN
            );
            assert_eq!(
                cooldown_for_spider_failure(&error, 40),
                MAX_EXPONENTIAL_COOLDOWN
            );
            assert_eq!(
                cooldown_for_spider_failure(&error, i32::MAX),
                MAX_EXPONENTIAL_COOLDOWN
            );
        }
    }

    #[test]
    fn error_kind_should_use_stable_persisted_identifiers() {
        for (kind, expected) in ALL_KINDS.into_iter().map(|kind| (kind, kind.as_str())) {
            assert_eq!(error_kind_for_spider_error(&crawl_failed(kind)), expected);
            assert_eq!(
                error_kind_for_spider_error(&preflight_failed(kind)),
                expected
            );
        }
        assert_eq!(
            error_kind_for_spider_error(&insufficient_sample()),
            "InsufficientInferenceSample"
        );
        assert_eq!(
            [
                "EmptyCrawl",
                "TinyCrawl",
                "NoAcceptedPages",
                "RobotsBlocked",
                "RateLimited",
                "CloudflareChallenge",
                "BotProtection",
                "ConnectError",
                "ServerError",
                "TlsError",
                "RedirectProblem",
                "InvalidUrl",
                "AccessDenied",
                "JavascriptRequired",
            ],
            ALL_KINDS.map(CrawlFailureKind::as_str)
        );
    }

    #[tokio::test]
    async fn should_skip_spider_candidates_when_authoritative_scope_refresh_fails() {
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

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(MockSpiderCandidateService::new()),
            Box::new(MockSpiderService::new()),
            Box::new(MockScraperCandidateService::new()),
            Box::new(MockScraperService::new()),
            registration,
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_run_spider_candidates() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(expected_domain_id)]) })
            });
        spider_candidates
            .expect_mark_crawl_completed()
            .withf(move |domain_id, completion| {
                *domain_id == expected_domain_id && *completion == CrawlCompletion::Completed
            })
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .withf(move |_, domain_id, _, _| *domain_id == expected_domain_id)
            .returning(|_, _, _, _| {
                Box::pin(async {
                    Ok(SpiderRunResult {
                        total_links: 10,
                        product_urls_count: 5,
                        product_pattern: None,
                        completion: CrawlCompletion::Completed,
                    })
                })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_refill_spider_slot_while_slow_crawl_is_running() {
        let slow_domain_id = CrawlerDomainId::new();
        let fast_domain_id = CrawlerDomainId::new();
        let refill_domain_id = CrawlerDomainId::new();

        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let excluded = excluded.to_vec();
                Box::pin(async move {
                    if excluded.is_empty() {
                        Ok(vec![
                            spider_candidate(slow_domain_id),
                            spider_candidate(fast_domain_id),
                        ])
                    } else if excluded.contains(&slow_domain_id)
                        && excluded.contains(&fast_domain_id)
                        && !excluded.contains(&refill_domain_id)
                    {
                        Ok(vec![spider_candidate(refill_domain_id)])
                    } else {
                        Ok(vec![])
                    }
                })
            });
        spider_candidates
            .expect_mark_crawl_completed()
            .times(3)
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let slow_running = Arc::new(AtomicBool::new(false));
        let refill_started_while_slow_running = Arc::new(AtomicBool::new(false));
        let release_slow = Arc::new(Notify::new());
        let release_slow_for_mock = Arc::clone(&release_slow);
        let slow_running_for_mock = Arc::clone(&slow_running);
        let refill_started_for_mock = Arc::clone(&refill_started_while_slow_running);

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(3)
            .returning(move |_, domain_id, _, _| {
                let domain_id = *domain_id;
                let release_slow = Arc::clone(&release_slow_for_mock);
                let slow_running = Arc::clone(&slow_running_for_mock);
                let refill_started = Arc::clone(&refill_started_for_mock);
                Box::pin(async move {
                    if domain_id == slow_domain_id {
                        slow_running.store(true, Ordering::SeqCst);
                        release_slow.notified().await;
                        slow_running.store(false, Ordering::SeqCst);
                    } else if domain_id == fast_domain_id {
                        while !slow_running.load(Ordering::SeqCst) {
                            tokio::time::sleep(Duration::from_millis(1)).await;
                        }
                    } else if domain_id == refill_domain_id {
                        if slow_running.load(Ordering::SeqCst) {
                            refill_started.store(true, Ordering::SeqCst);
                        }
                        release_slow.notify_one();
                    }
                    Ok(SpiderRunResult {
                        total_links: 10,
                        product_urls_count: 5,
                        product_pattern: None,
                        completion: CrawlCompletion::Completed,
                    })
                })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));
        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig {
                spider_concurrency: 2,
                ..CrawlerCronConfig::default()
            },
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;

        assert!(refill_started_while_slow_running.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn should_mark_crawl_failure_when_spider_run_errors() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(expected_domain_id)]) })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, failure_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && error_kind == "spider_run_error"
                    && *failure_count == 1
                    && next_crawl_at_is_about(
                        *next_crawl_at,
                        durable_retry_cooldown_for(NetworkErrorKind::Unknown),
                    )
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, _, _| {
            Box::pin(async {
                Err(crate::spider::service::SpiderServiceError::Database(
                    sqlx::Error::RowNotFound,
                ))
            })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_mark_tiny_crawl_failure_with_specific_error_kind() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(expected_domain_id)]) })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, failure_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && error_kind == "TinyCrawl"
                    && *failure_count == 1
                    && next_crawl_at_is_about(*next_crawl_at, CRAWL_RETRY_COOLDOWN)
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, _, _| {
            Box::pin(async move { Err(crawl_failed(CrawlFailureKind::TinyCrawl)) })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_mark_empty_crawl_failure_with_specific_error_kind() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(expected_domain_id)]) })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, failure_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && error_kind == "EmptyCrawl"
                    && *failure_count == 1
                    && next_crawl_at_is_about(*next_crawl_at, CRAWL_RETRY_COOLDOWN)
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, _, _| {
            Box::pin(async move { Err(crawl_failed(CrawlFailureKind::EmptyCrawl)) })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_mark_insufficient_inference_sample_failure_with_specific_error_kind() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(expected_domain_id)]) })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, failure_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && error_kind == "InsufficientInferenceSample"
                    && *failure_count == 1
                    && next_crawl_at_is_about(*next_crawl_at, CRAWL_RETRY_COOLDOWN)
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, _, _| {
            Box::pin(async move {
                Err(
                    crate::spider::service::SpiderServiceError::InsufficientInferenceSample {
                        crawl_root_url: "https://example.com/".to_string(),
                        stage: "end_of_crawl",
                        sample_size: 16,
                        min_sample_size: 20,
                    },
                )
            })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_escalate_cooldown_after_repeated_spider_run_errors() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move {
                    let mut candidate = spider_candidate(expected_domain_id);
                    candidate.crawl_failure_count = 2;
                    candidate.last_crawl_error_kind = Some("spider_run_error".to_string());
                    Ok(vec![candidate])
                })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, failure_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && error_kind == "spider_run_error"
                    && *failure_count == 3
                    && next_crawl_at_is_about(
                        *next_crawl_at,
                        durable_retry_cooldown_for(NetworkErrorKind::Unknown) * 4,
                    )
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, _, _| {
            Box::pin(async {
                Err(crate::spider::service::SpiderServiceError::Database(
                    sqlx::Error::RowNotFound,
                ))
            })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_use_long_cooldown_after_repeated_empty_crawl_failures() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move {
                    let mut candidate = spider_candidate(expected_domain_id);
                    candidate.crawl_failure_count = 2;
                    candidate.last_crawl_error_kind = Some("EmptyCrawl".to_string());
                    Ok(vec![candidate])
                })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, failure_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && error_kind == "EmptyCrawl"
                    && *failure_count == 3
                    && next_crawl_at_is_about(*next_crawl_at, TRANSIENT_CRAWL_LONG_COOLDOWN)
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, _, _| {
            Box::pin(async move { Err(crawl_failed(CrawlFailureKind::EmptyCrawl)) })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_use_long_cooldown_after_repeated_insufficient_inference_sample_failures() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move {
                    let mut candidate = spider_candidate(expected_domain_id);
                    candidate.crawl_failure_count = 2;
                    candidate.last_crawl_error_kind =
                        Some("InsufficientInferenceSample".to_string());
                    Ok(vec![candidate])
                })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, failure_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && error_kind == "InsufficientInferenceSample"
                    && *failure_count == 3
                    && next_crawl_at_is_about(*next_crawl_at, RECOVERABLE_CRAWL_LONG_COOLDOWN)
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, _, _| {
            Box::pin(async move {
                Err(
                    crate::spider::service::SpiderServiceError::InsufficientInferenceSample {
                        crawl_root_url: "https://example.com/".to_string(),
                        stage: "end_of_crawl",
                        sample_size: 16,
                        min_sample_size: 20,
                    },
                )
            })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    async fn assert_diagnostic_failure_is_persisted(
        kind: CrawlFailureKind,
        expected_error_kind: &'static str,
    ) {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(expected_domain_id)]) })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, failure_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && error_kind == expected_error_kind
                    && *failure_count == 1
                    && next_crawl_at_is_about(*next_crawl_at, CRAWL_RETRY_COOLDOWN)
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .returning(move |_, _, _, _| Box::pin(async move { Err(crawl_failed(kind)) }));

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_persist_diagnostic_failure_kinds_with_small_crawl_cooldown() {
        for (kind, expected_error_kind) in [
            (CrawlFailureKind::EmptyCrawl, "EmptyCrawl"),
            (CrawlFailureKind::TinyCrawl, "TinyCrawl"),
            (CrawlFailureKind::NoAcceptedPages, "NoAcceptedPages"),
            (CrawlFailureKind::RobotsBlocked, "RobotsBlocked"),
            (CrawlFailureKind::RateLimited, "RateLimited"),
            (CrawlFailureKind::AccessDenied, "AccessDenied"),
            (CrawlFailureKind::CloudflareChallenge, "CloudflareChallenge"),
            (CrawlFailureKind::BotProtection, "BotProtection"),
            (CrawlFailureKind::TlsError, "TlsError"),
            (CrawlFailureKind::ConnectError, "ConnectError"),
            (CrawlFailureKind::ServerError, "ServerError"),
            (CrawlFailureKind::RedirectProblem, "RedirectProblem"),
            (CrawlFailureKind::InvalidUrl, "InvalidUrl"),
            (CrawlFailureKind::JavascriptRequired, "JavascriptRequired"),
        ] {
            assert_diagnostic_failure_is_persisted(kind, expected_error_kind).await;
        }
    }

    #[tokio::test]
    async fn should_persist_typed_preflight_failure_instead_of_generic_run_error() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move {
                    let mut candidate = spider_candidate(expected_domain_id);
                    candidate.crawl_failure_count = 2;
                    candidate.last_crawl_error_kind = Some("ConnectError".to_string());
                    Ok(vec![candidate])
                })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, failure_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && error_kind == "ConnectError"
                    && *failure_count == 3
                    && next_crawl_at_is_about(*next_crawl_at, TRANSIENT_CRAWL_LONG_COOLDOWN)
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, _, _| {
            Box::pin(async { Err(preflight_failed(CrawlFailureKind::ConnectError)) })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(MockScraperService::new()),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_use_long_cooldown_after_repeated_diagnostic_failures() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move {
                    let mut candidate = spider_candidate(expected_domain_id);
                    candidate.crawl_failure_count = 2;
                    candidate.last_crawl_error_kind = Some("RateLimited".to_string());
                    Ok(vec![candidate])
                })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, failure_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && error_kind == "RateLimited"
                    && *failure_count == 3
                    && next_crawl_at_is_about(*next_crawl_at, TRANSIENT_CRAWL_LONG_COOLDOWN)
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, _, _| {
            Box::pin(async move { Err(crawl_failed(CrawlFailureKind::RateLimited)) })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[test]
    fn truncated_cooldown_should_start_sooner_than_recrawl_and_double_up_to_it() {
        let day = Duration::from_secs(24 * 60 * 60);

        assert_eq!(truncated_crawl_cooldown(1), day);
        assert_eq!(truncated_crawl_cooldown(2), day * 2);
        assert_eq!(truncated_crawl_cooldown(3), day * 4);
        assert_eq!(truncated_crawl_cooldown(4), SPIDER_RECRAWL_INTERVAL);
        assert!(truncated_crawl_cooldown(1) < SPIDER_RECRAWL_INTERVAL);
    }

    #[test]
    fn truncated_cooldown_should_never_drop_below_one_day_or_exceed_recrawl_interval() {
        for count in [i32::MIN, -1, 0, 1, 5, 31, 32, 1_000, i32::MAX] {
            let cooldown = truncated_crawl_cooldown(count);
            assert!(
                cooldown >= TRUNCATED_CRAWL_RETRY_COOLDOWN,
                "{count}: {cooldown:?}"
            );
            assert!(cooldown <= SPIDER_RECRAWL_INTERVAL, "{count}: {cooldown:?}");
        }
    }

    fn run_result(completion: CrawlCompletion) -> SpiderRunResult {
        SpiderRunResult {
            total_links: 4_000,
            product_urls_count: 1_200,
            product_pattern: Some("/product/".to_string()),
            completion,
        }
    }

    fn spider_job(
        spider_candidates: MockSpiderCandidateService,
        spider_service: MockSpiderService,
        spider_concurrency: usize,
    ) -> CrawlerCronJob {
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        CrawlerCronJob::new(
            CrawlerCronConfig {
                spider_concurrency,
                ..CrawlerCronConfig::default()
            },
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(MockScraperService::new()),
            noop_listing_source_registration(),
            noop_raw_capture(),
        )
    }

    #[tokio::test]
    async fn should_mark_truncated_without_resetting_prior_failure_metadata() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let first_call = excluded.is_empty();
                Box::pin(async move {
                    if !first_call {
                        return Ok(vec![]);
                    }
                    let mut candidate = spider_candidate(expected_domain_id);
                    candidate.crawl_failure_count = 2;
                    candidate.last_crawl_error_kind = Some("RateLimited".to_string());
                    candidate.crawl_truncation_count = 1;
                    Ok(vec![candidate])
                })
            });
        spider_candidates
            .expect_mark_crawl_truncated()
            .times(1)
            .withf(move |domain_id, truncation_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && *truncation_count == 2
                    && next_crawl_at_is_about(*next_crawl_at, truncated_crawl_cooldown(2))
            })
            .returning(|_, _, _| Box::pin(async { Ok(()) }));
        spider_candidates.expect_mark_crawl_completed().times(0);
        spider_candidates.expect_mark_crawl_failure().times(0);

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(1)
            .returning(|_, _, _, _| {
                Box::pin(async { Ok(run_result(CrawlCompletion::HardTimeoutTruncated)) })
            });

        spider_job(spider_candidates, spider_service, 1)
            .run_spider_once()
            .await;
    }

    #[tokio::test]
    async fn should_treat_page_budget_as_completed_crawl() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let first_call = excluded.is_empty();
                Box::pin(async move {
                    Ok(if first_call {
                        vec![spider_candidate(expected_domain_id)]
                    } else {
                        vec![]
                    })
                })
            });
        spider_candidates
            .expect_mark_crawl_completed()
            .times(1)
            .withf(move |domain_id, completion| {
                *domain_id == expected_domain_id
                    && *completion == CrawlCompletion::PageBudgetReached
            })
            .returning(|_, _| Box::pin(async { Ok(()) }));
        spider_candidates.expect_mark_crawl_truncated().times(0);

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(1)
            .returning(|_, _, _, _| {
                Box::pin(async { Ok(run_result(CrawlCompletion::PageBudgetReached)) })
            });

        spider_job(spider_candidates, spider_service, 1)
            .run_spider_once()
            .await;
    }

    #[tokio::test]
    async fn should_keep_genuine_failure_classification_after_truncation() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let first_call = excluded.is_empty();
                Box::pin(async move {
                    if !first_call {
                        return Ok(vec![]);
                    }
                    let mut candidate = spider_candidate(expected_domain_id);
                    candidate.crawl_truncation_count = 3;
                    Ok(vec![candidate])
                })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .times(1)
            .withf(move |domain_id, error_kind, failure_count, next_crawl_at| {
                *domain_id == expected_domain_id
                    && error_kind == "ConnectError"
                    && *failure_count == 1
                    && next_crawl_at_is_about(*next_crawl_at, CRAWL_RETRY_COOLDOWN)
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(()) }));
        spider_candidates.expect_mark_crawl_truncated().times(0);
        spider_candidates.expect_mark_crawl_completed().times(0);

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(1)
            .returning(|_, _, _, _| {
                Box::pin(async { Err(crawl_failed(CrawlFailureKind::ConnectError)) })
            });

        spider_job(spider_candidates, spider_service, 1)
            .run_spider_once()
            .await;
    }

    #[tokio::test]
    async fn should_complete_other_domains_while_one_crawl_is_truncated() {
        let truncated_domain_id = CrawlerDomainId::new();
        let completed_domain_id = CrawlerDomainId::new();

        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let first_call = excluded.is_empty();
                Box::pin(async move {
                    Ok(if first_call {
                        vec![
                            spider_candidate(truncated_domain_id),
                            spider_candidate(completed_domain_id),
                        ]
                    } else {
                        vec![]
                    })
                })
            });
        spider_candidates
            .expect_mark_crawl_truncated()
            .times(1)
            .withf(move |domain_id, truncation_count, _| {
                *domain_id == truncated_domain_id && *truncation_count == 1
            })
            .returning(|_, _, _| Box::pin(async { Ok(()) }));
        spider_candidates
            .expect_mark_crawl_completed()
            .times(1)
            .withf(move |domain_id, completion| {
                *domain_id == completed_domain_id && *completion == CrawlCompletion::Completed
            })
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(2)
            .returning(move |_, domain_id, _, _| {
                let completion = if *domain_id == truncated_domain_id {
                    CrawlCompletion::HardTimeoutTruncated
                } else {
                    CrawlCompletion::Completed
                };
                Box::pin(async move { Ok(run_result(completion)) })
            });

        spider_job(spider_candidates, spider_service, 2)
            .run_spider_once()
            .await;
    }

    #[tokio::test]
    async fn should_run_spider_candidate_while_the_scraper_holds_its_own_domain_lock() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| Box::pin(async move { Ok(vec![spider_candidate(domain_id)]) }));
        spider_candidates
            .expect_mark_crawl_completed()
            .returning(|_, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().once().returning(|_, _, _, _| {
            Box::pin(async {
                Ok(SpiderRunResult {
                    total_links: 10,
                    product_urls_count: 5,
                    product_pattern: None,
                    completion: CrawlCompletion::Completed,
                })
            })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();
        let lock_manager = Arc::new(LocalLockManager::new());
        let _scraper_lock =
            DomainLock::try_acquire(&lock_manager, DomainLockScope::Scraper, domain_id).unwrap();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::clone(&lock_manager),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_skip_spider_candidate_when_domain_lock_is_already_held() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let locked_domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(locked_domain_id)]) })
            });

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().times(0);

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();
        let lock_manager = Arc::new(LocalLockManager::new());
        let _prelock =
            DomainLock::try_acquire(&lock_manager, DomainLockScope::Spider, locked_domain_id)
                .unwrap();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::clone(&lock_manager),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );

        job.run_spider_once().await;
    }
}
