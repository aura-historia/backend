use super::job::CrawlerCronJob;
use super::metrics::emit_pass_duration;
use crate::metrics::{CrawlerMetric, MetricDimension, MetricRecord};
use crate::network::CRAWLER_USER_AGENT;
use crate::network::policy::{
    MAX_EXPONENTIAL_COOLDOWN, NetworkErrorKind, durable_retry_cooldown_for, exponential_cooldown,
};
use crate::spider::candidate_service::{
    AWAITING_URL_PATTERN_REVIEW_CRAWL_OUTCOME, CrawlRateLimitWrite, FAILED_CRAWL_OUTCOME,
    NEEDS_OPERATOR_CRAWL_OUTCOME, RATE_LIMITED_CRAWL_OUTCOME, SPIDER_RECRAWL_INTERVAL,
    SpiderCandidate, SpiderCandidateService, UrlPatternReviewWaitWrite,
};
use crate::spider::classification::url_pattern_service::UrlPatternServiceError;
use crate::spider::crawled_root::CrawlOutcomeWrite;
use crate::spider::discovery::frontier::CrawlKind;
use crate::spider::discovery::page_status::RateLimitSignal;
use crate::spider::discovery::website_spider::{
    CrawlCompletion, CrawlCooldownProfile, CrawlFailureKind, CrawlPace, SpiderDiscoveryError,
};
use crate::spider::local_lock::{DomainLock, DomainLockScope};
use crate::spider::service::{SpiderRunResult, SpiderService, SpiderServiceError};
use crate::{CrawlerDomainId, CrawlerReviewId};
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
const RATE_LIMITED_CRAWL_RETRY_COOLDOWN: Duration = Duration::from_secs(60 * 60);

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

/// A full crawl cut short by the wall-clock budget continues from the
/// frontier it left a day later, however often it was cut short before: each
/// crawl visits pages the previous ones did not reach, so a large shop is
/// covered in daily steps instead of restarting from the root
/// (`docs/crawler/scheduling-and-retries.md#discovery-coverage`).
fn truncated_crawl_cooldown() -> Duration {
    TRUNCATED_CRAWL_RETRY_COOLDOWN
}

/// A crawl a shop rate-limited is retried after the later of the shop's
/// `Retry-After` and a backoff that starts at one hour and doubles with each
/// consecutive rate-limited crawl, capped at one day. The retry is never
/// sooner than the shop asked, and repeated rate limits always wait longer.
fn rate_limited_crawl_cooldown(rate_limit_streak: i32, retry_after: Option<Duration>) -> Duration {
    let backoff = exponential_cooldown(
        RATE_LIMITED_CRAWL_RETRY_COOLDOWN,
        u32::try_from(rate_limit_streak).unwrap_or(0),
        MAX_EXPONENTIAL_COOLDOWN,
    );
    retry_after.map_or(backoff, |retry_after| retry_after.max(backoff))
}

/// What a run whose crawl a shop's rate-limit answer stopped records,
/// whatever its outcome: the domain's rate-limit streak grows by one and its
/// next crawl waits for the rate-limit cooldown, returned for the log.
fn rate_limited_crawl_write(
    candidate: &SpiderCandidate,
    rate_limit: RateLimitSignal,
) -> (CrawlRateLimitWrite, Duration) {
    let streak = candidate.crawl_rate_limit_streak.max(0).saturating_add(1);
    let cooldown = rate_limited_crawl_cooldown(streak, rate_limit.retry_after);
    let write = CrawlRateLimitWrite {
        streak,
        not_before: next_crawl_after(cooldown),
    };
    (write, cooldown)
}

/// The pace of the domain's next crawl, slowed by its rate-limit streak.
fn crawl_pace(candidate: &SpiderCandidate) -> CrawlPace {
    CrawlPace::after_rate_limited_crawls(
        u32::try_from(candidate.crawl_rate_limit_streak).unwrap_or(0),
    )
}

fn next_truncation_count(candidate: &SpiderCandidate) -> i32 {
    candidate.crawl_truncation_count.max(0).saturating_add(1)
}

/// Persisted error kinds of the failures whose run delivered pages but found no product URL
/// ([`SpiderServiceError::found_no_product_urls`]). They count as one failure streak, so a site
/// without a catalogue whose runs alternate between them is escalated like one that repeats a
/// single kind: each such run moves the domain's crawl frontier on, and the streak bounds how
/// often it does (`docs/crawler/scheduling-and-retries.md#discovery-coverage`).
const NO_PRODUCT_URL_ERROR_KINDS: [&str; 3] = [
    "InsufficientInferenceSample",
    "NoProductUrls",
    "NoUrlPattern",
];

/// Whether a failure of `error_kind` continues the streak of the domain's last failure.
fn continues_failure_streak(last_error_kind: Option<&str>, error_kind: &str) -> bool {
    let found_no_product_urls = |kind: &str| NO_PRODUCT_URL_ERROR_KINDS.contains(&kind);
    last_error_kind.is_some_and(|last| {
        last == error_kind || (found_no_product_urls(last) && found_no_product_urls(error_kind))
    })
}

fn next_failure_count(candidate: &SpiderCandidate, error_kind: &str) -> i32 {
    if continues_failure_streak(candidate.last_crawl_error_kind.as_deref(), error_kind) {
        candidate.crawl_failure_count.max(0).saturating_add(1)
    } else {
        1
    }
}

/// Typed crawl failure behind a spider run error, whether Spider or the root
/// preflight detected it, or the run's pages were mostly bot challenges.
fn crawl_failure_kind(error: &SpiderServiceError) -> Option<CrawlFailureKind> {
    match error.cause() {
        SpiderServiceError::CrawlFailed { kind, .. }
        | SpiderServiceError::Discovery(SpiderDiscoveryError::Preflight { kind, .. }) => {
            Some(*kind)
        }
        SpiderServiceError::BotChallengedCrawl { .. } => Some(CrawlFailureKind::BotProtection),
        _ => None,
    }
}

fn crawl_cooldown_profile(error: &SpiderServiceError) -> Option<CrawlCooldownProfile> {
    if error.found_no_product_urls() {
        return Some(CrawlCooldownProfile::Recoverable);
    }
    crawl_failure_kind(error).map(CrawlFailureKind::cooldown_profile)
}

/// Consecutive failures of one streak ([`continues_failure_streak`]) after
/// which a domain stops retrying and waits for an operator. Only failures
/// that need a site or configuration change (the recoverable profile)
/// escalate: with two short retries and then the long cooldown, the fifth
/// failure comes about a week after the first. Transient failures keep
/// retrying because the site may recover, durable blocks already cool for a
/// month.
const NEEDS_OPERATOR_FAILURE_COUNT: i32 = 5;

fn needs_operator(error: &SpiderServiceError, failure_count: i32) -> bool {
    crawl_cooldown_profile(error) == Some(CrawlCooldownProfile::Recoverable)
        && failure_count >= NEEDS_OPERATOR_FAILURE_COUNT
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
        MAX_EXPONENTIAL_COOLDOWN,
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
    match error.cause() {
        SpiderServiceError::UrlPattern(UrlPatternServiceError::PendingReview { .. }) => {
            "PendingUrlPatternReview"
        }
        SpiderServiceError::InsufficientInferenceSample { .. } => "InsufficientInferenceSample",
        SpiderServiceError::NoProductUrls { .. } => "NoProductUrls",
        SpiderServiceError::NoUrlPattern { .. } => "NoUrlPattern",
        _ => crawl_failure_kind(error).map_or("spider_run_error", CrawlFailureKind::as_str),
    }
}

/// A domain waiting for a URL-pattern review is released by the review's
/// decision; this bound only keeps an undecided review from parking it
/// forever.
const URL_PATTERN_REVIEW_WAIT: Duration = SPIDER_RECRAWL_INTERVAL;

#[derive(Clone, Copy)]
enum SpiderSlotStatus {
    Succeeded,
    Truncated,
    /// A shop's rate-limit answer ended the crawl.
    RateLimited,
    /// The crawl stopped classifying at a pending URL-pattern review.
    AwaitingReview,
    /// The crawl waited on a URL-pattern review that was decided before the run recorded the
    /// wait. The decision released the domain, so the run records no outcome over it.
    ReviewDecided,
    Failed,
    /// Repeated deterministic failures; the domain is not retried until an
    /// operator acts.
    StoppedForOperator,
    /// The domain moved to another crawl root while the run crawled the old one, so the run
    /// records no outcome over the move.
    RootMoved,
    /// Another spider worker holds the domain lock.
    Deferred,
}

struct SpiderSlotOutcome {
    domain_id: CrawlerDomainId,
    status: SpiderSlotStatus,
}

fn next_crawl_after(cooldown: Duration) -> time::OffsetDateTime {
    time::OffsetDateTime::now_utc() + time::Duration::seconds(cooldown.as_secs() as i64)
}

/// The run's write of `outcome` found the domain on another crawl root: the move cleared the
/// old root's crawl state, and the run's outcome, which describes the old root, is not recorded
/// over it (`docs/crawler/operations.md#manual-authoring`). A failed run names its `failure`
/// here, since it writes no failure log either.
fn record_root_moved(
    candidate: &SpiderCandidate,
    outcome: &'static str,
    failure: Option<(&SpiderServiceError, &str)>,
) -> SpiderSlotStatus {
    info!(
        domain = %candidate.listing_source_domain,
        outcome,
        error = failure.map(|(error, _)| tracing::field::display(error)),
        error_kind = failure.map(|(_, error_kind)| error_kind),
        "Domain moved to another crawl root while the spider run crawled this one; the run's outcome is not recorded"
    );
    SpiderSlotStatus::RootMoved
}

/// A full crawl clears failure, truncation and cooldown state.
async fn record_completed_run(
    spider_candidates: &dyn SpiderCandidateService,
    candidate: &SpiderCandidate,
    result: &SpiderRunResult,
) -> SpiderSlotStatus {
    match spider_candidates
        .mark_crawl_completed(&candidate.crawled_root(), result.completion)
        .await
    {
        Ok(CrawlOutcomeWrite::Recorded) => {}
        Ok(CrawlOutcomeWrite::RootMoved) => {
            return record_root_moved(candidate, result.completion.as_str(), None);
        }
        Err(err) => warn!(
            error = ?err,
            domain = %candidate.listing_source_domain,
            "Failed to persist crawl completion metadata"
        ),
    }
    SpiderSlotStatus::Succeeded
}

/// Neither a success nor a failure: failure and truncation metadata stay
/// untouched, and the reviewer's decision releases the domain. A decision
/// that came first, such as a rejection while the crawl ran, already
/// released it; the domain keeps the crawl state that decision left.
async fn record_review_wait_run(
    spider_candidates: &dyn SpiderCandidateService,
    candidate: &SpiderCandidate,
    result: &SpiderRunResult,
    review_id: CrawlerReviewId,
) -> SpiderSlotStatus {
    let next_crawl_at = next_crawl_after(URL_PATTERN_REVIEW_WAIT);
    let rate_limit = result.completion.rate_limit();
    let rate_limit_write =
        rate_limit.map(|rate_limit| rate_limited_crawl_write(candidate, rate_limit).0);
    match spider_candidates
        .mark_crawl_awaiting_url_pattern_review(
            &candidate.crawled_root(),
            &review_id,
            rate_limit_write,
            next_crawl_at,
        )
        .await
    {
        Ok(UrlPatternReviewWaitWrite::Recorded) => {}
        Ok(UrlPatternReviewWaitWrite::RootMoved) => {
            return record_root_moved(candidate, AWAITING_URL_PATTERN_REVIEW_CRAWL_OUTCOME, None);
        }
        // The decision released the domain, but not from the shop's limit.
        Ok(UrlPatternReviewWaitWrite::ReviewNotPending) if let Some(rate_limit) = rate_limit => {
            return record_rate_limited_run(spider_candidates, candidate, result, rate_limit).await;
        }
        Ok(UrlPatternReviewWaitWrite::ReviewNotPending) => {
            info!(
                domain = %candidate.listing_source_domain,
                review_id = %review_id,
                completion = %result.completion,
                total_crawled = result.total_links,
                "URL pattern review was decided before the spider run recorded its wait; the domain keeps the crawl state the decision left"
            );
            return SpiderSlotStatus::ReviewDecided;
        }
        Err(err) => warn!(
            error = ?err,
            domain = %candidate.listing_source_domain,
            "Failed to persist crawl review-wait metadata"
        ),
    }
    info!(
        domain = %candidate.listing_source_domain,
        review_id = %review_id,
        completion = %result.completion,
        total_crawled = result.total_links,
        crawl_failure_count = candidate.crawl_failure_count,
        next_crawl_at = %next_crawl_at,
        "Spider run waits for a URL pattern review"
    );
    SpiderSlotStatus::AwaitingReview
}

/// Failure metadata stays untouched: a truncated crawl neither proves
/// recovery nor counts as a failure.
async fn record_truncated_run(
    spider_candidates: &dyn SpiderCandidateService,
    candidate: &SpiderCandidate,
    result: &SpiderRunResult,
) -> SpiderSlotStatus {
    let truncation_count = next_truncation_count(candidate);
    let cooldown = truncated_crawl_cooldown();
    let next_crawl_at = next_crawl_after(cooldown);

    match spider_candidates
        .mark_crawl_truncated(&candidate.crawled_root(), truncation_count, next_crawl_at)
        .await
    {
        Ok(CrawlOutcomeWrite::Recorded) => {}
        Ok(CrawlOutcomeWrite::RootMoved) => {
            return record_root_moved(
                candidate,
                CrawlCompletion::HardTimeoutTruncated.as_str(),
                None,
            );
        }
        Err(err) => warn!(
            error = ?err,
            domain = %candidate.listing_source_domain,
            "Failed to persist crawl truncation metadata"
        ),
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
    SpiderSlotStatus::Truncated
}

/// An incremental crawl between full crawls: it keeps the domain's full-crawl,
/// truncation and rate-limit state, and the next one is due a day after it.
/// What it left unvisited carries over like a full crawl's frontier.
async fn record_incremental_run(
    spider_candidates: &dyn SpiderCandidateService,
    candidate: &SpiderCandidate,
    result: &SpiderRunResult,
) -> SpiderSlotStatus {
    match spider_candidates
        .mark_incremental_crawl_finished(&candidate.crawled_root(), result.completion)
        .await
    {
        Ok(CrawlOutcomeWrite::Recorded) => {}
        Ok(CrawlOutcomeWrite::RootMoved) => {
            return record_root_moved(candidate, result.completion.as_str(), None);
        }
        Err(err) => warn!(
            error = ?err,
            domain = %candidate.listing_source_domain,
            "Failed to persist incremental crawl metadata"
        ),
    }
    info!(
        domain = %candidate.listing_source_domain,
        completion = %result.completion,
        total_crawled = result.total_links,
        product_urls_count = result.product_urls_count,
        known_product_urls_skipped = result.known_product_urls_skipped,
        "Incremental spider run finished"
    );
    match result.completion.is_full() {
        true => SpiderSlotStatus::Succeeded,
        false => SpiderSlotStatus::Truncated,
    }
}

/// Failure and truncation metadata stay untouched: the shop asked for less
/// traffic, which says nothing about the site's health or size.
async fn record_rate_limited_run(
    spider_candidates: &dyn SpiderCandidateService,
    candidate: &SpiderCandidate,
    result: &SpiderRunResult,
    rate_limit: RateLimitSignal,
) -> SpiderSlotStatus {
    let (write, cooldown) = rate_limited_crawl_write(candidate, rate_limit);

    match spider_candidates
        .mark_crawl_rate_limited(&candidate.crawled_root(), write)
        .await
    {
        Ok(CrawlOutcomeWrite::Recorded) => {}
        Ok(CrawlOutcomeWrite::RootMoved) => {
            return record_root_moved(candidate, RATE_LIMITED_CRAWL_OUTCOME, None);
        }
        Err(err) => warn!(
            error = ?err,
            domain = %candidate.listing_source_domain,
            "Failed to persist rate-limited crawl metadata"
        ),
    }
    warn!(
        domain = %candidate.listing_source_domain,
        completion = %result.completion,
        rate_limit_status = rate_limit.status,
        retry_after_seconds = ?rate_limit.retry_after.map(|retry_after| retry_after.as_secs()),
        total_crawled = result.total_links,
        product_urls_count = result.product_urls_count,
        rate_limit_streak = write.streak,
        crawl_failure_count = candidate.crawl_failure_count,
        next_crawl_at = %write.not_before,
        cooldown_seconds = cooldown.as_secs(),
        "Spider run stopped on a rate-limit answer; the next crawl runs slower"
    );
    SpiderSlotStatus::RateLimited
}

async fn record_failed_run(
    spider_candidates: &dyn SpiderCandidateService,
    candidate: &SpiderCandidate,
    error: &SpiderServiceError,
) -> SpiderSlotStatus {
    let error_kind = error_kind_for_spider_error(error);
    let failure_count = next_failure_count(candidate, error_kind);
    if needs_operator(error, failure_count) {
        return record_run_stopped_for_operator(
            spider_candidates,
            candidate,
            error,
            error_kind,
            failure_count,
        )
        .await;
    }
    let rate_limit = error
        .rate_limit()
        .map(|rate_limit| (rate_limit, rate_limited_crawl_write(candidate, rate_limit)));
    let failure_cooldown = cooldown_for_spider_failure(error, failure_count);
    let cooldown = rate_limit.map_or(failure_cooldown, |(_, (_, rate_limit_cooldown))| {
        failure_cooldown.max(rate_limit_cooldown)
    });
    let next_crawl_at = next_crawl_after(cooldown);

    match spider_candidates
        .mark_crawl_failure(
            &candidate.crawled_root(),
            error_kind,
            failure_count,
            rate_limit.map(|(_, (write, _))| write),
            next_crawl_at,
        )
        .await
    {
        Ok(CrawlOutcomeWrite::Recorded) => {}
        Ok(CrawlOutcomeWrite::RootMoved) => {
            return record_root_moved(
                candidate,
                FAILED_CRAWL_OUTCOME,
                Some((error.cause(), error_kind)),
            );
        }
        Err(err) => warn!(
            error = ?err,
            domain = %candidate.listing_source_domain,
            "Failed to persist crawl failure metadata"
        ),
    }
    log_spider_failure(
        candidate,
        error.cause(),
        error_kind,
        failure_count,
        next_crawl_at,
        cooldown,
    );
    if let Some((rate_limit, (write, _))) = rate_limit {
        warn!(
            domain = %candidate.listing_source_domain,
            rate_limit_status = rate_limit.status,
            retry_after_seconds = ?rate_limit.retry_after.map(|retry_after| retry_after.as_secs()),
            rate_limit_streak = write.streak,
            error_kind,
            "Spider run failed after a rate-limit answer stopped its crawl; the next crawl runs slower"
        );
    }
    SpiderSlotStatus::Failed
}

/// The failure is recorded like any other, but without a cooldown: the
/// domain leaves the schedule until an operator changes its configuration.
async fn record_run_stopped_for_operator(
    spider_candidates: &dyn SpiderCandidateService,
    candidate: &SpiderCandidate,
    error: &SpiderServiceError,
    error_kind: &str,
    failure_count: i32,
) -> SpiderSlotStatus {
    match spider_candidates
        .mark_crawl_needs_operator(&candidate.crawled_root(), error_kind, failure_count)
        .await
    {
        Ok(CrawlOutcomeWrite::Recorded) => {}
        Ok(CrawlOutcomeWrite::RootMoved) => {
            return record_root_moved(
                candidate,
                NEEDS_OPERATOR_CRAWL_OUTCOME,
                Some((error.cause(), error_kind)),
            );
        }
        Err(err) => warn!(
            error = ?err,
            domain = %candidate.listing_source_domain,
            "Failed to persist crawl needs-operator metadata"
        ),
    }
    warn!(
        domain = %candidate.listing_source_domain,
        error = %error.cause(),
        error_kind,
        failure_count,
        "Spider run kept failing; domain waits for an operator"
    );
    SpiderSlotStatus::StoppedForOperator
}

/// One warning per failed run, with the evidence the error variant carries.
fn log_spider_failure(
    candidate: &SpiderCandidate,
    error: &SpiderServiceError,
    error_kind: &str,
    failure_count: i32,
    next_crawl_at: time::OffsetDateTime,
    cooldown: Duration,
) {
    match error {
        SpiderServiceError::InsufficientInferenceSample {
            stage,
            sample_size,
            min_sample_size,
            diagnostics,
            ..
        } => warn!(
            domain = %candidate.listing_source_domain,
            error = %error,
            error_kind,
            failure_count,
            stage,
            sample_size = *sample_size,
            min_sample_size = *min_sample_size,
            completion = %diagnostics.completion,
            truncated = !diagnostics.completion.is_full(),
            stopped_early = ?diagnostics.stopped_early.map(|reason| reason.as_str()),
            crawl_budget_ms = diagnostics.crawl_budget_ms,
            seeds = diagnostics.seeds,
            known_skipped = diagnostics.known_skipped,
            root_reroot = ?diagnostics.root_reroot,
            diagnostic_reason = ?diagnostics.diagnostic_reason,
            initial_anti_bot = ?diagnostics.initial_anti_bot,
            pages_received = diagnostics.pages_received,
            accepted = diagnostics.accepted,
            rejected_host = diagnostics.rejected_host,
            rejected_blacklist = diagnostics.rejected_blacklist,
            rejected_duplicate = diagnostics.rejected_duplicate,
            lagged = diagnostics.lagged,
            status_counts = ?diagnostics.status_counts,
            bot_challenge_pages = diagnostics.bot_challenge_pages,
            robots_policy = ?diagnostics.robots_policy,
            robots_crawl_delay_ms = ?diagnostics.robots_crawl_delay_ms,
            request_delay_ms = diagnostics.request_delay_ms,
            robots_rules_failing_closed = diagnostics.robots_rules_failing_closed,
            next_crawl_at = %next_crawl_at,
            cooldown_seconds = cooldown.as_secs(),
            "Spider run failed"
        ),
        SpiderServiceError::CrawlFailed {
            total_links,
            diagnostics,
            ..
        } => warn!(
            domain = %candidate.listing_source_domain,
            error = %error,
            error_kind,
            failure_count,
            total_crawled = *total_links,
            min_required_links = 2,
            user_agent = CRAWLER_USER_AGENT,
            root_preflight_status = ?diagnostics.root_preflight_status,
            http_status = ?diagnostics.http_status,
            final_url = ?diagnostics.final_url,
            redirect_url = ?diagnostics.redirect_url,
            root_reroot = ?diagnostics.root_reroot,
            diagnostic_reason = ?diagnostics.diagnostic_reason,
            initial_html_length = ?diagnostics.initial_html_length,
            initial_should_retry = diagnostics.initial_should_retry,
            initial_anti_bot = ?diagnostics.initial_anti_bot,
            bot_challenge_pages = diagnostics.bot_challenge_pages,
            hard_block = ?diagnostics.hard_block,
            pages_received = diagnostics.pages_received,
            accepted = diagnostics.accepted,
            rejected_host = diagnostics.rejected_host,
            rejected_blacklist = diagnostics.rejected_blacklist,
            rejected_duplicate = diagnostics.rejected_duplicate,
            robots_policy = ?diagnostics.robots_policy,
            robots_crawl_delay_ms = ?diagnostics.robots_crawl_delay_ms,
            request_delay_ms = diagnostics.request_delay_ms,
            rate_limit = ?diagnostics.rate_limit,
            robots_rules_failing_closed = diagnostics.robots_rules_failing_closed,
            lagged = diagnostics.lagged,
            next_crawl_at = %next_crawl_at,
            cooldown_seconds = cooldown.as_secs(),
            "Spider run failed"
        ),
        SpiderServiceError::Discovery(
            preflight @ SpiderDiscoveryError::Preflight { reason, source, .. },
        ) => {
            warn!(
                domain = %candidate.listing_source_domain,
                error = %error,
                error_kind,
                failure_count,
                reason,
                redirect_url = ?preflight.redirect_target(),
                root_reroot = ?preflight.root_reroot(),
                source = ?source,
                next_crawl_at = %next_crawl_at,
                cooldown_seconds = cooldown.as_secs(),
                "Spider run failed"
            )
        }
        SpiderServiceError::NoProductUrls {
            total_links,
            product_pattern,
            completion,
            ..
        } => warn!(
            domain = %candidate.listing_source_domain,
            error = %error,
            error_kind,
            failure_count,
            total_crawled = *total_links,
            product_urls_count = 0,
            product_pattern,
            completion = %completion,
            truncated = !completion.is_full(),
            next_crawl_at = %next_crawl_at,
            cooldown_seconds = cooldown.as_secs(),
            "Spider run failed"
        ),
        SpiderServiceError::NoUrlPattern {
            total_links,
            completion,
            ..
        } => warn!(
            domain = %candidate.listing_source_domain,
            error = %error,
            error_kind,
            failure_count,
            total_crawled = *total_links,
            product_urls_count = 0,
            completion = %completion,
            truncated = !completion.is_full(),
            next_crawl_at = %next_crawl_at,
            cooldown_seconds = cooldown.as_secs(),
            "Spider run failed"
        ),
        _ => warn!(
            domain = %candidate.listing_source_domain,
            error = %error,
            error_kind,
            failure_count,
            next_crawl_at = %next_crawl_at,
            cooldown_seconds = cooldown.as_secs(),
            "Spider run failed"
        ),
    }
}

/// Persists the outcome of one spider run and reports how the slot ended.
async fn record_run_outcome(
    spider_candidates: &dyn SpiderCandidateService,
    candidate: &SpiderCandidate,
    run: &Result<SpiderRunResult, SpiderServiceError>,
) -> SpiderSlotStatus {
    match run {
        Ok(result) if let Some(review_id) = result.pending_url_pattern_review => {
            record_review_wait_run(spider_candidates, candidate, result, review_id).await
        }
        Ok(result)
            if candidate.kind == CrawlKind::Incremental
                && result.completion.rate_limit().is_none() =>
        {
            record_incremental_run(spider_candidates, candidate, result).await
        }
        Ok(result) if result.completion.is_full() => {
            record_completed_run(spider_candidates, candidate, result).await
        }
        Ok(result) if let Some(rate_limit) = result.completion.rate_limit() => {
            record_rate_limited_run(spider_candidates, candidate, result, rate_limit).await
        }
        Ok(result) => record_truncated_run(spider_candidates, candidate, result).await,
        Err(error) => record_failed_run(spider_candidates, candidate, error).await,
    }
}

/// The `last_crawl_outcome` a run left on its domain.
fn persisted_crawl_outcome(
    run: &Result<SpiderRunResult, SpiderServiceError>,
    status: SpiderSlotStatus,
) -> Option<&'static str> {
    match (status, run) {
        (SpiderSlotStatus::Succeeded, Ok(result)) => Some(result.completion.as_str()),
        (SpiderSlotStatus::Truncated, _) => Some(CrawlCompletion::HardTimeoutTruncated.as_str()),
        (SpiderSlotStatus::RateLimited, _) => Some(RATE_LIMITED_CRAWL_OUTCOME),
        (SpiderSlotStatus::AwaitingReview, _) => Some(AWAITING_URL_PATTERN_REVIEW_CRAWL_OUTCOME),
        (SpiderSlotStatus::Failed, _) => Some(FAILED_CRAWL_OUTCOME),
        (SpiderSlotStatus::StoppedForOperator, _) => Some(NEEDS_OPERATOR_CRAWL_OUTCOME),
        (SpiderSlotStatus::Succeeded, Err(_))
        | (SpiderSlotStatus::ReviewDecided, _)
        | (SpiderSlotStatus::RootMoved, _)
        | (SpiderSlotStatus::Deferred, _) => None,
    }
}

/// The product URL count and completion of a run that classified its URLs with the
/// domain's product pattern, including a pattern that matched nothing. An incremental crawl
/// counts the stored product pages it found linked and did not fetch again too, so a day
/// without new products does not read as a pattern that matched nothing.
fn classified_crawl(
    run: &Result<SpiderRunResult, SpiderServiceError>,
) -> Option<(usize, CrawlCompletion)> {
    match run {
        Ok(result) if result.pending_url_pattern_review.is_none() => Some((
            result.product_urls_count + result.known_product_urls_skipped,
            result.completion,
        )),
        Err(error) => match error.cause() {
            SpiderServiceError::NoProductUrls { completion, .. } => Some((0, *completion)),
            _ => None,
        },
        _ => None,
    }
}

fn emit_crawl_metrics(
    candidate: &SpiderCandidate,
    run: &Result<SpiderRunResult, SpiderServiceError>,
    status: SpiderSlotStatus,
) {
    let record = || {
        MetricRecord::for_listing_source_domain(
            &candidate.listing_source_id,
            &candidate.listing_source_domain,
        )
    };
    if let Some(outcome) = persisted_crawl_outcome(run, status) {
        record()
            .with_dimension(MetricDimension::CrawlOutcome, outcome)
            .with_value(CrawlerMetric::CrawlOutcome, 1)
            .emit();
    }
    if let Some((product_urls, completion)) = classified_crawl(run) {
        record()
            .with_value(CrawlerMetric::CrawlProductUrls, product_urls as u64)
            .with_value(
                CrawlerMetric::CrawlTruncated,
                u64::from(!completion.is_full()),
            )
            .emit();
    }
}

/// The crawl root is always `https://{host}`: the stored host is a bare host, never a URL, and
/// there is no plain-HTTP fallback. A host that merely starts with `http` (`httpbin.org`) is
/// still a host.
fn crawl_root_url(host: &str) -> String {
    format!("https://{host}")
}

fn spawn_spider_candidate(
    join_set: &mut JoinSet<SpiderSlotOutcome>,
    candidate: SpiderCandidate,
    spider_candidates: Arc<dyn SpiderCandidateService>,
    spider_service: Arc<dyn SpiderService>,
    lock_manager: Arc<crate::spider::local_lock::LocalLockManager>,
    threshold: usize,
) {
    let crawl_root_url = crawl_root_url(&candidate.listing_source_domain);
    let domain_id = candidate.domain_id;
    let pace = crawl_pace(&candidate);
    let span = tracing::info_span!(
        "spider_candidate",
        listing_source_id = %candidate.listing_source_id,
        domain_id = %candidate.domain_id,
        crawl_root_url = %crawl_root_url,
        kind = %candidate.kind
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
                    status: SpiderSlotStatus::Deferred,
                };
            };

            let run = spider_service
                .run(
                    &candidate.listing_source_id,
                    &candidate.domain_id,
                    &crawl_root_url,
                    threshold,
                    pace,
                    candidate.kind,
                )
                .await;
            let status = record_run_outcome(spider_candidates.as_ref(), &candidate, &run).await;
            emit_crawl_metrics(&candidate, &run, status);
            SpiderSlotOutcome { domain_id, status }
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
        let mut rate_limited = 0usize;
        let mut awaiting_review = 0usize;
        let mut review_decided = 0usize;
        let mut failed = 0usize;
        let mut stopped_for_operator = 0usize;
        let mut root_moved = 0usize;
        let mut started = false;
        let mut fetch_failed = false;
        let mut no_more_candidates = false;
        let mut deferred = 0usize;

        loop {
            while join_set.len() < spider_concurrency && !self.stop.is_requested() {
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
                        emit_pass_duration(CrawlerMetric::SpiderPassDuration, pass_start);
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
                    match outcome.status {
                        SpiderSlotStatus::Succeeded => succeeded += 1,
                        SpiderSlotStatus::Truncated => truncated += 1,
                        SpiderSlotStatus::RateLimited => rate_limited += 1,
                        SpiderSlotStatus::AwaitingReview => awaiting_review += 1,
                        SpiderSlotStatus::ReviewDecided => review_decided += 1,
                        SpiderSlotStatus::Failed => failed += 1,
                        SpiderSlotStatus::StoppedForOperator => stopped_for_operator += 1,
                        SpiderSlotStatus::RootMoved => root_moved += 1,
                        SpiderSlotStatus::Deferred => deferred += 1,
                    }
                }
                Some(Err(e)) => {
                    error!(error = %e, "Spider worker task failed to join");
                    failed += 1;
                }
                None => break,
            }
        }

        let duration_ms = emit_pass_duration(CrawlerMetric::SpiderPassDuration, pass_start);
        info!(
            total,
            succeeded,
            truncated,
            rate_limited,
            awaiting_review,
            review_decided,
            failed,
            stopped_for_operator,
            root_moved,
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
    use crate::spider::service::MockSpiderService;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::Notify;

    #[test]
    fn crawl_root_url_is_always_https_host() {
        for (host, expected) in [
            ("example.com", "https://example.com"),
            ("httpbin.org", "https://httpbin.org"),
            ("httpbin.org:8443", "https://httpbin.org:8443"),
            ("https-shop.example", "https://https-shop.example"),
        ] {
            assert_eq!(crawl_root_url(host), expected);
        }
    }

    #[tokio::test]
    async fn should_crawl_a_host_starting_with_http_from_its_https_root() {
        let domain_id = CrawlerDomainId::new();
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move {
                    Ok(vec![SpiderCandidate {
                        listing_source_domain: "httpbin.org".to_string(),
                        ..spider_candidate(domain_id)
                    }])
                })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(1)
            .withf(|_, _, crawl_root_url, _, _, _| crawl_root_url == "https://httpbin.org")
            .returning(|_, _, _, _, _, _| {
                Box::pin(async {
                    Err(crate::spider::service::SpiderServiceError::Database(
                        sqlx::Error::RowNotFound,
                    ))
                })
            });

        spider_job(spider_candidates, spider_service, 1)
            .run_spider_once()
            .await;
    }

    fn spider_candidate(domain_id: CrawlerDomainId) -> SpiderCandidate {
        SpiderCandidate {
            listing_source_id: ListingSourceId::new(),
            domain_id,
            listing_source_domain: "example.com".to_string(),
            crawl_failure_count: 0,
            last_crawl_error_kind: None,
            crawl_truncation_count: 0,
            crawl_rate_limit_streak: 0,
            kind: CrawlKind::Full,
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
    fn next_failure_count_continues_across_the_kinds_that_found_no_product_urls() {
        for (last, next) in [
            ("InsufficientInferenceSample", "NoProductUrls"),
            ("NoProductUrls", "NoUrlPattern"),
            ("NoUrlPattern", "InsufficientInferenceSample"),
        ] {
            let mut candidate = spider_candidate(CrawlerDomainId::new());
            candidate.crawl_failure_count = 2;
            candidate.last_crawl_error_kind = Some(last.to_string());

            assert_eq!(next_failure_count(&candidate, next), 3, "{last} -> {next}");
        }
    }

    #[test]
    fn next_failure_count_resets_between_a_no_product_url_kind_and_another_kind() {
        for (last, next) in [("NoUrlPattern", "TinyCrawl"), ("TinyCrawl", "NoUrlPattern")] {
            let mut candidate = spider_candidate(CrawlerDomainId::new());
            candidate.crawl_failure_count = 2;
            candidate.last_crawl_error_kind = Some(last.to_string());

            assert_eq!(next_failure_count(&candidate, next), 1, "{last} -> {next}");
        }
    }

    /// The streak's kind list is the persisted spelling of exactly the failures that found no
    /// product URL.
    #[test]
    fn no_product_url_error_kinds_should_be_the_kinds_of_the_failures_that_found_no_product_urls() {
        let mut kinds = [no_product_urls(), no_url_pattern(), insufficient_sample()]
            .iter()
            .inspect(|error| assert!(error.found_no_product_urls(), "{error:?}"))
            .map(error_kind_for_spider_error)
            .collect::<Vec<_>>();
        kinds.sort_unstable();

        assert_eq!(kinds, NO_PRODUCT_URL_ERROR_KINDS);
        assert!(!crawl_failed(CrawlFailureKind::TinyCrawl).found_no_product_urls());
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
            root_reroot: None,
        })
    }

    fn insufficient_sample() -> SpiderServiceError {
        SpiderServiceError::InsufficientInferenceSample {
            crawl_root_url: "https://example.com/".to_string(),
            stage: "end_of_crawl",
            sample_size: 2,
            min_sample_size: 20,
            diagnostics: Box::default(),
        }
    }

    fn no_product_urls() -> SpiderServiceError {
        SpiderServiceError::NoProductUrls {
            crawl_root_url: "https://example.com/".to_string(),
            total_links: 40,
            product_pattern: "/product/".to_string(),
            completion: CrawlCompletion::Completed,
        }
    }

    fn no_url_pattern() -> SpiderServiceError {
        SpiderServiceError::NoUrlPattern {
            crawl_root_url: "https://example.com/".to_string(),
            total_links: 40,
            completion: CrawlCompletion::Completed,
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
            .chain([insufficient_sample(), no_product_urls(), no_url_pattern()])
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
            no_product_urls(),
            no_url_pattern(),
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
    fn needs_operator_should_stop_deterministic_failures_at_the_escalation_count() {
        for error in [
            insufficient_sample(),
            no_product_urls(),
            no_url_pattern(),
            crawl_failed(CrawlFailureKind::RobotsBlocked),
            crawl_failed(CrawlFailureKind::RedirectProblem),
            preflight_failed(CrawlFailureKind::RedirectProblem),
            preflight_failed(CrawlFailureKind::TlsError),
        ] {
            assert!(!needs_operator(&error, NEEDS_OPERATOR_FAILURE_COUNT - 1));
            assert!(needs_operator(&error, NEEDS_OPERATOR_FAILURE_COUNT));
        }
    }

    /// A run that found no product URL among pages that were mostly bot challenges, whatever
    /// stopped its crawl, is a bot-protection failure: its own kind and streak, the transient
    /// cooldown, no step towards an operator and no product count for the pattern alarm.
    #[test]
    fn should_record_a_crawl_of_mostly_bot_challenges_as_bot_protection() {
        let bot_challenged = |failure| SpiderServiceError::BotChallengedCrawl {
            bot_challenge_pages: 30,
            pages_received: 31,
            failure: Box::new(failure),
        };
        for error in [
            bot_challenged(no_product_urls()),
            bot_challenged(no_url_pattern()),
            SpiderServiceError::RateLimitedCrawl {
                rate_limit: RateLimitSignal {
                    status: 429,
                    retry_after: None,
                },
                failure: Box::new(bot_challenged(insufficient_sample())),
            },
        ] {
            assert_eq!(error_kind_for_spider_error(&error), "BotProtection");
            assert!(!error.found_no_product_urls(), "{error:?}");
            assert_eq!(
                cooldown_for_spider_failure(&error, LONG_COOLDOWN_FAILURE_COUNT),
                TRANSIENT_CRAWL_LONG_COOLDOWN
            );
            assert!(!needs_operator(&error, i32::MAX));
            assert_eq!(classified_crawl(&Err(error)), None);
        }
    }

    #[test]
    fn needs_operator_should_keep_retrying_transient_blocked_and_unclassified_failures() {
        for error in ALL_KINDS
            .into_iter()
            .filter(|kind| kind.cooldown_profile() != CrawlCooldownProfile::Recoverable)
            .map(crawl_failed)
            .chain(unclassified_errors())
        {
            assert!(!needs_operator(&error, i32::MAX));
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
            error_kind_for_spider_error(&no_product_urls()),
            "NoProductUrls"
        );
        assert_eq!(
            error_kind_for_spider_error(&no_url_pattern()),
            "NoUrlPattern"
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

    fn incremental_result(
        completion: CrawlCompletion,
    ) -> Result<SpiderRunResult, SpiderServiceError> {
        Ok(SpiderRunResult {
            total_links: 40,
            product_urls_count: 3,
            product_pattern: Some("/product/".to_string()),
            completion,
            pending_url_pattern_review: None,
            known_product_urls_skipped: 0,
        })
    }

    #[tokio::test]
    async fn should_record_an_incremental_run_without_touching_full_crawl_or_truncation_state() {
        let domain_id = CrawlerDomainId::new();
        let candidate = SpiderCandidate {
            kind: CrawlKind::Incremental,
            ..spider_candidate(domain_id)
        };
        for (completion, status) in [
            (CrawlCompletion::Completed, "COMPLETED"),
            (
                CrawlCompletion::HardTimeoutTruncated,
                "HARD_TIMEOUT_TRUNCATED",
            ),
        ] {
            let mut spider_candidates = MockSpiderCandidateService::new();
            spider_candidates
                .expect_mark_incremental_crawl_finished()
                .withf(move |root, recorded| root.domain_id == domain_id && *recorded == completion)
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
            spider_candidates.expect_mark_crawl_completed().times(0);
            spider_candidates.expect_mark_crawl_truncated().times(0);
            let run = incremental_result(completion);

            let slot = record_run_outcome(&spider_candidates, &candidate, &run).await;

            assert_eq!(persisted_crawl_outcome(&run, slot), Some(status));
        }
    }

    #[tokio::test]
    async fn should_hold_back_an_incremental_run_a_shop_rate_limited_like_any_crawl() {
        let domain_id = CrawlerDomainId::new();
        let candidate = SpiderCandidate {
            kind: CrawlKind::Incremental,
            crawl_rate_limit_streak: 1,
            ..spider_candidate(domain_id)
        };
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_mark_incremental_crawl_finished()
            .times(0);
        spider_candidates
            .expect_mark_crawl_rate_limited()
            .withf(move |root, write| {
                root.domain_id == domain_id
                    && write.streak == 2
                    && next_crawl_at_is_about(
                        write.not_before,
                        rate_limited_crawl_cooldown(2, None),
                    )
            })
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

        let slot = record_run_outcome(
            &spider_candidates,
            &candidate,
            &incremental_result(rate_limited(None)),
        )
        .await;

        assert!(matches!(slot, SpiderSlotStatus::RateLimited));
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
            .withf(move |root, completion| {
                root.domain_id == expected_domain_id && *completion == CrawlCompletion::Completed
            })
            .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .withf(move |_, domain_id, _, _, _, _| *domain_id == expected_domain_id)
            .returning(|_, _, _, _, _, _| {
                Box::pin(async {
                    Ok(SpiderRunResult {
                        total_links: 10,
                        product_urls_count: 5,
                        product_pattern: None,
                        completion: CrawlCompletion::Completed,
                        pending_url_pattern_review: None,
                        known_product_urls_skipped: 0,
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
            .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

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
            .returning(move |_, domain_id, _, _, _, _| {
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
                        pending_url_pattern_review: None,
                        known_product_urls_skipped: 0,
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
    async fn should_finish_the_running_crawl_without_starting_another_once_stopped() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .once()
            .returning(|_, _| {
                Box::pin(async { Ok(vec![spider_candidate(CrawlerDomainId::new())]) })
            });
        spider_candidates
            .expect_mark_crawl_completed()
            .once()
            .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        let crawl_started = Arc::new(Notify::new());
        let release_crawl = Arc::new(Notify::new());
        let crawl_started_for_mock = Arc::clone(&crawl_started);
        let release_crawl_for_mock = Arc::clone(&release_crawl);
        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .once()
            .returning(move |_, _, _, _, _, _| {
                let crawl_started = Arc::clone(&crawl_started_for_mock);
                let release_crawl = Arc::clone(&release_crawl_for_mock);
                Box::pin(async move {
                    crawl_started.notify_one();
                    release_crawl.notified().await;
                    Ok(SpiderRunResult {
                        total_links: 10,
                        product_urls_count: 5,
                        product_pattern: None,
                        completion: CrawlCompletion::Completed,
                        pending_url_pattern_review: None,
                        known_product_urls_skipped: 0,
                    })
                })
            });
        let job = CrawlerCronJob::new(
            CrawlerCronConfig {
                spider_concurrency: 1,
                ..CrawlerCronConfig::default()
            },
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(MockScraperCandidateService::new()),
            Box::new(MockScraperService::new()),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );
        let stop = job.stop_handle();

        tokio::join!(job.run_spider_once(), async {
            crawl_started.notified().await;
            stop.request();
            release_crawl.notify_one();
        });
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
            .withf(move |root, error_kind, failure_count, _, next_crawl_at| {
                root.domain_id == expected_domain_id
                    && error_kind == "spider_run_error"
                    && *failure_count == 1
                    && next_crawl_at_is_about(
                        *next_crawl_at,
                        durable_retry_cooldown_for(NetworkErrorKind::Unknown),
                    )
            })
            .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, _, _, _, _| {
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
            .withf(move |root, error_kind, failure_count, _, next_crawl_at| {
                root.domain_id == expected_domain_id
                    && error_kind == "InsufficientInferenceSample"
                    && *failure_count == 1
                    && next_crawl_at_is_about(*next_crawl_at, CRAWL_RETRY_COOLDOWN)
            })
            .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, _, _, _, _| {
            Box::pin(async move {
                Err(
                    crate::spider::service::SpiderServiceError::InsufficientInferenceSample {
                        crawl_root_url: "https://example.com/".to_string(),
                        stage: "end_of_crawl",
                        sample_size: 16,
                        min_sample_size: 20,
                        diagnostics: Box::default(),
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
            .withf(move |root, error_kind, failure_count, _, next_crawl_at| {
                root.domain_id == expected_domain_id
                    && error_kind == "spider_run_error"
                    && *failure_count == 3
                    && next_crawl_at_is_about(
                        *next_crawl_at,
                        durable_retry_cooldown_for(NetworkErrorKind::Unknown) * 4,
                    )
            })
            .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, _, _, _, _| {
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
            .withf(move |root, error_kind, failure_count, _, next_crawl_at| {
                root.domain_id == expected_domain_id
                    && error_kind == "InsufficientInferenceSample"
                    && *failure_count == 3
                    && next_crawl_at_is_about(*next_crawl_at, RECOVERABLE_CRAWL_LONG_COOLDOWN)
            })
            .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, _, _, _, _| {
            Box::pin(async move {
                Err(
                    crate::spider::service::SpiderServiceError::InsufficientInferenceSample {
                        crawl_root_url: "https://example.com/".to_string(),
                        stage: "end_of_crawl",
                        sample_size: 16,
                        min_sample_size: 20,
                        diagnostics: Box::default(),
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
            .withf(move |root, error_kind, failure_count, _, next_crawl_at| {
                root.domain_id == expected_domain_id
                    && error_kind == expected_error_kind
                    && *failure_count == 1
                    && next_crawl_at_is_about(*next_crawl_at, CRAWL_RETRY_COOLDOWN)
            })
            .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .returning(move |_, _, _, _, _, _| Box::pin(async move { Err(crawl_failed(kind)) }));

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
            .withf(move |root, error_kind, failure_count, _, next_crawl_at| {
                root.domain_id == expected_domain_id
                    && error_kind == "ConnectError"
                    && *failure_count == 3
                    && next_crawl_at_is_about(*next_crawl_at, TRANSIENT_CRAWL_LONG_COOLDOWN)
            })
            .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, _, _, _, _| {
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
            .withf(move |root, error_kind, failure_count, _, next_crawl_at| {
                root.domain_id == expected_domain_id
                    && error_kind == "RateLimited"
                    && *failure_count == 3
                    && next_crawl_at_is_about(*next_crawl_at, TRANSIENT_CRAWL_LONG_COOLDOWN)
            })
            .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, _, _, _, _| {
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
    fn truncated_cooldown_should_continue_the_frontier_a_day_later_sooner_than_recrawl() {
        assert_eq!(
            truncated_crawl_cooldown(),
            Duration::from_secs(24 * 60 * 60)
        );
        assert!(truncated_crawl_cooldown() < SPIDER_RECRAWL_INTERVAL);
    }

    fn rate_limited(retry_after: Option<Duration>) -> CrawlCompletion {
        CrawlCompletion::RateLimited(RateLimitSignal {
            status: 429,
            retry_after,
        })
    }

    const HOUR: Duration = Duration::from_secs(60 * 60);

    #[test]
    fn rate_limited_cooldown_should_start_below_a_day_and_double_up_to_a_day() {
        assert_eq!(rate_limited_crawl_cooldown(1, None), HOUR);
        assert_eq!(rate_limited_crawl_cooldown(2, None), HOUR * 2);
        assert_eq!(rate_limited_crawl_cooldown(4, None), HOUR * 8);
        assert_eq!(
            rate_limited_crawl_cooldown(6, None),
            MAX_EXPONENTIAL_COOLDOWN
        );
        assert!(rate_limited_crawl_cooldown(1, None) < TRUNCATED_CRAWL_RETRY_COOLDOWN);
        for streak in [i32::MIN, -1, 0, 1, 31, 32, i32::MAX] {
            let cooldown = rate_limited_crawl_cooldown(streak, None);
            assert!(cooldown >= HOUR, "{streak}: {cooldown:?}");
            assert!(
                cooldown <= MAX_EXPONENTIAL_COOLDOWN,
                "{streak}: {cooldown:?}"
            );
        }
    }

    #[test]
    fn rate_limited_cooldown_should_never_retry_before_the_shop_s_retry_after() {
        assert_eq!(rate_limited_crawl_cooldown(1, Some(HOUR * 5)), HOUR * 5);
        assert_eq!(
            rate_limited_crawl_cooldown(3, Some(Duration::from_secs(120))),
            HOUR * 4,
            "a short Retry-After never undercuts the backoff of repeated rate limits"
        );
    }

    fn signal(retry_after: Option<Duration>) -> RateLimitSignal {
        RateLimitSignal {
            status: 429,
            retry_after,
        }
    }

    #[test]
    fn rate_limited_crawl_should_extend_the_streak_and_wait_for_its_cooldown() {
        let mut candidate = spider_candidate(CrawlerDomainId::new());
        candidate.crawl_rate_limit_streak = 2;

        let (write, cooldown) = rate_limited_crawl_write(&candidate, signal(None));
        assert_eq!(write.streak, 3);
        assert_eq!(cooldown, HOUR * 4);
        assert!(next_crawl_at_is_about(write.not_before, cooldown));
        assert_eq!(
            crawl_pace(&candidate),
            CrawlPace::after_rate_limited_crawls(2)
        );

        candidate.crawl_rate_limit_streak = -4;
        let (write, cooldown) = rate_limited_crawl_write(&candidate, signal(Some(HOUR * 3)));
        assert_eq!((write.streak, cooldown), (1, HOUR * 3));
        assert_eq!(crawl_pace(&candidate), CrawlPace::default());
    }

    #[tokio::test]
    async fn should_record_rate_limited_crawl_with_a_short_cooldown_and_crawl_slower_next_time() {
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
                    candidate.last_crawl_error_kind = Some("ConnectError".to_string());
                    candidate.crawl_rate_limit_streak = 1;
                    Ok(vec![candidate])
                })
            });
        spider_candidates
            .expect_mark_crawl_rate_limited()
            .times(1)
            .withf(move |root, write| {
                root.domain_id == expected_domain_id
                    && write.streak == 2
                    && next_crawl_at_is_about(write.not_before, HOUR * 2)
            })
            .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        spider_candidates.expect_mark_crawl_truncated().times(0);
        spider_candidates.expect_mark_crawl_completed().times(0);
        spider_candidates.expect_mark_crawl_failure().times(0);

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(1)
            .withf(|_, _, _, _, pace, _| *pace == CrawlPace::after_rate_limited_crawls(1))
            .returning(|_, _, _, _, _, _| {
                Box::pin(async { Ok(run_result(rate_limited(Some(Duration::from_secs(1_800))))) })
            });

        spider_job(spider_candidates, spider_service, 1)
            .run_spider_once()
            .await;
    }

    #[tokio::test]
    async fn should_wait_for_a_retry_after_longer_than_the_rate_limit_backoff() {
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
            .expect_mark_crawl_rate_limited()
            .times(1)
            .withf(move |root, write| {
                root.domain_id == expected_domain_id
                    && write.streak == 1
                    && next_crawl_at_is_about(write.not_before, HOUR * 5)
            })
            .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(1)
            .withf(|_, _, _, _, pace, _| *pace == CrawlPace::default())
            .returning(|_, _, _, _, _, _| {
                Box::pin(async { Ok(run_result(rate_limited(Some(HOUR * 5)))) })
            });

        spider_job(spider_candidates, spider_service, 1)
            .run_spider_once()
            .await;
    }

    /// The incident's shape: a 429 after a few dozen pages leaves too small a
    /// sample to infer a URL pattern, so the run fails with its own kind.
    #[tokio::test]
    async fn should_slow_the_next_crawl_and_honour_retry_after_when_a_rate_limited_run_fails() {
        let mut spider_candidates = failing_candidate_service(CrawlerDomainId::new(), 0, "None");
        spider_candidates
            .expect_mark_crawl_failure()
            .times(1)
            .withf(
                move |_, error_kind, failure_count, rate_limit, next_crawl_at| {
                    error_kind == "InsufficientInferenceSample"
                        && *failure_count == 1
                        && rate_limit.is_some_and(|write| {
                            write.streak == 1 && next_crawl_at_is_about(write.not_before, HOUR * 12)
                        })
                        && next_crawl_at_is_about(*next_crawl_at, HOUR * 12)
                },
            )
            .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        spider_candidates.expect_mark_crawl_rate_limited().times(0);

        spider_job(
            spider_candidates,
            spider_service_failing_with(|| SpiderServiceError::RateLimitedCrawl {
                rate_limit: RateLimitSignal {
                    status: 429,
                    retry_after: Some(Duration::from_secs(12 * 60 * 60)),
                },
                failure: Box::new(insufficient_sample()),
            }),
            1,
        )
        .run_spider_once()
        .await;
    }

    #[tokio::test]
    async fn should_keep_the_rate_limit_state_when_a_failure_saw_no_rate_limit() {
        let mut spider_candidates = failing_candidate_service(CrawlerDomainId::new(), 0, "None");
        spider_candidates
            .expect_mark_crawl_failure()
            .times(1)
            .withf(|_, _, _, rate_limit, _| rate_limit.is_none())
            .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

        spider_job(
            spider_candidates,
            spider_service_failing_with(insufficient_sample),
            1,
        )
        .run_spider_once()
        .await;
    }

    #[tokio::test]
    async fn should_slow_the_next_crawl_and_honour_retry_after_when_a_rate_limit_fails_the_crawl() {
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
            .expect_mark_crawl_failure()
            .times(1)
            .withf(
                move |root, error_kind, failure_count, rate_limit, next_crawl_at| {
                    root.domain_id == expected_domain_id
                        && error_kind == "RateLimited"
                        && *failure_count == 1
                        && rate_limit.is_some_and(|write| write.streak == 1)
                        && next_crawl_at_is_about(*next_crawl_at, HOUR * 20)
                },
            )
            .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        spider_candidates.expect_mark_crawl_rate_limited().times(0);

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(1)
            .returning(|_, _, _, _, _, _| {
                Box::pin(async {
                    let SpiderServiceError::CrawlFailed {
                        crawl_root_url,
                        kind,
                        total_links,
                        mut diagnostics,
                    } = crawl_failed(CrawlFailureKind::RateLimited)
                    else {
                        unreachable!("crawl_failed builds a crawl failure")
                    };
                    diagnostics.completion = rate_limited(Some(HOUR * 20));
                    Err(SpiderServiceError::RateLimitedCrawl {
                        rate_limit: signal(Some(HOUR * 20)),
                        failure: Box::new(SpiderServiceError::CrawlFailed {
                            crawl_root_url,
                            kind,
                            total_links,
                            diagnostics,
                        }),
                    })
                })
            });

        spider_job(spider_candidates, spider_service, 1)
            .run_spider_once()
            .await;
    }

    #[test]
    fn should_persist_a_rate_limited_crawl_as_rate_limited_not_as_a_hard_timeout() {
        let completion = rate_limited(None);
        let run = Ok(run_result(completion));

        assert_eq!(
            persisted_crawl_outcome(&run, SpiderSlotStatus::RateLimited),
            Some("RATE_LIMITED")
        );
        assert_eq!(completion.as_str(), RATE_LIMITED_CRAWL_OUTCOME);
    }

    fn run_result(completion: CrawlCompletion) -> SpiderRunResult {
        SpiderRunResult {
            total_links: 4_000,
            product_urls_count: 1_200,
            product_pattern: Some("/product/".to_string()),
            completion,
            pending_url_pattern_review: None,
            known_product_urls_skipped: 0,
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
            .withf(move |root, truncation_count, next_crawl_at| {
                root.domain_id == expected_domain_id
                    && *truncation_count == 2
                    && next_crawl_at_is_about(*next_crawl_at, truncated_crawl_cooldown())
            })
            .returning(|_, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        spider_candidates.expect_mark_crawl_completed().times(0);
        spider_candidates.expect_mark_crawl_failure().times(0);

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(1)
            .returning(|_, _, _, _, _, _| {
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
            .withf(move |root, completion| {
                root.domain_id == expected_domain_id
                    && *completion == CrawlCompletion::PageBudgetReached
            })
            .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        spider_candidates.expect_mark_crawl_truncated().times(0);

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(1)
            .returning(|_, _, _, _, _, _| {
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
            .withf(move |root, error_kind, failure_count, _, next_crawl_at| {
                root.domain_id == expected_domain_id
                    && error_kind == "ConnectError"
                    && *failure_count == 1
                    && next_crawl_at_is_about(*next_crawl_at, CRAWL_RETRY_COOLDOWN)
            })
            .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        spider_candidates.expect_mark_crawl_truncated().times(0);
        spider_candidates.expect_mark_crawl_completed().times(0);

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(1)
            .returning(|_, _, _, _, _, _| {
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
            .withf(move |root, truncation_count, _| {
                root.domain_id == truncated_domain_id && *truncation_count == 1
            })
            .returning(|_, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        spider_candidates
            .expect_mark_crawl_completed()
            .times(1)
            .withf(move |root, completion| {
                root.domain_id == completed_domain_id && *completion == CrawlCompletion::Completed
            })
            .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(2)
            .returning(move |_, domain_id, _, _, _, _| {
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
            .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .once()
            .returning(|_, _, _, _, _, _| {
                Box::pin(async {
                    Ok(SpiderRunResult {
                        total_links: 10,
                        product_urls_count: 5,
                        product_pattern: None,
                        completion: CrawlCompletion::Completed,
                        pending_url_pattern_review: None,
                        known_product_urls_skipped: 0,
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

    /// A spider service whose one run waits on the URL-pattern review `review_id`.
    fn spider_service_waiting_on(review_id: CrawlerReviewId) -> MockSpiderService {
        spider_service_waiting_after(review_id, CrawlCompletion::HardTimeoutTruncated)
    }

    fn spider_service_waiting_after(
        review_id: CrawlerReviewId,
        completion: CrawlCompletion,
    ) -> MockSpiderService {
        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(1)
            .returning(move |_, _, _, _, _, _| {
                Box::pin(async move {
                    Ok(SpiderRunResult {
                        pending_url_pattern_review: Some(review_id),
                        product_urls_count: 0,
                        product_pattern: None,
                        ..run_result(completion)
                    })
                })
            });
        spider_service
    }

    #[tokio::test]
    async fn should_record_a_rate_limited_crawl_even_when_its_review_was_decided_first() {
        let review_id = CrawlerReviewId::new();
        let mut spider_candidates = MockSpiderCandidateService::new();
        let domain_id = CrawlerDomainId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let first_call = excluded.is_empty();
                Box::pin(async move {
                    Ok(if first_call {
                        vec![spider_candidate(domain_id)]
                    } else {
                        vec![]
                    })
                })
            });
        spider_candidates
            .expect_mark_crawl_awaiting_url_pattern_review()
            .times(1)
            .withf(|_, _, rate_limit, _| rate_limit.is_some_and(|write| write.streak == 1))
            .returning(|_, _, _, _| {
                Box::pin(async { Ok(UrlPatternReviewWaitWrite::ReviewNotPending) })
            });
        spider_candidates
            .expect_mark_crawl_rate_limited()
            .times(1)
            .withf(move |root, write| {
                root.domain_id == domain_id
                    && write.streak == 1
                    && next_crawl_at_is_about(write.not_before, HOUR)
            })
            .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

        spider_job(
            spider_candidates,
            spider_service_waiting_after(review_id, rate_limited(None)),
            1,
        )
        .run_spider_once()
        .await;
    }

    /// A candidate service with one due domain that records no outcome but a review wait, which
    /// its write reports as `wait_write`.
    fn candidate_service_recording_review_wait(
        domain_id: CrawlerDomainId,
        review_id: CrawlerReviewId,
        wait_write: UrlPatternReviewWaitWrite,
    ) -> MockSpiderCandidateService {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let first_call = excluded.is_empty();
                Box::pin(async move {
                    if !first_call {
                        return Ok(vec![]);
                    }
                    let mut candidate = spider_candidate(domain_id);
                    candidate.crawl_failure_count = 2;
                    candidate.last_crawl_error_kind = Some("RateLimited".to_string());
                    Ok(vec![candidate])
                })
            });
        spider_candidates
            .expect_mark_crawl_awaiting_url_pattern_review()
            .times(1)
            .withf(move |root, written_review_id, rate_limit, next_crawl_at| {
                root.domain_id == domain_id
                    && *written_review_id == review_id
                    && rate_limit.is_none()
                    && next_crawl_at_is_about(*next_crawl_at, URL_PATTERN_REVIEW_WAIT)
            })
            .returning(move |_, _, _, _| Box::pin(async move { Ok(wait_write) }));
        spider_candidates.expect_mark_crawl_failure().times(0);
        spider_candidates.expect_mark_crawl_completed().times(0);
        spider_candidates.expect_mark_crawl_truncated().times(0);
        spider_candidates
            .expect_mark_crawl_needs_operator()
            .times(0);
        spider_candidates
    }

    #[tokio::test]
    async fn should_record_review_wait_without_failure_or_completion_state() {
        let review_id = CrawlerReviewId::new();
        let spider_candidates = candidate_service_recording_review_wait(
            CrawlerDomainId::new(),
            review_id,
            UrlPatternReviewWaitWrite::Recorded,
        );

        spider_job(spider_candidates, spider_service_waiting_on(review_id), 1)
            .run_spider_once()
            .await;
    }

    fn failing_candidate_service(
        domain_id: CrawlerDomainId,
        failure_count: i32,
        last_error_kind: &'static str,
    ) -> MockSpiderCandidateService {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let first_call = excluded.is_empty();
                Box::pin(async move {
                    if !first_call {
                        return Ok(vec![]);
                    }
                    let mut candidate = spider_candidate(domain_id);
                    candidate.crawl_failure_count = failure_count;
                    candidate.last_crawl_error_kind = Some(last_error_kind.to_string());
                    Ok(vec![candidate])
                })
            });
        spider_candidates.expect_mark_crawl_completed().times(0);
        spider_candidates.expect_mark_crawl_truncated().times(0);
        spider_candidates
    }

    fn spider_service_failing_with(error: fn() -> SpiderServiceError) -> MockSpiderService {
        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(1)
            .returning(move |_, _, _, _, _, _| Box::pin(async move { Err(error()) }));
        spider_service
    }

    #[tokio::test]
    async fn should_record_crawl_without_product_urls_as_failure_instead_of_completion() {
        let expected_domain_id = CrawlerDomainId::new();
        let mut spider_candidates = failing_candidate_service(expected_domain_id, 3, "TinyCrawl");
        spider_candidates
            .expect_mark_crawl_failure()
            .times(1)
            .withf(move |root, error_kind, failure_count, _, next_crawl_at| {
                root.domain_id == expected_domain_id
                    && error_kind == "NoProductUrls"
                    && *failure_count == 1
                    && next_crawl_at_is_about(*next_crawl_at, CRAWL_RETRY_COOLDOWN)
            })
            .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        spider_candidates
            .expect_mark_crawl_needs_operator()
            .times(0);

        spider_job(
            spider_candidates,
            spider_service_failing_with(no_product_urls),
            1,
        )
        .run_spider_once()
        .await;
    }

    #[tokio::test]
    async fn should_stop_domain_for_operator_when_no_pattern_failures_keep_repeating() {
        let expected_domain_id = CrawlerDomainId::new();
        let mut spider_candidates = failing_candidate_service(
            expected_domain_id,
            NEEDS_OPERATOR_FAILURE_COUNT - 1,
            "NoUrlPattern",
        );
        spider_candidates
            .expect_mark_crawl_needs_operator()
            .times(1)
            .withf(move |root, error_kind, failure_count| {
                root.domain_id == expected_domain_id
                    && error_kind == "NoUrlPattern"
                    && *failure_count == NEEDS_OPERATOR_FAILURE_COUNT
            })
            .returning(|_, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        spider_candidates.expect_mark_crawl_failure().times(0);

        spider_job(
            spider_candidates,
            spider_service_failing_with(no_url_pattern),
            1,
        )
        .run_spider_once()
        .await;
    }

    /// Runs that alternate between the kinds that found no product URL reach the operator at
    /// the count a single repeated kind does.
    #[tokio::test]
    async fn should_stop_domain_for_operator_when_no_product_url_failures_alternate_kinds() {
        let expected_domain_id = CrawlerDomainId::new();
        let mut spider_candidates = failing_candidate_service(
            expected_domain_id,
            NEEDS_OPERATOR_FAILURE_COUNT - 1,
            "InsufficientInferenceSample",
        );
        spider_candidates
            .expect_mark_crawl_needs_operator()
            .times(1)
            .withf(move |root, error_kind, failure_count| {
                root.domain_id == expected_domain_id
                    && error_kind == "NoUrlPattern"
                    && *failure_count == NEEDS_OPERATOR_FAILURE_COUNT
            })
            .returning(|_, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        spider_candidates.expect_mark_crawl_failure().times(0);

        spider_job(
            spider_candidates,
            spider_service_failing_with(no_url_pattern),
            1,
        )
        .run_spider_once()
        .await;
    }

    mod crawl_metrics {
        use super::*;
        use crate::metrics::test_support::capture_metrics;

        #[tokio::test]
        async fn should_emit_outcome_product_count_and_truncation_when_a_crawl_is_truncated() {
            let (metrics, _guard) = capture_metrics();
            let mut spider_candidates = MockSpiderCandidateService::new();
            spider_candidates
                .expect_get_candidates()
                .returning(move |_, excluded| {
                    let first_call = excluded.is_empty();
                    Box::pin(async move {
                        Ok(if first_call {
                            vec![spider_candidate(CrawlerDomainId::new())]
                        } else {
                            vec![]
                        })
                    })
                });
            spider_candidates
                .expect_mark_crawl_truncated()
                .returning(|_, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
            let mut spider_service = MockSpiderService::new();
            spider_service.expect_run().returning(|_, _, _, _, _, _| {
                Box::pin(async { Ok(run_result(CrawlCompletion::HardTimeoutTruncated)) })
            });

            spider_job(spider_candidates, spider_service, 1)
                .run_spider_once()
                .await;

            assert_eq!(
                metrics.values_with(
                    CrawlerMetric::CrawlOutcome,
                    MetricDimension::CrawlOutcome,
                    "HARD_TIMEOUT_TRUNCATED"
                ),
                vec![1]
            );
            assert_eq!(
                metrics.values_with(
                    CrawlerMetric::CrawlProductUrls,
                    MetricDimension::Domain,
                    "example.com"
                ),
                vec![1_200]
            );
            assert_eq!(
                metrics.values_with(
                    CrawlerMetric::CrawlTruncated,
                    MetricDimension::Domain,
                    "example.com"
                ),
                vec![1]
            );
            assert_eq!(metrics.values(CrawlerMetric::SpiderPassDuration).len(), 1);
        }

        #[tokio::test]
        async fn should_emit_the_pass_duration_when_no_domain_is_due() {
            let (metrics, _guard) = capture_metrics();
            let mut spider_candidates = MockSpiderCandidateService::new();
            spider_candidates
                .expect_get_candidates()
                .returning(|_, _| Box::pin(async { Ok(vec![]) }));

            spider_job(spider_candidates, MockSpiderService::new(), 1)
                .run_spider_once()
                .await;

            let durations = metrics.values(CrawlerMetric::SpiderPassDuration);
            assert_eq!(durations.len(), 1);
            assert!(durations[0].0.is_empty(), "pass duration is crawler-wide");
        }

        /// A review decided before the run records its wait, such as one rejected while the
        /// crawl ran, leaves the domain's crawl state as the decision left it: the run writes
        /// nothing else and reports no crawl outcome.
        #[tokio::test]
        async fn should_record_no_outcome_when_the_url_pattern_review_is_decided_before_the_wait() {
            let (metrics, _guard) = capture_metrics();
            let review_id = CrawlerReviewId::new();
            let spider_candidates = candidate_service_recording_review_wait(
                CrawlerDomainId::new(),
                review_id,
                UrlPatternReviewWaitWrite::ReviewNotPending,
            );

            spider_job(spider_candidates, spider_service_waiting_on(review_id), 1)
                .run_spider_once()
                .await;

            assert!(
                metrics.values(CrawlerMetric::CrawlOutcome).is_empty(),
                "the run persisted no crawl outcome"
            );
        }

        #[test]
        fn should_record_a_pattern_that_matched_nothing_as_a_failed_crawl_with_zero_products() {
            let run = Err(no_product_urls());

            assert_eq!(
                persisted_crawl_outcome(&run, SpiderSlotStatus::Failed),
                Some("FAILED")
            );
            assert_eq!(
                classified_crawl(&run),
                Some((0, CrawlCompletion::Completed))
            );
        }

        #[test]
        fn should_count_the_stored_product_pages_an_incremental_crawl_skipped() {
            let run = Ok(SpiderRunResult {
                product_urls_count: 0,
                known_product_urls_skipped: 12,
                ..run_result(CrawlCompletion::Completed)
            });

            assert_eq!(
                classified_crawl(&run),
                Some((12, CrawlCompletion::Completed))
            );
        }

        #[test]
        fn should_record_no_product_count_when_the_crawl_waits_for_a_url_pattern_review() {
            let run = Ok(SpiderRunResult {
                pending_url_pattern_review: Some(CrawlerReviewId::new()),
                ..run_result(CrawlCompletion::Completed)
            });

            assert_eq!(
                persisted_crawl_outcome(&run, SpiderSlotStatus::AwaitingReview),
                Some("AWAITING_URL_PATTERN_REVIEW")
            );
            assert_eq!(classified_crawl(&run), None);
        }

        #[test]
        fn should_record_no_product_count_when_the_crawl_failed_before_classifying() {
            let run = Err(insufficient_sample());

            assert_eq!(
                persisted_crawl_outcome(&run, SpiderSlotStatus::StoppedForOperator),
                Some("NEEDS_OPERATOR")
            );
            assert_eq!(classified_crawl(&run), None);
        }
    }

    /// A domain registered again with another crawl root while a run crawled the old one
    /// (`docs/crawler/operations.md#manual-authoring`).
    mod crawl_root_moved_during_the_run {
        use super::*;
        use crate::local_db::crawler_domain_configuration_repository::CrawlerDomainConfigurationRepositoryImpl;
        use crate::review::repository::CrawlerReviewRepository;
        use crate::service::crawler_domain_configuration::CrawlerDomainConfigurationRepository;
        use crate::spider::candidate_service::SpiderCandidateServiceImpl;
        use crate::spider::classification::url_metadata_repository::UrlMetadataRepositoryImpl;
        use crate::spider::classification::url_pattern_repository::ListingSourceUrlPatternRepositoryImpl;
        use crate::spider::classification::url_pattern_service::{
            UrlPatternAuthoring, UrlPatternServiceImpl,
        };
        use crate::spider::crawl_frontier_repository::{
            CrawlFrontierRepository, CrawlFrontierRepositoryImpl,
        };
        use crate::spider::crawled_root::CrawledRoot;
        use crate::spider::discovery::frontier::CrawlDiscovery;
        use crate::spider::discovery::website_spider::{CrawledPage, MockSpider, SpiderCrawl};
        use crate::spider::service::{SpiderServiceConfig, SpiderServiceImpl};
        use crate::spider::utils::url::CrawledUrl;
        use listing_source_core::Domain;
        use sqlx::PgPool;
        use test_api::IntegrationTestService as _;
        use tokio::sync::{mpsc, oneshot};

        /// A candidate service whose every write of a run's outcome names `root` and finds the
        /// domain on another crawl root.
        fn candidate_service_after_the_move(root: CrawledRoot) -> MockSpiderCandidateService {
            let mut spider_candidates = MockSpiderCandidateService::new();
            let moved = || Box::pin(async { Ok::<_, sqlx::Error>(CrawlOutcomeWrite::RootMoved) });
            let named = move |written: &CrawledRoot| *written == root;
            let root_of = named.clone();
            spider_candidates
                .expect_mark_crawl_completed()
                .withf(move |written, _| root_of(written))
                .returning(move |_, _| moved());
            let root_of = named.clone();
            spider_candidates
                .expect_mark_crawl_truncated()
                .withf(move |written, _, _| root_of(written))
                .returning(move |_, _, _| moved());
            let root_of = named.clone();
            spider_candidates
                .expect_mark_incremental_crawl_finished()
                .withf(move |written, _| root_of(written))
                .returning(move |_, _| moved());
            let root_of = named.clone();
            spider_candidates
                .expect_mark_crawl_rate_limited()
                .withf(move |written, _| root_of(written))
                .returning(move |_, _| moved());
            let root_of = named.clone();
            spider_candidates
                .expect_mark_crawl_failure()
                .withf(move |written, _, _, _, _| root_of(written))
                .returning(move |_, _, _, _, _| moved());
            let root_of = named.clone();
            spider_candidates
                .expect_mark_crawl_needs_operator()
                .withf(move |written, _, _| root_of(written))
                .returning(move |_, _, _| moved());
            spider_candidates
                .expect_mark_crawl_awaiting_url_pattern_review()
                .withf(move |written, _, _, _| named(written))
                .returning(|_, _, _, _| {
                    Box::pin(async { Ok(UrlPatternReviewWaitWrite::RootMoved) })
                });
            spider_candidates
        }

        #[tokio::test]
        async fn should_record_no_outcome_whatever_the_run_ended_with() {
            let domain_id = CrawlerDomainId::new();
            let full = || spider_candidate(domain_id);
            let runs: [(
                &str,
                SpiderCandidate,
                Result<SpiderRunResult, SpiderServiceError>,
            ); 7] = [
                (
                    "completed",
                    full(),
                    Ok(run_result(CrawlCompletion::Completed)),
                ),
                (
                    "truncated",
                    full(),
                    Ok(run_result(CrawlCompletion::HardTimeoutTruncated)),
                ),
                ("rate-limited", full(), Ok(run_result(rate_limited(None)))),
                (
                    "waiting for a review",
                    full(),
                    Ok(SpiderRunResult {
                        pending_url_pattern_review: Some(CrawlerReviewId::new()),
                        ..run_result(rate_limited(None))
                    }),
                ),
                (
                    "incremental",
                    SpiderCandidate {
                        kind: CrawlKind::Incremental,
                        ..full()
                    },
                    Ok(run_result(CrawlCompletion::Completed)),
                ),
                ("failed", full(), Err(no_url_pattern())),
                (
                    "stopped for an operator",
                    SpiderCandidate {
                        crawl_failure_count: NEEDS_OPERATOR_FAILURE_COUNT - 1,
                        last_crawl_error_kind: Some("NoUrlPattern".to_string()),
                        ..full()
                    },
                    Err(no_url_pattern()),
                ),
            ];

            for (ended, candidate, run) in runs {
                let spider_candidates = candidate_service_after_the_move(candidate.crawled_root());

                let slot = record_run_outcome(&spider_candidates, &candidate, &run).await;

                assert!(matches!(slot, SpiderSlotStatus::RootMoved), "{ended}");
                assert_eq!(persisted_crawl_outcome(&run, slot), None, "{ended}");
            }
        }

        const OLD_ROOT: &str = "moving-shop.example.com";
        const NEW_ROOT: &str = "www.moving-shop.example.com";

        /// A crawl of the old root during which the domain is registered with `NEW_ROOT`. It
        /// then delivers product and listing pages of the old root and leaves one unvisited.
        fn spider_moving_the_root_mid_crawl(
            pool: PgPool,
            listing_source_id: ListingSourceId,
        ) -> MockSpider {
            let mut spider = MockSpider::new();
            spider.expect_crawl().times(1).return_once(move |request| {
                Box::pin(async move {
                    assert_eq!(request.crawl_root_url, format!("https://{OLD_ROOT}"));
                    let moved = CrawlerDomainConfigurationRepositoryImpl::new(pool)
                        .register(listing_source_id, Domain::try_from(NEW_ROOT).unwrap())
                        .await
                        .unwrap();
                    assert_eq!(moved.domain.as_str(), NEW_ROOT);
                    let page = |path: &str| {
                        let url = url::Url::parse(&format!("https://{OLD_ROOT}{path}")).unwrap();
                        CrawledPage::new(CrawledUrl::new(url))
                    };
                    let (pages_tx, pages) = mpsc::channel(8);
                    for path in ["/", "/item/1", "/item/2", "/category/1"] {
                        pages_tx.send(page(path)).await.unwrap();
                    }
                    let (diagnostics_tx, diagnostics) = oneshot::channel();
                    diagnostics_tx.send(CrawlDiagnostics::default()).unwrap();
                    let (discovery_tx, discovery) = oneshot::channel();
                    discovery_tx
                        .send(CrawlDiscovery {
                            frontier: vec![page("/category/2").url.as_url().clone()],
                        })
                        .unwrap();
                    Ok(SpiderCrawl {
                        pages,
                        diagnostics,
                        discovery,
                    })
                })
            });
            spider
        }

        /// The domain's crawl root, last full crawl, failure count, outcome and last run.
        type CrawlState = (
            String,
            Option<time::OffsetDateTime>,
            i32,
            Option<String>,
            Option<time::OffsetDateTime>,
        );

        async fn crawl_state(pool: &PgPool, domain_id: CrawlerDomainId) -> CrawlState {
            sqlx::query_as(
                "SELECT crawl_root_host, last_crawled, crawl_failure_count, last_crawl_outcome, \
                        last_crawl_run_at \
                 FROM listing_source_domains WHERE domain_id = $1",
            )
            .bind(domain_id.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap()
        }

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_keep_the_move_when_the_crawl_of_the_old_root_ends_after_it() {
            let pool = test_api::get_postgres_client().await;
            let listing_source_id = ListingSourceId::new();
            sqlx::query(
                "INSERT INTO listing_sources \
                 (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled) \
                 VALUES ($1, 'Moving shop', 'moving-shop', TRUE)",
            )
            .bind(listing_source_id.as_uuid())
            .execute(&pool)
            .await
            .unwrap();
            let domain_id = CrawlerDomainConfigurationRepositoryImpl::new(pool.clone())
                .register(listing_source_id, Domain::try_from(OLD_ROOT).unwrap())
                .await
                .unwrap()
                .domain_id;
            sqlx::query(
                "UPDATE listing_source_domains \
                 SET url_pattern = '/item/', url_pattern_state = 'MATCHED', \
                     crawl_failure_count = 1, last_crawl_error_kind = 'ConnectError', \
                     last_crawl_outcome = 'FAILED', last_crawl_run_at = NOW() - INTERVAL '1 day' \
                 WHERE domain_id = $1",
            )
            .bind(domain_id.as_uuid())
            .execute(&pool)
            .await
            .unwrap();
            let spider_service = SpiderServiceImpl::new(
                SpiderServiceConfig::default(),
                Box::new(spider_moving_the_root_mid_crawl(
                    pool.clone(),
                    listing_source_id,
                )),
                Box::new(UrlPatternServiceImpl::new(
                    Arc::new(ListingSourceUrlPatternRepositoryImpl::new(pool.clone())),
                    UrlPatternAuthoring::Manual {
                        review_repository: CrawlerReviewRepository::new(pool.clone()),
                    },
                )),
                Arc::new(UrlMetadataRepositoryImpl::new(pool.clone())),
                Arc::new(CrawlFrontierRepositoryImpl::new(pool.clone())),
            );
            let mut scraper_candidates = MockScraperCandidateService::new();
            scraper_candidates
                .expect_get_candidates()
                .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));
            let job = CrawlerCronJob::new(
                CrawlerCronConfig {
                    spider_concurrency: 1,
                    ..CrawlerCronConfig::default()
                },
                Arc::new(LocalLockManager::new()),
                Box::new(SpiderCandidateServiceImpl::new(pool.clone())),
                Box::new(spider_service),
                Box::new(scraper_candidates),
                Box::new(MockScraperService::new()),
                noop_listing_source_registration(),
                noop_raw_capture(),
            );

            job.run_spider_once().await;

            assert_eq!(
                crawl_state(&pool, domain_id).await,
                (NEW_ROOT.to_owned(), None, 0, None, None),
                "the move's cleared crawl state survives the old root's completed crawl"
            );
            assert!(
                CrawlFrontierRepositoryImpl::new(pool.clone())
                    .load_frontier(&domain_id)
                    .await
                    .unwrap()
                    .is_empty(),
                "the old root's frontier is not kept"
            );
            let stored_urls: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM listing_source_urls WHERE domain_id = $1")
                    .bind(domain_id.as_uuid())
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(
                stored_urls, 4,
                "the crawled URLs belong to the site and stay"
            );
            let next = SpiderCandidateServiceImpl::new(pool.clone())
                .get_candidates(10, &[])
                .await
                .unwrap();
            assert_eq!(next.len(), 1);
            assert_eq!(
                (next[0].listing_source_domain.as_str(), next[0].kind),
                (NEW_ROOT, CrawlKind::Full),
                "the next spider cycle crawls the new root in full"
            );
        }
    }
}
