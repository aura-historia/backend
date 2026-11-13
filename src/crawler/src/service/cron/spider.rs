use super::job::CrawlerCronJob;
use super::metrics::emit_pass_record;
use crate::CrawlerReviewId;
use crate::metrics::{CrawlerMetric, MetricRecord};
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
    CrawlBudgetScale, CrawlCompletion, CrawlCooldownProfile, CrawlFailureKind, CrawlPace,
    RobotsUnreachable, SpiderDiscoveryError,
};
use crate::spider::local_lock::{WorkLock, WorkLockScope};
use crate::spider::service::{SpiderRunResult, SpiderService, SpiderServiceError};
use listing_source_core::ListingSourceId;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;
use tracing::{Instrument, debug, error, info, warn};

const CRAWL_RETRY_COOLDOWN: Duration = Duration::from_secs(6 * 60 * 60);
const TRANSIENT_CRAWL_LONG_COOLDOWN: Duration = Duration::from_secs(24 * 60 * 60);
const RECOVERABLE_CRAWL_LONG_COOLDOWN: Duration = Duration::from_secs(3 * 24 * 60 * 60);
const LONG_COOLDOWN_FAILURE_COUNT: i32 = 3;
const TRUNCATED_CRAWL_RETRY_COOLDOWN: Duration = Duration::from_secs(24 * 60 * 60);
const TRUNCATED_CRAWL_CONTINUATION_COOLDOWN: Duration = Duration::from_secs(6 * 60 * 60);
/// A crawl the crawler's shutdown ended is due again once the crawler is back: the hold only
/// keeps a restart loop from starting it before the process has settled.
const STOPPED_CRAWL_RETRY_COOLDOWN: Duration = Duration::from_secs(60);
const RATE_LIMITED_CRAWL_RETRY_COOLDOWN: Duration = Duration::from_secs(60 * 60);

/// The cooldown after the `failure_count`th failure of a streak that does not wait for an
/// operator yet: two short retries, then the profile's long cooldown. A durable block waits for
/// an operator before its long cooldown would begin
/// ([`DURABLE_BLOCK_NEEDS_OPERATOR_FAILURE_COUNT`]), so it only ever takes the short retry.
fn cooldown_for_failure_count(profile: CrawlCooldownProfile, failure_count: i32) -> Duration {
    let long = failure_count >= LONG_COOLDOWN_FAILURE_COUNT;
    match profile {
        CrawlCooldownProfile::Transient if long => TRANSIENT_CRAWL_LONG_COOLDOWN,
        CrawlCooldownProfile::Recoverable if long => RECOVERABLE_CRAWL_LONG_COOLDOWN,
        _ => CRAWL_RETRY_COOLDOWN,
    }
}

/// Whether a truncated full crawl ran out of its wall-clock budget with URLs left unvisited, so
/// the shop is too large for one crawl. A crawl that ended early and left only pages it could
/// not fetch, such as bot challenges, or that could not tell its frontier, is no such
/// continuation (`docs/crawler/scheduling-and-retries.md#discovery-coverage`).
fn ran_out_of_time_with_frontier(result: &SpiderRunResult) -> bool {
    result.ran_out_of_time
        && result
            .frontier_left
            .is_some_and(|frontier_left| frontier_left > 0)
}

/// A full crawl cut short continues from the frontier it left, however often it was cut short
/// before: each crawl visits pages the previous ones did not reach, so a large shop is covered in
/// steps instead of restarting from the root. A crawl that ran out of time with URLs left
/// continues a few hours later; any other waits a day
/// (`docs/crawler/scheduling-and-retries.md#discovery-coverage`).
fn truncated_crawl_cooldown(result: &SpiderRunResult) -> Duration {
    if ran_out_of_time_with_frontier(result) {
        TRUNCATED_CRAWL_CONTINUATION_COOLDOWN
    } else {
        TRUNCATED_CRAWL_RETRY_COOLDOWN
    }
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
/// whatever its outcome: the ListingSource's rate-limit streak grows by one and its
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

/// The pace of the ListingSource's next crawl, slowed by its rate-limit streak.
fn crawl_pace(candidate: &SpiderCandidate) -> CrawlPace {
    CrawlPace::after_rate_limited_crawls(
        u32::try_from(candidate.crawl_rate_limit_streak).unwrap_or(0),
    )
}

/// At most one spider slot in this many runs an extended crawl at a time, so the continuations of
/// large shops cannot hold every slot for several budgets while other shops wait.
const EXTENDED_CRAWL_SLOT_SHARE: usize = 3;

/// How many crawls of a pass may run an extended budget at once: none below
/// [`EXTENDED_CRAWL_SLOT_SHARE`] slots.
fn extended_crawl_slots(spider_concurrency: usize) -> usize {
    spider_concurrency / EXTENDED_CRAWL_SLOT_SHARE
}

/// The budget of the ListingSource's next crawl. A full crawl that continues truncated ones runs
/// longer while an extended-crawl slot is free; an incremental crawl, a crawl of a shop that
/// rate-limited the crawler, which asked for fewer requests, and a crawl when no extended slot
/// is free run the configured budget
/// (`docs/crawler/scheduling-and-retries.md#discovery-coverage`).
fn crawl_budget_scale(candidate: &SpiderCandidate, extended_slot_free: bool) -> CrawlBudgetScale {
    if candidate.kind != CrawlKind::Full
        || candidate.crawl_rate_limit_streak > 0
        || !extended_slot_free
    {
        return CrawlBudgetScale::STANDARD;
    }
    CrawlBudgetScale::after_truncated_crawls(
        u32::try_from(candidate.crawl_truncation_count).unwrap_or(0),
    )
}

/// The truncation count counts consecutive crawls that ran out of time with URLs left, which
/// [`crawl_budget_scale`] extends the next crawl by; any other truncated crawl ends the run of
/// them, so a shop that fits one crawl but leaves pages it cannot fetch gets no extended budget.
fn next_truncation_count(candidate: &SpiderCandidate, result: &SpiderRunResult) -> i32 {
    if ran_out_of_time_with_frontier(result) {
        candidate.crawl_truncation_count.max(0).saturating_add(1)
    } else {
        0
    }
}

/// Persisted error kinds of the failures whose run delivered pages but found no product URL
/// ([`SpiderServiceError::found_no_product_urls`]). They count as one failure streak, so a site
/// without a catalogue whose runs alternate between them is escalated like one that repeats a
/// single kind: each such run moves the ListingSource's crawl frontier on, and the streak bounds
/// how often it does (`docs/crawler/scheduling-and-retries.md#discovery-coverage`).
const NO_PRODUCT_URL_ERROR_KINDS: [&str; 3] = [
    "InsufficientInferenceSample",
    "NoProductUrls",
    "NoUrlPattern",
];

/// Whether `error_kind` belongs to the one shared streak: the kinds that found no product URL
/// and `BotProtection`. A shop that challenges its crawls keeps its product pages away as a
/// missing catalogue does, so a run alternating between the two escalates like either alone.
fn in_shared_failure_streak(error_kind: &str) -> bool {
    NO_PRODUCT_URL_ERROR_KINDS.contains(&error_kind)
        || error_kind == CrawlFailureKind::BotProtection.as_str()
}

/// Whether a failure of `error_kind` continues the streak of the ListingSource's last failure.
fn continues_failure_streak(last_error_kind: Option<&str>, error_kind: &str) -> bool {
    last_error_kind.is_some_and(|last| {
        last == error_kind
            || (in_shared_failure_streak(last) && in_shared_failure_streak(error_kind))
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

/// Consecutive failures of one streak ([`continues_failure_streak`]) after which a ListingSource
/// stops retrying and waits for an operator. Every classified failure escalates, since a shop
/// that keeps failing the same way costs a crawl preflight every few days and only a person can
/// tell whether it recovers, needs another crawl root or should stop being crawled. With two
/// short retries and then the long cooldown, the fifth failure comes about two and a half days
/// after the first for a transient kind (`ConnectError`, `CloudflareChallenge`,
/// `BotProtection`, ...) and about a week after it for a recoverable one (a TLS, robots.txt or
/// redirect problem, or a crawl without product URLs).
const NEEDS_OPERATOR_FAILURE_COUNT: i32 = 5;

/// A shop that refuses the crawler (`AccessDenied`, `JavascriptRequired`) does not let it in
/// by itself, so its second failure in a row, one short retry after the first, already waits
/// for an operator instead of cooling down for long.
const DURABLE_BLOCK_NEEDS_OPERATOR_FAILURE_COUNT: i32 = 2;
const _: () = assert!(DURABLE_BLOCK_NEEDS_OPERATOR_FAILURE_COUNT <= LONG_COOLDOWN_FAILURE_COUNT);

const fn needs_operator_failure_count(profile: CrawlCooldownProfile) -> i32 {
    match profile {
        CrawlCooldownProfile::Transient | CrawlCooldownProfile::Recoverable => {
            NEEDS_OPERATOR_FAILURE_COUNT
        }
        CrawlCooldownProfile::DurableBlock => DURABLE_BLOCK_NEEDS_OPERATOR_FAILURE_COUNT,
    }
}

/// Whether the failure, the `failure_count`th of its streak, stops the ListingSource for an
/// operator. Unclassified internal or configuration faults are the crawler's own and never do:
/// they keep their capped exponential retry ([`unclassified_cooldown_for_failure_count`]).
fn needs_operator(error: &SpiderServiceError, failure_count: i32) -> bool {
    crawl_cooldown_profile(error)
        .is_some_and(|profile| failure_count >= needs_operator_failure_count(profile))
}

/// Cooldown for the catch-all `spider_run_error` case (internal or config
/// faults without a crawl profile). Starts at the generic durable retry
/// cooldown and escalates like the scrape circuit, so a persistent
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

/// A ListingSource waiting for a URL-pattern review is released by the review's
/// decision; this bound only keeps an undecided review from parking it
/// forever.
const URL_PATTERN_REVIEW_WAIT: Duration = SPIDER_RECRAWL_INTERVAL;

#[derive(Clone, Copy)]
enum SpiderSlotStatus {
    Succeeded,
    Truncated,
    /// The crawler's shutdown ended the crawl.
    Stopped,
    /// A shop's rate-limit answer ended the crawl.
    RateLimited,
    /// The crawl stopped classifying at a pending URL-pattern review.
    AwaitingReview,
    /// The crawl waited on a URL-pattern review that was decided before the run recorded the
    /// wait. The decision released the ListingSource, so the run records no outcome over it.
    ReviewDecided,
    Failed,
    /// A failure streak reached its escalation count; the ListingSource is not retried until an
    /// operator acts.
    StoppedForOperator,
    /// The ListingSource moved to another crawl root while the run crawled the old one, so the run
    /// records no outcome over the move.
    RootMoved,
    /// Another spider worker holds the spider ListingSource lock.
    Deferred,
}

struct SpiderSlotOutcome {
    listing_source_id: ListingSourceId,
    status: SpiderSlotStatus,
}

fn next_crawl_after(cooldown: Duration) -> time::OffsetDateTime {
    time::OffsetDateTime::now_utc() + time::Duration::seconds(cooldown.as_secs() as i64)
}

/// The run's write of `outcome` found the ListingSource on another crawl root: the move cleared the
/// old root's crawl state, and the run's outcome, which describes the old root, is not recorded
/// over it (`docs/crawler/operations.md#manual-authoring`). A failed run names its `failure`
/// here, since it writes no failure log either.
fn record_root_moved(
    candidate: &SpiderCandidate,
    outcome: &'static str,
    failure: Option<(&SpiderServiceError, &str)>,
) -> SpiderSlotStatus {
    info!(
        event = ROOT_MOVED_EVENT,
        listing_source_id = %candidate.listing_source_id,
        listing_source_name = %candidate.listing_source_name,
        domain = %candidate.crawl_root.host(),
        outcome,
        error = failure.map(|(error, _)| tracing::field::display(error)),
        error_kind = failure.map(|(_, error_kind)| error_kind),
        "ListingSource moved to another crawl root while the spider run crawled this one; the run's outcome is not recorded"
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
            listing_source_id = %candidate.listing_source_id,
            listing_source_name = %candidate.listing_source_name,
            domain = %candidate.crawl_root.host(),
            "Failed to persist crawl completion metadata"
        ),
    }
    SpiderSlotStatus::Succeeded
}

/// Neither a success nor a failure: failure and truncation metadata stay
/// untouched, and the reviewer's decision releases the ListingSource. A decision
/// that came first, such as a rejection while the crawl ran, already
/// released it; the ListingSource keeps the crawl state that decision left.
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
        // The decision released the ListingSource, but not from the shop's limit.
        Ok(UrlPatternReviewWaitWrite::ReviewNotPending) if let Some(rate_limit) = rate_limit => {
            return record_rate_limited_run(spider_candidates, candidate, result, rate_limit).await;
        }
        Ok(UrlPatternReviewWaitWrite::ReviewNotPending) => {
            debug!(
                listing_source_id = %candidate.listing_source_id,
                listing_source_name = %candidate.listing_source_name,
                domain = %candidate.crawl_root.host(),
                review_id = %review_id,
                completion = %result.completion,
                total_crawled = result.total_links,
                "URL pattern review was decided before the spider run recorded its wait; the ListingSource keeps the crawl state the decision left"
            );
            return SpiderSlotStatus::ReviewDecided;
        }
        Err(err) => warn!(
            error = ?err,
            listing_source_id = %candidate.listing_source_id,
            listing_source_name = %candidate.listing_source_name,
            domain = %candidate.crawl_root.host(),
            "Failed to persist crawl review-wait metadata"
        ),
    }
    debug!(
        listing_source_id = %candidate.listing_source_id,
        listing_source_name = %candidate.listing_source_name,
        domain = %candidate.crawl_root.host(),
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
    let truncation_count = next_truncation_count(candidate, result);
    let cooldown = truncated_crawl_cooldown(result);
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
            listing_source_id = %candidate.listing_source_id,
            listing_source_name = %candidate.listing_source_name,
            domain = %candidate.crawl_root.host(),
            "Failed to persist crawl truncation metadata"
        ),
    }
    info!(
        event = CRAWL_TRUNCATED_EVENT,
        listing_source_id = %candidate.listing_source_id,
        listing_source_name = %candidate.listing_source_name,
        domain = %candidate.crawl_root.host(),
        completion = %result.completion,
        total_crawled = result.total_links,
        product_urls_count = result.product_urls_count,
        frontier_left = result.frontier_left,
        ran_out_of_time = result.ran_out_of_time,
        db_wait_ms = u64::try_from(result.db_wait.as_millis()).unwrap_or(u64::MAX),
        truncation_count,
        crawl_failure_count = candidate.crawl_failure_count,
        next_crawl_at = %next_crawl_at,
        cooldown_seconds = cooldown.as_secs(),
        "Spider run truncated by hard timeout"
    );
    SpiderSlotStatus::Truncated
}

/// A crawl the crawler's shutdown ended, or whose too small sample the shutdown caused, is
/// neither a truncation nor a failure: it keeps the frontier it left and is due again soon.
async fn record_stopped_run(
    spider_candidates: &dyn SpiderCandidateService,
    candidate: &SpiderCandidate,
) -> SpiderSlotStatus {
    let next_crawl_at = next_crawl_after(STOPPED_CRAWL_RETRY_COOLDOWN);
    match spider_candidates
        .mark_crawl_stopped(&candidate.crawled_root(), next_crawl_at)
        .await
    {
        Ok(CrawlOutcomeWrite::Recorded) => {}
        Ok(CrawlOutcomeWrite::RootMoved) => {
            return record_root_moved(candidate, CrawlCompletion::Stopped.as_str(), None);
        }
        Err(err) => warn!(
            error = ?err,
            listing_source_id = %candidate.listing_source_id,
            listing_source_name = %candidate.listing_source_name,
            domain = %candidate.crawl_root.host(),
            "Failed to persist stopped crawl metadata"
        ),
    }
    debug!(
        listing_source_id = %candidate.listing_source_id,
        listing_source_name = %candidate.listing_source_name,
        domain = %candidate.crawl_root.host(),
        next_crawl_at = %next_crawl_at,
        "Spider run stopped by the crawler's shutdown; the next crawl resumes from its frontier"
    );
    SpiderSlotStatus::Stopped
}

/// An incremental crawl between full crawls: it keeps the ListingSource's full-crawl,
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
            listing_source_id = %candidate.listing_source_id,
            listing_source_name = %candidate.listing_source_name,
            domain = %candidate.crawl_root.host(),
            "Failed to persist incremental crawl metadata"
        ),
    }
    debug!(
        listing_source_id = %candidate.listing_source_id,
        listing_source_name = %candidate.listing_source_name,
        domain = %candidate.crawl_root.host(),
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
            listing_source_id = %candidate.listing_source_id,
            listing_source_name = %candidate.listing_source_name,
            domain = %candidate.crawl_root.host(),
            "Failed to persist rate-limited crawl metadata"
        ),
    }
    warn!(
        event = CRAWL_RATE_LIMITED_EVENT,
        listing_source_id = %candidate.listing_source_id,
        listing_source_name = %candidate.listing_source_name,
        domain = %candidate.crawl_root.host(),
        completion = %result.completion,
        rate_limit_status = rate_limit.status,
        retry_after_seconds = rate_limit.retry_after.map(|retry_after| retry_after.as_secs()),
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
            listing_source_id = %candidate.listing_source_id,
            listing_source_name = %candidate.listing_source_name,
            domain = %candidate.crawl_root.host(),
            "Failed to persist crawl failure metadata"
        ),
    }
    log_spider_failure(
        candidate,
        error.cause(),
        error_kind,
        failure_count,
        FailureDisposition::Retry {
            next_crawl_at,
            cooldown,
            rate_limit: rate_limit.map(|(signal, (write, _))| RateLimitFacts {
                signal,
                streak: write.streak,
            }),
        },
    );
    SpiderSlotStatus::Failed
}

/// The failure is recorded like any other, but without a cooldown: the
/// ListingSource leaves the schedule until an operator changes its configuration.
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
            listing_source_id = %candidate.listing_source_id,
            listing_source_name = %candidate.listing_source_name,
            domain = %candidate.crawl_root.host(),
            "Failed to persist crawl needs-operator metadata"
        ),
    }
    log_spider_failure(
        candidate,
        error.cause(),
        error_kind,
        failure_count,
        FailureDisposition::NeedsOperator,
    );
    SpiderSlotStatus::StoppedForOperator
}

pub(crate) const CRAWL_FAILED_EVENT: &str = "crawler.spider.crawl_failed";
const ROOT_MOVED_EVENT: &str = "crawler.spider.root_moved";
const CRAWL_TRUNCATED_EVENT: &str = "crawler.spider.crawl_truncated";
const CRAWL_RATE_LIMITED_EVENT: &str = "crawler.spider.crawl_rate_limited";

/// What a failed run's failure came to.
#[derive(Clone, Copy)]
enum FailureDisposition {
    /// The ListingSource is crawled again at `next_crawl_at`, after `cooldown`.
    Retry {
        next_crawl_at: time::OffsetDateTime,
        cooldown: Duration,
        /// The shop's rate-limit answer that stopped the crawl, when it gave one.
        rate_limit: Option<RateLimitFacts>,
    },
    /// The ListingSource leaves the schedule until an operator changes its configuration.
    NeedsOperator,
}

/// A shop's rate-limit answer and the ListingSource's resulting streak.
#[derive(Clone, Copy)]
struct RateLimitFacts {
    signal: RateLimitSignal,
    streak: i32,
}

impl FailureDisposition {
    fn rate_limit(self) -> Option<RateLimitFacts> {
        match self {
            Self::Retry { rate_limit, .. } => rate_limit,
            Self::NeedsOperator => None,
        }
    }

    fn rate_limit_status(self) -> Option<u16> {
        self.rate_limit().map(|facts| facts.signal.status)
    }

    fn retry_after_seconds(self) -> Option<u64> {
        self.rate_limit()
            .and_then(|facts| facts.signal.retry_after)
            .map(|retry_after| retry_after.as_secs())
    }

    fn rate_limit_streak(self) -> Option<i32> {
        self.rate_limit().map(|facts| facts.streak)
    }

    fn next_crawl_at(self) -> Option<String> {
        match self {
            Self::Retry { next_crawl_at, .. } => Some(next_crawl_at.to_string()),
            Self::NeedsOperator => None,
        }
    }

    fn cooldown_seconds(self) -> Option<u64> {
        match self {
            Self::Retry { cooldown, .. } => Some(cooldown.as_secs()),
            Self::NeedsOperator => None,
        }
    }

    fn needs_operator(self) -> bool {
        matches!(self, Self::NeedsOperator)
    }
}

/// One `crawler.spider.crawl_failed` warning per failed run, with the evidence the error variant
/// carries and what the run's failure came to: a retry after a cooldown, or a wait for an operator.
fn log_spider_failure(
    candidate: &SpiderCandidate,
    error: &SpiderServiceError,
    error_kind: &str,
    failure_count: i32,
    disposition: FailureDisposition,
) {
    match error {
        SpiderServiceError::InsufficientInferenceSample {
            stage,
            sample_size,
            min_sample_size,
            diagnostics,
            ..
        } => warn!(
            event = CRAWL_FAILED_EVENT,
            listing_source_id = %candidate.listing_source_id,
            listing_source_name = %candidate.listing_source_name,
            domain = %candidate.crawl_root.host(),
            error = %error,
            error_kind,
            failure_count,
            stage,
            sample_size = *sample_size,
            min_sample_size = *min_sample_size,
            completion = %diagnostics.completion,
            truncated = !diagnostics.completion.is_full(),
            stopped_early = diagnostics.stopped_early.map(|reason| reason.as_str()),
            crawl_budget_ms = diagnostics.crawl_budget_ms,
            seeds = diagnostics.seeds,
            known_skipped = diagnostics.known_skipped,
            root_reroot = ?diagnostics.root_reroot,
            diagnostic_reason = diagnostics.diagnostic_reason,
            initial_anti_bot = ?diagnostics.initial_anti_bot,
            pages_received = diagnostics.pages_received,
            accepted = diagnostics.accepted,
            rejected_host = diagnostics.rejected_host,
            rejected_blacklist = diagnostics.rejected_blacklist,
            rejected_duplicate = diagnostics.rejected_duplicate,
            rejected_page_error = diagnostics.rejected_page_error,
            rejected_page_directive = diagnostics.rejected_page_directive,
            nofollow_pages = diagnostics.nofollow_pages,
            root_directives_forbid_crawl = diagnostics.root_directives_forbid_crawl,
            lagged = diagnostics.lagged,
            status_counts = ?diagnostics.status_counts,
            bot_challenge_pages = diagnostics.bot_challenge_pages,
            robots_policy = ?diagnostics.robots_policy,
            robots_crawl_delay_ms = diagnostics.robots_crawl_delay_ms,
            request_delay_ms = diagnostics.request_delay_ms,
            robots_rules_failing_closed = diagnostics.robots_rules_failing_closed,
            next_crawl_at = disposition.next_crawl_at(),
            cooldown_seconds = disposition.cooldown_seconds(),
            needs_operator = disposition.needs_operator(),
            rate_limit_status = disposition.rate_limit_status(),
            retry_after_seconds = disposition.retry_after_seconds(),
            rate_limit_streak = disposition.rate_limit_streak(),
            "Spider run failed"
        ),
        SpiderServiceError::CrawlFailed {
            total_links,
            diagnostics,
            ..
        } => warn!(
            event = CRAWL_FAILED_EVENT,
            listing_source_id = %candidate.listing_source_id,
            listing_source_name = %candidate.listing_source_name,
            domain = %candidate.crawl_root.host(),
            error = %error,
            error_kind,
            failure_count,
            total_crawled = *total_links,
            completion = %diagnostics.completion,
            truncated = !diagnostics.completion.is_full(),
            stopped_early = diagnostics.stopped_early.map(|reason| reason.as_str()),
            min_required_links = 2,
            user_agent = CRAWLER_USER_AGENT,
            root_preflight_status = diagnostics.root_preflight_status,
            http_status = diagnostics.http_status,
            final_url = diagnostics.final_url,
            redirect_url = diagnostics.redirect_url,
            root_reroot = ?diagnostics.root_reroot,
            diagnostic_reason = diagnostics.diagnostic_reason,
            initial_html_length = diagnostics.initial_html_length,
            initial_should_retry = diagnostics.initial_should_retry,
            initial_anti_bot = ?diagnostics.initial_anti_bot,
            bot_challenge_pages = diagnostics.bot_challenge_pages,
            hard_block = ?diagnostics.hard_block,
            pages_received = diagnostics.pages_received,
            accepted = diagnostics.accepted,
            rejected_host = diagnostics.rejected_host,
            rejected_blacklist = diagnostics.rejected_blacklist,
            rejected_duplicate = diagnostics.rejected_duplicate,
            rejected_page_error = diagnostics.rejected_page_error,
            rejected_page_directive = diagnostics.rejected_page_directive,
            nofollow_pages = diagnostics.nofollow_pages,
            root_directives_forbid_crawl = diagnostics.root_directives_forbid_crawl,
            robots_policy = ?diagnostics.robots_policy,
            robots_crawl_delay_ms = diagnostics.robots_crawl_delay_ms,
            request_delay_ms = diagnostics.request_delay_ms,
            rate_limit = ?diagnostics.rate_limit,
            robots_rules_failing_closed = diagnostics.robots_rules_failing_closed,
            lagged = diagnostics.lagged,
            next_crawl_at = disposition.next_crawl_at(),
            cooldown_seconds = disposition.cooldown_seconds(),
            needs_operator = disposition.needs_operator(),
            rate_limit_status = disposition.rate_limit_status(),
            retry_after_seconds = disposition.retry_after_seconds(),
            rate_limit_streak = disposition.rate_limit_streak(),
            "Spider run failed"
        ),
        SpiderServiceError::Discovery(
            preflight @ SpiderDiscoveryError::Preflight { reason, source, .. },
        ) => {
            warn!(
                event = CRAWL_FAILED_EVENT,
                listing_source_id = %candidate.listing_source_id,
                listing_source_name = %candidate.listing_source_name,
                domain = %candidate.crawl_root.host(),
                error = %error,
                error_kind,
                failure_count,
                reason,
                robots_url = preflight.robots_unreachable().map(RobotsUnreachable::url),
                robots_status = preflight.robots_unreachable().and_then(RobotsUnreachable::status),
                robots_transport_error = preflight
                    .robots_unreachable()
                    .and_then(RobotsUnreachable::transport_error),
                redirect_url = preflight.redirect_target(),
                root_reroot = ?preflight.root_reroot(),
                source = ?source,
                next_crawl_at = disposition.next_crawl_at(),
                cooldown_seconds = disposition.cooldown_seconds(),
                needs_operator = disposition.needs_operator(),
                rate_limit_status = disposition.rate_limit_status(),
                retry_after_seconds = disposition.retry_after_seconds(),
                rate_limit_streak = disposition.rate_limit_streak(),
                "Spider run failed"
            )
        }
        SpiderServiceError::NoProductUrls {
            total_links,
            product_pattern,
            completion,
            ..
        } => warn!(
            event = CRAWL_FAILED_EVENT,
            listing_source_id = %candidate.listing_source_id,
            listing_source_name = %candidate.listing_source_name,
            domain = %candidate.crawl_root.host(),
            error = %error,
            error_kind,
            failure_count,
            total_crawled = *total_links,
            product_urls_count = 0,
            product_pattern,
            completion = %completion,
            truncated = !completion.is_full(),
            next_crawl_at = disposition.next_crawl_at(),
            cooldown_seconds = disposition.cooldown_seconds(),
            needs_operator = disposition.needs_operator(),
            rate_limit_status = disposition.rate_limit_status(),
            retry_after_seconds = disposition.retry_after_seconds(),
            rate_limit_streak = disposition.rate_limit_streak(),
            "Spider run failed"
        ),
        SpiderServiceError::NoUrlPattern {
            total_links,
            completion,
            ..
        } => warn!(
            event = CRAWL_FAILED_EVENT,
            listing_source_id = %candidate.listing_source_id,
            listing_source_name = %candidate.listing_source_name,
            domain = %candidate.crawl_root.host(),
            error = %error,
            error_kind,
            failure_count,
            total_crawled = *total_links,
            product_urls_count = 0,
            completion = %completion,
            truncated = !completion.is_full(),
            next_crawl_at = disposition.next_crawl_at(),
            cooldown_seconds = disposition.cooldown_seconds(),
            needs_operator = disposition.needs_operator(),
            rate_limit_status = disposition.rate_limit_status(),
            retry_after_seconds = disposition.retry_after_seconds(),
            rate_limit_streak = disposition.rate_limit_streak(),
            "Spider run failed"
        ),
        SpiderServiceError::BotChallengedCrawl {
            bot_challenge_pages,
            pages_received,
            failure,
        } => warn!(
            event = CRAWL_FAILED_EVENT,
            listing_source_id = %candidate.listing_source_id,
            listing_source_name = %candidate.listing_source_name,
            domain = %candidate.crawl_root.host(),
            error = %error,
            error_kind,
            failure_count,
            bot_challenge_pages = *bot_challenge_pages,
            pages_received = *pages_received,
            replaced_error = %failure,
            replaced_error_kind = error_kind_for_spider_error(failure),
            next_crawl_at = disposition.next_crawl_at(),
            cooldown_seconds = disposition.cooldown_seconds(),
            needs_operator = disposition.needs_operator(),
            rate_limit_status = disposition.rate_limit_status(),
            retry_after_seconds = disposition.retry_after_seconds(),
            rate_limit_streak = disposition.rate_limit_streak(),
            "Spider run failed"
        ),
        _ => warn!(
            event = CRAWL_FAILED_EVENT,
            listing_source_id = %candidate.listing_source_id,
            listing_source_name = %candidate.listing_source_name,
            domain = %candidate.crawl_root.host(),
            error = %error,
            error_kind,
            failure_count,
            next_crawl_at = disposition.next_crawl_at(),
            cooldown_seconds = disposition.cooldown_seconds(),
            needs_operator = disposition.needs_operator(),
            rate_limit_status = disposition.rate_limit_status(),
            retry_after_seconds = disposition.retry_after_seconds(),
            rate_limit_streak = disposition.rate_limit_streak(),
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
        Ok(result) if result.completion == CrawlCompletion::Stopped => {
            record_stopped_run(spider_candidates, candidate).await
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
        Err(error) if error.crawl_was_stopped() => {
            record_stopped_run(spider_candidates, candidate).await
        }
        Err(error) => record_failed_run(spider_candidates, candidate, error).await,
    }
}

fn spawn_spider_candidate(
    join_set: &mut JoinSet<SpiderSlotOutcome>,
    candidate: SpiderCandidate,
    spider_candidates: Arc<dyn SpiderCandidateService>,
    spider_service: Arc<dyn SpiderService>,
    lock_manager: Arc<crate::spider::local_lock::LocalLockManager>,
    threshold: usize,
    budget: CrawlBudgetScale,
) -> tokio::task::Id {
    let crawl_root_url = candidate.crawl_root.url().to_string();
    let listing_source_id = candidate.listing_source_id;
    let pace = crawl_pace(&candidate);
    let span = tracing::info_span!(
        "spider_candidate",
        listing_source_id = %candidate.listing_source_id,
        crawl_root_url = %crawl_root_url,
        kind = %candidate.kind,
        budget_factor = budget.factor()
    );

    let task = join_set.spawn(
        async move {
            let Some(_lock) =
                WorkLock::try_acquire(&lock_manager, WorkLockScope::Spider, listing_source_id)
            else {
                debug!(
                    listing_source_id = %listing_source_id,
                    "Deferring ListingSource - lock held by another spider worker"
                );
                return SpiderSlotOutcome {
                    listing_source_id,
                    status: SpiderSlotStatus::Deferred,
                };
            };

            // The whole slot runs scoped, so every record of the run and of its outcome's
            // persistence names the source, which the span list no longer does.
            candidate
                .source_log_context()
                .scope(async {
                    let run = spider_service
                        .run(
                            &listing_source_id,
                            &crawl_root_url,
                            threshold,
                            pace,
                            budget,
                            candidate.kind,
                        )
                        .await;
                    let status =
                        record_run_outcome(spider_candidates.as_ref(), &candidate, &run).await;
                    SpiderSlotOutcome {
                        listing_source_id,
                        status,
                    }
                })
                .await
        }
        .instrument(span),
    );
    task.id()
}
/// What one spider pass publishes: its failed runs, aggregated in process. No per-source figure
/// leaves the pass as a metric; the run's own outcome events name the source.
fn spider_pass_metrics(crawl_runs_failed: u64) -> MetricRecord {
    MetricRecord::default().with_value(CrawlerMetric::CrawlRunsFailed, crawl_runs_failed)
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
        let mut excluded_listing_source_ids: HashSet<ListingSourceId> = HashSet::new();
        let extended_slots = extended_crawl_slots(spider_concurrency);
        // Keyed by task, so a slot whose task panicked releases its extended budget too.
        let mut extended_in_flight: HashSet<tokio::task::Id> = HashSet::new();
        let mut join_set: JoinSet<SpiderSlotOutcome> = JoinSet::new();
        let mut total = 0usize;
        let mut succeeded = 0usize;
        let mut truncated = 0usize;
        let mut stopped = 0usize;
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
                let excluded: Vec<ListingSourceId> =
                    excluded_listing_source_ids.iter().copied().collect();
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
                        emit_pass_record(
                            spider_pass_metrics(0),
                            CrawlerMetric::SpiderPassDuration,
                            pass_start,
                        );
                        return;
                    }
                    break;
                }

                if !started {
                    debug!(
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
                    if !excluded_listing_source_ids.insert(candidate.listing_source_id) {
                        continue;
                    }
                    scheduled_any = true;
                    total += 1;
                    let budget =
                        crawl_budget_scale(&candidate, extended_in_flight.len() < extended_slots);
                    let task = spawn_spider_candidate(
                        &mut join_set,
                        candidate,
                        Arc::clone(&self.spider_candidates),
                        Arc::clone(&self.spider_service),
                        Arc::clone(&self.lock_manager),
                        self.config.spider_classify_threshold,
                        budget,
                    );
                    if budget.is_extended() {
                        extended_in_flight.insert(task);
                    }
                }

                if !scheduled_any {
                    break;
                }
            }

            if join_set.is_empty() {
                break;
            }

            match join_set.join_next_with_id().await {
                Some(Ok((task, outcome))) => {
                    extended_in_flight.remove(&task);
                    excluded_listing_source_ids.insert(outcome.listing_source_id);
                    match outcome.status {
                        SpiderSlotStatus::Succeeded => succeeded += 1,
                        SpiderSlotStatus::Truncated => truncated += 1,
                        SpiderSlotStatus::Stopped => stopped += 1,
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
                    extended_in_flight.remove(&e.id());
                    error!(error = %e, "Spider worker task failed to join");
                    failed += 1;
                }
                None => break,
            }
        }

        // A run that stopped its ListingSource for an operator is a failed run too.
        let duration_ms = emit_pass_record(
            spider_pass_metrics((failed + stopped_for_operator) as u64),
            CrawlerMetric::SpiderPassDuration,
            pass_start,
        );
        info!(
            event = "crawler.spider.pass_completed",
            total,
            succeeded,
            truncated,
            stopped,
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
    use crate::CrawlRoot;
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

    #[tokio::test]
    async fn should_crawl_a_candidate_from_its_crawl_root_url_with_its_path() {
        let listing_source_id = ListingSourceId::new();
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move {
                    Ok(vec![SpiderCandidate {
                        crawl_root: crawl_root("https://httpbin.org/en/antiques/"),
                        ..spider_candidate(listing_source_id)
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
            .withf(|_, crawl_root_url, _, _, _, _| {
                crawl_root_url == "https://httpbin.org/en/antiques/"
            })
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

    fn crawl_root(presentation_url: &str) -> CrawlRoot {
        CrawlRoot::from_presentation_url(presentation_url)
            .unwrap_or_else(|error| panic!("{presentation_url} is a crawl root: {error}"))
    }

    fn spider_candidate(listing_source_id: ListingSourceId) -> SpiderCandidate {
        SpiderCandidate {
            listing_source_id,
            listing_source_name: "Example Shop".to_owned(),
            crawl_root: crawl_root("https://example.com/"),
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
    fn crawl_budget_scale_should_extend_only_a_full_continuation_with_a_free_extended_slot() {
        let continuation = || {
            let mut candidate = spider_candidate(ListingSourceId::new());
            candidate.crawl_truncation_count = 2;
            candidate
        };
        let extendable = continuation();
        assert_eq!(
            crawl_budget_scale(&extendable, true),
            CrawlBudgetScale::after_truncated_crawls(2)
        );
        assert_eq!(
            crawl_budget_scale(&extendable, false),
            CrawlBudgetScale::STANDARD
        );

        let first_crawl = spider_candidate(ListingSourceId::new());
        assert_eq!(
            crawl_budget_scale(&first_crawl, true),
            CrawlBudgetScale::STANDARD
        );

        let mut incremental = continuation();
        incremental.kind = CrawlKind::Incremental;
        assert_eq!(
            crawl_budget_scale(&incremental, true),
            CrawlBudgetScale::STANDARD
        );

        let mut rate_limited = continuation();
        rate_limited.crawl_rate_limit_streak = 1;
        assert_eq!(
            crawl_budget_scale(&rate_limited, true),
            CrawlBudgetScale::STANDARD
        );
    }

    #[test]
    fn extended_crawl_slots_should_leave_at_least_two_thirds_of_the_slots_standard() {
        assert_eq!(extended_crawl_slots(1), 0);
        assert_eq!(extended_crawl_slots(2), 0);
        assert_eq!(extended_crawl_slots(3), 1);
        assert_eq!(extended_crawl_slots(8), 2);
    }

    #[tokio::test]
    async fn should_extend_at_most_the_fair_share_of_concurrent_continuations() {
        use std::sync::Mutex;

        let sources = [
            ListingSourceId::new(),
            ListingSourceId::new(),
            ListingSourceId::new(),
        ];
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let first_call = excluded.is_empty();
                Box::pin(async move {
                    Ok(match first_call {
                        true => sources
                            .iter()
                            .map(|&listing_source_id| {
                                let mut candidate = spider_candidate(listing_source_id);
                                candidate.crawl_truncation_count = 2;
                                candidate
                            })
                            .collect(),
                        false => vec![],
                    })
                })
            });
        spider_candidates
            .expect_mark_crawl_completed()
            .times(3)
            .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        let budgets = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&budgets);
        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(3)
            .returning(move |_, _, _, _, budget, _| {
                recorded.lock().unwrap().push(budget.factor());
                Box::pin(async { Ok(run_result(CrawlCompletion::Completed)) })
            });

        spider_job(spider_candidates, spider_service, 3)
            .run_spider_once()
            .await;

        let mut factors = budgets.lock().unwrap().clone();
        factors.sort_unstable();
        assert_eq!(factors, vec![1, 1, 3]);
    }

    #[tokio::test]
    async fn should_release_the_extended_slot_of_a_continuation_whose_task_panicked() {
        use std::sync::Mutex;
        use tokio::sync::Semaphore;

        let panicking = ListingSourceId::new();
        let busy = [ListingSourceId::new(), ListingSourceId::new()];
        let next = ListingSourceId::new();
        let continuation = |listing_source_id| {
            let mut candidate = spider_candidate(listing_source_id);
            candidate.crawl_truncation_count = 2;
            candidate
        };
        let mut spider_candidates = MockSpiderCandidateService::new();
        let calls = Arc::new(Mutex::new(0usize));
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                let call = {
                    let mut calls = calls.lock().unwrap();
                    *calls += 1;
                    *calls
                };
                Box::pin(async move {
                    Ok(match call {
                        // The continuation takes the only extended slot; two standard crawls keep
                        // the other slots busy until the next continuation has started.
                        1 => vec![
                            continuation(panicking),
                            spider_candidate(busy[0]),
                            spider_candidate(busy[1]),
                        ],
                        2 => vec![continuation(next)],
                        _ => vec![],
                    })
                })
            });
        spider_candidates
            .expect_mark_crawl_completed()
            .times(3)
            .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        let release_busy = Arc::new(Semaphore::new(0));
        let next_budget = Arc::new(Mutex::new(None));
        let recorded = Arc::clone(&next_budget);
        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(4)
            .returning(move |source, _, _, _, budget, _| {
                let source = *source;
                let release_busy = Arc::clone(&release_busy);
                if source == next {
                    *recorded.lock().unwrap() = Some(budget.factor());
                    release_busy.add_permits(busy.len());
                }
                Box::pin(async move {
                    if source == panicking {
                        panic!("spider worker panicked");
                    }
                    if busy.contains(&source) {
                        release_busy.acquire().await.unwrap().forget();
                    }
                    Ok(run_result(CrawlCompletion::Completed))
                })
            });

        spider_job(spider_candidates, spider_service, 3)
            .run_spider_once()
            .await;

        assert_eq!(*next_budget.lock().unwrap(), Some(3));
    }

    #[test]
    fn next_failure_count_increments_for_same_error_kind() {
        let mut candidate = spider_candidate(ListingSourceId::new());
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
            let mut candidate = spider_candidate(ListingSourceId::new());
            candidate.crawl_failure_count = 2;
            candidate.last_crawl_error_kind = Some(last.to_string());

            assert_eq!(next_failure_count(&candidate, next), 3, "{last} -> {next}");
        }
    }

    #[test]
    fn next_failure_count_resets_between_a_no_product_url_kind_and_another_kind() {
        for (last, next) in [("NoUrlPattern", "TinyCrawl"), ("TinyCrawl", "NoUrlPattern")] {
            let mut candidate = spider_candidate(ListingSourceId::new());
            candidate.crawl_failure_count = 2;
            candidate.last_crawl_error_kind = Some(last.to_string());

            assert_eq!(next_failure_count(&candidate, next), 1, "{last} -> {next}");
        }
    }

    #[test]
    fn continues_failure_streak_should_share_one_streak_between_bot_protection_and_no_product_kinds()
     {
        for no_product in NO_PRODUCT_URL_ERROR_KINDS {
            assert!(continues_failure_streak(Some(no_product), "BotProtection"));
            assert!(continues_failure_streak(Some("BotProtection"), no_product));
        }
        assert!(continues_failure_streak(
            Some("BotProtection"),
            "BotProtection"
        ));
        for other in ["TinyCrawl", "CloudflareChallenge", "RateLimited"] {
            assert!(!continues_failure_streak(Some(other), "BotProtection"));
            assert!(!continues_failure_streak(Some("BotProtection"), other));
        }
        assert!(!continues_failure_streak(None, "BotProtection"));
    }

    #[test]
    fn next_failure_count_should_count_an_alternating_run_of_bot_protection_and_no_product_kinds() {
        let mut candidate = spider_candidate(ListingSourceId::new());
        let mut count = 0;
        for kind in [
            "BotProtection",
            "NoProductUrls",
            "BotProtection",
            "NoUrlPattern",
            "BotProtection",
        ] {
            count = next_failure_count(&candidate, kind);
            candidate.crawl_failure_count = count;
            candidate.last_crawl_error_kind = Some(kind.to_string());
        }
        assert_eq!(count, NEEDS_OPERATOR_FAILURE_COUNT);
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
        let mut candidate = spider_candidate(ListingSourceId::new());
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

    mod crawl_truncated_event {
        use super::*;
        use crate::test_support::logged_events::LoggedEvents;

        #[tokio::test]
        async fn should_log_the_frontier_left_and_the_database_wait_of_a_truncated_run() {
            let (events, _guard) = LoggedEvents::capture();
            let mut spider_candidates = MockSpiderCandidateService::new();
            spider_candidates
                .expect_mark_crawl_truncated()
                .returning(|_, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
            let result = SpiderRunResult {
                frontier_left: Some(1_500),
                db_wait: Duration::from_millis(42_000),
                ran_out_of_time: true,
                ..run_result(CrawlCompletion::HardTimeoutTruncated)
            };

            record_truncated_run(
                &spider_candidates,
                &spider_candidate(ListingSourceId::new()),
                &result,
            )
            .await;

            let truncated = events.named(CRAWL_TRUNCATED_EVENT);
            assert_eq!(truncated.len(), 1);
            assert_eq!(truncated[0].field("frontier_left"), Some("1500"));
            assert_eq!(truncated[0].field("db_wait_ms"), Some("42000"));
            assert_eq!(truncated[0].field("ran_out_of_time"), Some("true"));
            assert_eq!(
                truncated[0].field("cooldown_seconds"),
                Some(
                    TRUNCATED_CRAWL_CONTINUATION_COOLDOWN
                        .as_secs()
                        .to_string()
                        .as_str()
                )
            );
        }
    }

    mod crawl_failed_event {
        use super::*;
        use crate::test_support::logged_events::LoggedEvents;

        fn failure_log(
            error: &SpiderServiceError,
            disposition: FailureDisposition,
        ) -> crate::test_support::logged_events::LoggedEvent {
            let (events, _guard) = LoggedEvents::capture();
            let candidate = spider_candidate(ListingSourceId::new());

            log_spider_failure(&candidate, error, "ConnectError", 5, disposition);

            let mut failures = events.named(CRAWL_FAILED_EVENT);
            assert_eq!(failures.len(), 1);
            failures.remove(0)
        }

        #[test]
        fn should_carry_the_rate_limit_answer_when_a_run_fails_after_one() {
            let failure = failure_log(
                &crawl_failed(CrawlFailureKind::ConnectError),
                FailureDisposition::Retry {
                    next_crawl_at: time::OffsetDateTime::UNIX_EPOCH,
                    cooldown: Duration::from_secs(90),
                    rate_limit: Some(RateLimitFacts {
                        signal: RateLimitSignal {
                            status: 429,
                            retry_after: Some(Duration::from_secs(30)),
                        },
                        streak: 3,
                    }),
                },
            );

            assert_eq!(failure.field("rate_limit_status"), Some("429"));
            assert_eq!(failure.field("retry_after_seconds"), Some("30"));
            assert_eq!(failure.field("rate_limit_streak"), Some("3"));
        }

        #[test]
        fn should_name_the_source_and_the_retry_when_a_run_fails_with_a_cooldown() {
            let failure = failure_log(
                &crawl_failed(CrawlFailureKind::ConnectError),
                FailureDisposition::Retry {
                    next_crawl_at: time::OffsetDateTime::UNIX_EPOCH,
                    cooldown: Duration::from_secs(90),
                    rate_limit: None,
                },
            );

            assert_eq!(failure.level, tracing::Level::WARN);
            assert_eq!(failure.field("listing_source_name"), Some("Example Shop"));
            assert!(failure.field("domain").is_some());
            assert_eq!(failure.field("needs_operator"), Some("false"));
            assert_eq!(failure.field("cooldown_seconds"), Some("90"));
            assert_eq!(
                failure.field("next_crawl_at"),
                Some("1970-01-01 0:00:00.0 +00:00:00"),
                "a plain time, not `Some(..)`"
            );
            assert_eq!(failure.field("diagnostic_reason"), Some("test_diagnostic"));
        }

        #[test]
        fn should_keep_the_failure_evidence_when_a_run_needs_an_operator() {
            let failure = failure_log(
                &crawl_failed(CrawlFailureKind::ConnectError),
                FailureDisposition::NeedsOperator,
            );

            assert_eq!(failure.field("needs_operator"), Some("true"));
            assert_eq!(failure.field("cooldown_seconds"), None);
            assert_eq!(failure.field("diagnostic_reason"), Some("test_diagnostic"));
        }

        #[test]
        fn should_name_the_failure_a_bot_challenge_replaced_when_a_crawl_met_bot_protection() {
            let failure = failure_log(
                &SpiderServiceError::BotChallengedCrawl {
                    bot_challenge_pages: 4,
                    pages_received: 5,
                    failure: Box::new(crawl_failed(CrawlFailureKind::ConnectError)),
                },
                FailureDisposition::NeedsOperator,
            );

            assert_eq!(failure.field("bot_challenge_pages"), Some("4"));
            assert_eq!(failure.field("pages_received"), Some("5"));
            assert!(failure.field("replaced_error").is_some());
            assert!(failure.field("replaced_error_kind").is_some());
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

    /// A durable block waits for an operator on its second failure, so the one retry it gets is
    /// the short one.
    #[test]
    fn cooldown_should_retry_an_explicit_block_once_after_the_short_cooldown() {
        for kind in [
            CrawlFailureKind::AccessDenied,
            CrawlFailureKind::JavascriptRequired,
        ] {
            let error = crawl_failed(kind);
            assert_eq!(cooldown_for_spider_failure(&error, 1), CRAWL_RETRY_COOLDOWN);
            assert!(needs_operator(
                &error,
                DURABLE_BLOCK_NEEDS_OPERATOR_FAILURE_COUNT
            ));
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
    /// stopped its crawl, is a bot-protection failure: its own kind, the transient cooldown
    /// until it escalates with the shared streak, and no product count for the pattern alarm.
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
            assert!(!needs_operator(&error, NEEDS_OPERATOR_FAILURE_COUNT - 1));
            assert!(needs_operator(&error, NEEDS_OPERATOR_FAILURE_COUNT));
        }
    }

    /// Every classified failure kind, from the spider or from the root preflight, reaches the
    /// operator: a refusing shop on its second failure in a row, every other kind on its fifth.
    #[test]
    fn needs_operator_should_stop_every_classified_failure_kind_at_its_escalation_count() {
        for kind in ALL_KINDS {
            let escalation_count = match kind {
                CrawlFailureKind::AccessDenied | CrawlFailureKind::JavascriptRequired => 2,
                _ => 5,
            };
            for error in [crawl_failed(kind), preflight_failed(kind)] {
                assert!(
                    !needs_operator(&error, escalation_count - 1),
                    "{kind} retries below {escalation_count}"
                );
                assert!(
                    needs_operator(&error, escalation_count),
                    "{kind} waits for an operator at {escalation_count}"
                );
            }
        }
    }

    #[test]
    fn needs_operator_should_keep_retrying_unclassified_failures() {
        for error in unclassified_errors() {
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
            frontier_left: None,
            db_wait: Duration::ZERO,
            ran_out_of_time: false,
        })
    }

    #[tokio::test]
    async fn should_record_an_incremental_run_without_touching_full_crawl_or_truncation_state() {
        let listing_source_id = ListingSourceId::new();
        let candidate = SpiderCandidate {
            kind: CrawlKind::Incremental,
            ..spider_candidate(listing_source_id)
        };
        for (completion, truncated) in [
            (CrawlCompletion::Completed, false),
            (CrawlCompletion::HardTimeoutTruncated, true),
        ] {
            let mut spider_candidates = MockSpiderCandidateService::new();
            spider_candidates
                .expect_mark_incremental_crawl_finished()
                .withf(move |root, recorded| {
                    root.listing_source_id == listing_source_id && *recorded == completion
                })
                .times(1)
                .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
            spider_candidates.expect_mark_crawl_completed().times(0);
            spider_candidates.expect_mark_crawl_truncated().times(0);
            let run = incremental_result(completion);

            let slot = record_run_outcome(&spider_candidates, &candidate, &run).await;

            assert_eq!(
                matches!(slot, SpiderSlotStatus::Truncated),
                truncated,
                "an incremental run ends as truncated only when its completion is not full"
            );
        }
    }

    #[tokio::test]
    async fn should_hold_back_an_incremental_run_a_shop_rate_limited_like_any_crawl() {
        let listing_source_id = ListingSourceId::new();
        let candidate = SpiderCandidate {
            kind: CrawlKind::Incremental,
            crawl_rate_limit_streak: 1,
            ..spider_candidate(listing_source_id)
        };
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_mark_incremental_crawl_finished()
            .times(0);
        spider_candidates
            .expect_mark_crawl_rate_limited()
            .withf(move |root, write| {
                root.listing_source_id == listing_source_id
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
        let expected_listing_source_id = ListingSourceId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(expected_listing_source_id)]) })
            });
        spider_candidates
            .expect_mark_crawl_completed()
            .withf(move |root, completion| {
                root.listing_source_id == expected_listing_source_id
                    && *completion == CrawlCompletion::Completed
            })
            .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .withf(move |source, _, _, _, _, _| *source == expected_listing_source_id)
            .returning(|_, _, _, _, _, _| {
                Box::pin(async {
                    Ok(SpiderRunResult {
                        total_links: 10,
                        product_urls_count: 5,
                        product_pattern: None,
                        completion: CrawlCompletion::Completed,
                        pending_url_pattern_review: None,
                        known_product_urls_skipped: 0,
                        frontier_left: None,
                        db_wait: Duration::ZERO,
                        ran_out_of_time: false,
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
        let slow_listing_source_id = ListingSourceId::new();
        let fast_listing_source_id = ListingSourceId::new();
        let refill_listing_source_id = ListingSourceId::new();

        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let excluded = excluded.to_vec();
                Box::pin(async move {
                    if excluded.is_empty() {
                        Ok(vec![
                            spider_candidate(slow_listing_source_id),
                            spider_candidate(fast_listing_source_id),
                        ])
                    } else if excluded.contains(&slow_listing_source_id)
                        && excluded.contains(&fast_listing_source_id)
                        && !excluded.contains(&refill_listing_source_id)
                    {
                        Ok(vec![spider_candidate(refill_listing_source_id)])
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
            .returning(move |source, _, _, _, _, _| {
                let listing_source_id = *source;
                let release_slow = Arc::clone(&release_slow_for_mock);
                let slow_running = Arc::clone(&slow_running_for_mock);
                let refill_started = Arc::clone(&refill_started_for_mock);
                Box::pin(async move {
                    if listing_source_id == slow_listing_source_id {
                        slow_running.store(true, Ordering::SeqCst);
                        release_slow.notified().await;
                        slow_running.store(false, Ordering::SeqCst);
                    } else if listing_source_id == fast_listing_source_id {
                        while !slow_running.load(Ordering::SeqCst) {
                            tokio::time::sleep(Duration::from_millis(1)).await;
                        }
                    } else if listing_source_id == refill_listing_source_id {
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
                        frontier_left: None,
                        db_wait: Duration::ZERO,
                        ran_out_of_time: false,
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
                Box::pin(async { Ok(vec![spider_candidate(ListingSourceId::new())]) })
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
                        frontier_left: None,
                        db_wait: Duration::ZERO,
                        ran_out_of_time: false,
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
        let expected_listing_source_id = ListingSourceId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(expected_listing_source_id)]) })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |root, error_kind, failure_count, _, next_crawl_at| {
                root.listing_source_id == expected_listing_source_id
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
        let expected_listing_source_id = ListingSourceId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(expected_listing_source_id)]) })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |root, error_kind, failure_count, _, next_crawl_at| {
                root.listing_source_id == expected_listing_source_id
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
        let expected_listing_source_id = ListingSourceId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move {
                    let mut candidate = spider_candidate(expected_listing_source_id);
                    candidate.crawl_failure_count = 2;
                    candidate.last_crawl_error_kind = Some("spider_run_error".to_string());
                    Ok(vec![candidate])
                })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |root, error_kind, failure_count, _, next_crawl_at| {
                root.listing_source_id == expected_listing_source_id
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
        let expected_listing_source_id = ListingSourceId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move {
                    let mut candidate = spider_candidate(expected_listing_source_id);
                    candidate.crawl_failure_count = 2;
                    candidate.last_crawl_error_kind =
                        Some("InsufficientInferenceSample".to_string());
                    Ok(vec![candidate])
                })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |root, error_kind, failure_count, _, next_crawl_at| {
                root.listing_source_id == expected_listing_source_id
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
        let expected_listing_source_id = ListingSourceId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(expected_listing_source_id)]) })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |root, error_kind, failure_count, _, next_crawl_at| {
                root.listing_source_id == expected_listing_source_id
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
        let expected_listing_source_id = ListingSourceId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move {
                    let mut candidate = spider_candidate(expected_listing_source_id);
                    candidate.crawl_failure_count = 2;
                    candidate.last_crawl_error_kind = Some("ConnectError".to_string());
                    Ok(vec![candidate])
                })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |root, error_kind, failure_count, _, next_crawl_at| {
                root.listing_source_id == expected_listing_source_id
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
        let expected_listing_source_id = ListingSourceId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move {
                    let mut candidate = spider_candidate(expected_listing_source_id);
                    candidate.crawl_failure_count = 2;
                    candidate.last_crawl_error_kind = Some("RateLimited".to_string());
                    Ok(vec![candidate])
                })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |root, error_kind, failure_count, _, next_crawl_at| {
                root.listing_source_id == expected_listing_source_id
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

    /// A truncated run that `ran_out_of_time` and left `frontier_left`.
    fn truncated_run(ran_out_of_time: bool, frontier_left: Option<usize>) -> SpiderRunResult {
        SpiderRunResult {
            ran_out_of_time,
            frontier_left,
            ..run_result(CrawlCompletion::HardTimeoutTruncated)
        }
    }

    #[test]
    fn truncated_cooldown_should_continue_a_crawl_that_ran_out_of_time_within_hours() {
        assert_eq!(
            truncated_crawl_cooldown(&truncated_run(true, Some(1))),
            Duration::from_secs(6 * 60 * 60)
        );
        assert_eq!(
            truncated_crawl_cooldown(&truncated_run(true, Some(20_000))),
            truncated_crawl_cooldown(&truncated_run(true, Some(1)))
        );
    }

    #[test]
    fn truncated_cooldown_should_wait_a_day_unless_the_crawl_ran_out_of_time_with_a_frontier() {
        let day = Duration::from_secs(24 * 60 * 60);
        // Ended early, leaving only pages it could not fetch, such as bot challenges.
        assert_eq!(
            truncated_crawl_cooldown(&truncated_run(false, Some(3))),
            day
        );
        assert_eq!(truncated_crawl_cooldown(&truncated_run(true, Some(0))), day);
        assert_eq!(truncated_crawl_cooldown(&truncated_run(true, None)), day);
        assert!(day < SPIDER_RECRAWL_INTERVAL);
    }

    #[test]
    fn truncation_count_should_count_only_consecutive_crawls_that_ran_out_of_time() {
        let mut candidate = spider_candidate(ListingSourceId::new());
        candidate.crawl_truncation_count = 2;

        assert_eq!(
            next_truncation_count(&candidate, &truncated_run(true, Some(5))),
            3
        );
        assert_eq!(
            next_truncation_count(&candidate, &truncated_run(false, Some(5))),
            0
        );
        assert_eq!(
            next_truncation_count(&candidate, &truncated_run(true, None)),
            0
        );
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
        let mut candidate = spider_candidate(ListingSourceId::new());
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
        let expected_listing_source_id = ListingSourceId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let first_call = excluded.is_empty();
                Box::pin(async move {
                    if !first_call {
                        return Ok(vec![]);
                    }
                    let mut candidate = spider_candidate(expected_listing_source_id);
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
                root.listing_source_id == expected_listing_source_id
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
            .withf(|_, _, _, pace, _, _| *pace == CrawlPace::after_rate_limited_crawls(1))
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
        let expected_listing_source_id = ListingSourceId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let first_call = excluded.is_empty();
                Box::pin(async move {
                    Ok(if first_call {
                        vec![spider_candidate(expected_listing_source_id)]
                    } else {
                        vec![]
                    })
                })
            });
        spider_candidates
            .expect_mark_crawl_rate_limited()
            .times(1)
            .withf(move |root, write| {
                root.listing_source_id == expected_listing_source_id
                    && write.streak == 1
                    && next_crawl_at_is_about(write.not_before, HOUR * 5)
            })
            .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(1)
            .withf(|_, _, _, pace, _, _| *pace == CrawlPace::default())
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
        let mut spider_candidates = failing_candidate_service(ListingSourceId::new(), 0, "None");
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
        let mut spider_candidates = failing_candidate_service(ListingSourceId::new(), 0, "None");
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
        let expected_listing_source_id = ListingSourceId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let first_call = excluded.is_empty();
                Box::pin(async move {
                    Ok(if first_call {
                        vec![spider_candidate(expected_listing_source_id)]
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
                    root.listing_source_id == expected_listing_source_id
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

        assert_eq!(completion.as_str(), "RATE_LIMITED");
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
            frontier_left: None,
            db_wait: Duration::ZERO,
            ran_out_of_time: false,
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
        let expected_listing_source_id = ListingSourceId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let first_call = excluded.is_empty();
                Box::pin(async move {
                    if !first_call {
                        return Ok(vec![]);
                    }
                    let mut candidate = spider_candidate(expected_listing_source_id);
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
                root.listing_source_id == expected_listing_source_id
                    && *truncation_count == 2
                    && next_crawl_at_is_about(*next_crawl_at, TRUNCATED_CRAWL_CONTINUATION_COOLDOWN)
            })
            .returning(|_, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        spider_candidates.expect_mark_crawl_completed().times(0);
        spider_candidates.expect_mark_crawl_failure().times(0);

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(1)
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(truncated_run(true, Some(1_500))) }));

        spider_job(spider_candidates, spider_service, 1)
            .run_spider_once()
            .await;
    }

    #[tokio::test]
    async fn should_continue_a_truncated_crawl_within_hours_when_it_left_a_frontier() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_listing_source_id = ListingSourceId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let first_call = excluded.is_empty();
                Box::pin(async move {
                    Ok(if first_call {
                        vec![spider_candidate(expected_listing_source_id)]
                    } else {
                        vec![]
                    })
                })
            });
        spider_candidates
            .expect_mark_crawl_truncated()
            .times(1)
            .withf(move |root, truncation_count, next_crawl_at| {
                root.listing_source_id == expected_listing_source_id
                    && *truncation_count == 1
                    && next_crawl_at_is_about(*next_crawl_at, TRUNCATED_CRAWL_CONTINUATION_COOLDOWN)
            })
            .returning(|_, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        spider_candidates.expect_mark_crawl_completed().times(0);
        spider_candidates.expect_mark_crawl_failure().times(0);

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(1)
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(truncated_run(true, Some(1_500))) }));

        spider_job(spider_candidates, spider_service, 1)
            .run_spider_once()
            .await;
    }

    /// A crawl that ended early and left only pages it could not fetch, such as bot challenges,
    /// is cut short without being too large for one crawl: it waits a day and ends the run of
    /// truncations that extends the next crawl's budget.
    #[tokio::test]
    async fn should_wait_a_day_without_extension_after_a_crawl_that_ended_early_with_a_frontier() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_listing_source_id = ListingSourceId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let first_call = excluded.is_empty();
                Box::pin(async move {
                    let mut candidate = spider_candidate(expected_listing_source_id);
                    candidate.crawl_truncation_count = 2;
                    Ok(if first_call { vec![candidate] } else { vec![] })
                })
            });
        spider_candidates
            .expect_mark_crawl_truncated()
            .times(1)
            .withf(move |root, truncation_count, next_crawl_at| {
                root.listing_source_id == expected_listing_source_id
                    && *truncation_count == 0
                    && next_crawl_at_is_about(*next_crawl_at, TRUNCATED_CRAWL_RETRY_COOLDOWN)
            })
            .returning(|_, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(1)
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(truncated_run(false, Some(3))) }));

        spider_job(spider_candidates, spider_service, 1)
            .run_spider_once()
            .await;
    }

    /// A crawl the shutdown ended keeps its frontier in the spider service; the scheduler records
    /// it as its own outcome, due again soon, without a truncation, a failure or a completion.
    fn stopped_candidates(
        expected_listing_source_id: ListingSourceId,
    ) -> MockSpiderCandidateService {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let first_call = excluded.is_empty();
                Box::pin(async move {
                    Ok(if first_call {
                        vec![spider_candidate(expected_listing_source_id)]
                    } else {
                        vec![]
                    })
                })
            });
        spider_candidates
            .expect_mark_crawl_stopped()
            .times(1)
            .withf(move |root, next_crawl_at| {
                root.listing_source_id == expected_listing_source_id
                    && next_crawl_at_is_about(*next_crawl_at, STOPPED_CRAWL_RETRY_COOLDOWN)
            })
            .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        spider_candidates.expect_mark_crawl_truncated().times(0);
        spider_candidates.expect_mark_crawl_completed().times(0);
        spider_candidates.expect_mark_crawl_failure().times(0);
        spider_candidates
    }

    #[tokio::test]
    async fn should_record_a_stopped_crawl_as_due_soon_without_counting_a_truncation() {
        let spider_candidates = stopped_candidates(ListingSourceId::new());
        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(1)
            .returning(|_, _, _, _, _, _| {
                Box::pin(async { Ok(run_result(CrawlCompletion::Stopped)) })
            });

        spider_job(spider_candidates, spider_service, 1)
            .run_spider_once()
            .await;
    }

    #[tokio::test]
    async fn should_not_count_a_failure_when_the_shutdown_left_too_few_pages_to_judge() {
        let spider_candidates = stopped_candidates(ListingSourceId::new());
        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(1)
            .returning(|_, _, _, _, _, _| {
                Box::pin(async {
                    Err(SpiderServiceError::NoProductUrls {
                        crawl_root_url: "https://example.com".to_string(),
                        total_links: 3,
                        product_pattern: "/p/".to_string(),
                        completion: CrawlCompletion::Stopped,
                    })
                })
            });

        spider_job(spider_candidates, spider_service, 1)
            .run_spider_once()
            .await;
    }

    #[test]
    fn should_persist_the_stopped_outcome_in_screaming_snake_case() {
        assert_eq!(CrawlCompletion::Stopped.as_str(), "STOPPED");
        assert_eq!(
            serde_json::to_string(&CrawlCompletion::Stopped).unwrap(),
            "\"STOPPED\""
        );
    }

    #[tokio::test]
    async fn should_treat_page_budget_as_completed_crawl() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_listing_source_id = ListingSourceId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let first_call = excluded.is_empty();
                Box::pin(async move {
                    Ok(if first_call {
                        vec![spider_candidate(expected_listing_source_id)]
                    } else {
                        vec![]
                    })
                })
            });
        spider_candidates
            .expect_mark_crawl_completed()
            .times(1)
            .withf(move |root, completion| {
                root.listing_source_id == expected_listing_source_id
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
        let expected_listing_source_id = ListingSourceId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let first_call = excluded.is_empty();
                Box::pin(async move {
                    if !first_call {
                        return Ok(vec![]);
                    }
                    let mut candidate = spider_candidate(expected_listing_source_id);
                    candidate.crawl_truncation_count = 3;
                    Ok(vec![candidate])
                })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .times(1)
            .withf(move |root, error_kind, failure_count, _, next_crawl_at| {
                root.listing_source_id == expected_listing_source_id
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
    async fn should_complete_other_sources_while_one_crawl_is_truncated() {
        let truncated_listing_source_id = ListingSourceId::new();
        let completed_listing_source_id = ListingSourceId::new();

        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let first_call = excluded.is_empty();
                Box::pin(async move {
                    Ok(if first_call {
                        vec![
                            spider_candidate(truncated_listing_source_id),
                            spider_candidate(completed_listing_source_id),
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
                root.listing_source_id == truncated_listing_source_id && *truncation_count == 1
            })
            .returning(|_, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        spider_candidates
            .expect_mark_crawl_completed()
            .times(1)
            .withf(move |root, completion| {
                root.listing_source_id == completed_listing_source_id
                    && *completion == CrawlCompletion::Completed
            })
            .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .times(2)
            .returning(move |source, _, _, _, _, _| {
                let result = if *source == truncated_listing_source_id {
                    truncated_run(true, Some(10))
                } else {
                    run_result(CrawlCompletion::Completed)
                };
                Box::pin(async move { Ok(result) })
            });

        spider_job(spider_candidates, spider_service, 2)
            .run_spider_once()
            .await;
    }

    #[tokio::test]
    async fn should_run_spider_candidate_while_the_scraper_holds_its_own_source_lock() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let listing_source_id = ListingSourceId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(listing_source_id)]) })
            });
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
                        frontier_left: None,
                        db_wait: Duration::ZERO,
                        ran_out_of_time: false,
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
            WorkLock::try_acquire(&lock_manager, WorkLockScope::Scraper, listing_source_id)
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

    #[tokio::test]
    async fn should_skip_spider_candidate_when_source_lock_is_already_held() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let locked_listing_source_id = ListingSourceId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                Box::pin(async move { Ok(vec![spider_candidate(locked_listing_source_id)]) })
            });

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().times(0);

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();
        let lock_manager = Arc::new(LocalLockManager::new());
        let _prelock = WorkLock::try_acquire(
            &lock_manager,
            WorkLockScope::Spider,
            locked_listing_source_id,
        )
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
        let listing_source_id = ListingSourceId::new();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, excluded| {
                let first_call = excluded.is_empty();
                Box::pin(async move {
                    Ok(if first_call {
                        vec![spider_candidate(listing_source_id)]
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
                root.listing_source_id == listing_source_id
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

    /// A candidate service with one due ListingSource that records no outcome but a review wait,
    /// which its write reports as `wait_write`.
    fn candidate_service_recording_review_wait(
        listing_source_id: ListingSourceId,
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
                    let mut candidate = spider_candidate(listing_source_id);
                    candidate.crawl_failure_count = 2;
                    candidate.last_crawl_error_kind = Some("RateLimited".to_string());
                    Ok(vec![candidate])
                })
            });
        spider_candidates
            .expect_mark_crawl_awaiting_url_pattern_review()
            .times(1)
            .withf(move |root, written_review_id, rate_limit, next_crawl_at| {
                root.listing_source_id == listing_source_id
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
            ListingSourceId::new(),
            review_id,
            UrlPatternReviewWaitWrite::Recorded,
        );

        spider_job(spider_candidates, spider_service_waiting_on(review_id), 1)
            .run_spider_once()
            .await;
    }

    fn failing_candidate_service(
        listing_source_id: ListingSourceId,
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
                    let mut candidate = spider_candidate(listing_source_id);
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
        let expected_listing_source_id = ListingSourceId::new();
        let mut spider_candidates =
            failing_candidate_service(expected_listing_source_id, 3, "TinyCrawl");
        spider_candidates
            .expect_mark_crawl_failure()
            .times(1)
            .withf(move |root, error_kind, failure_count, _, next_crawl_at| {
                root.listing_source_id == expected_listing_source_id
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
    async fn should_stop_source_for_operator_when_no_pattern_failures_keep_repeating() {
        let expected_listing_source_id = ListingSourceId::new();
        let mut spider_candidates = failing_candidate_service(
            expected_listing_source_id,
            NEEDS_OPERATOR_FAILURE_COUNT - 1,
            "NoUrlPattern",
        );
        spider_candidates
            .expect_mark_crawl_needs_operator()
            .times(1)
            .withf(move |root, error_kind, failure_count| {
                root.listing_source_id == expected_listing_source_id
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
    async fn should_stop_source_for_operator_when_no_product_url_failures_alternate_kinds() {
        let expected_listing_source_id = ListingSourceId::new();
        let mut spider_candidates = failing_candidate_service(
            expected_listing_source_id,
            NEEDS_OPERATOR_FAILURE_COUNT - 1,
            "InsufficientInferenceSample",
        );
        spider_candidates
            .expect_mark_crawl_needs_operator()
            .times(1)
            .withf(move |root, error_kind, failure_count| {
                root.listing_source_id == expected_listing_source_id
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

    fn bot_challenged_crawl() -> SpiderServiceError {
        SpiderServiceError::BotChallengedCrawl {
            bot_challenge_pages: 30,
            pages_received: 31,
            failure: Box::new(no_product_urls()),
        }
    }

    /// Alternating `BotProtection` and no-product runs reach the operator at the shared count,
    /// and a bot-protected run below it keeps retrying on the transient cooldown.
    #[tokio::test]
    async fn should_stop_source_for_operator_when_bot_protection_continues_a_no_product_streak() {
        let expected_listing_source_id = ListingSourceId::new();
        let mut spider_candidates = failing_candidate_service(
            expected_listing_source_id,
            NEEDS_OPERATOR_FAILURE_COUNT - 1,
            "NoProductUrls",
        );
        spider_candidates
            .expect_mark_crawl_needs_operator()
            .times(1)
            .withf(move |root, error_kind, failure_count| {
                root.listing_source_id == expected_listing_source_id
                    && error_kind == "BotProtection"
                    && *failure_count == NEEDS_OPERATOR_FAILURE_COUNT
            })
            .returning(|_, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        spider_candidates.expect_mark_crawl_failure().times(0);

        spider_job(
            spider_candidates,
            spider_service_failing_with(bot_challenged_crawl),
            1,
        )
        .run_spider_once()
        .await;
    }

    #[tokio::test]
    async fn should_keep_retrying_bot_protection_below_the_escalation_count() {
        let expected_listing_source_id = ListingSourceId::new();
        let mut spider_candidates = failing_candidate_service(
            expected_listing_source_id,
            NEEDS_OPERATOR_FAILURE_COUNT - 2,
            "BotProtection",
        );
        spider_candidates
            .expect_mark_crawl_failure()
            .times(1)
            .withf(move |root, error_kind, failure_count, _, _| {
                root.listing_source_id == expected_listing_source_id
                    && error_kind == "BotProtection"
                    && *failure_count == NEEDS_OPERATOR_FAILURE_COUNT - 1
            })
            .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        spider_candidates
            .expect_mark_crawl_needs_operator()
            .times(0);

        spider_job(
            spider_candidates,
            spider_service_failing_with(bot_challenged_crawl),
            1,
        )
        .run_spider_once()
        .await;
    }

    /// Runs the scheduler once on a ListingSource one failure short of `escalation_count` in a
    /// streak of `expected_kind`, whose run fails with `failure`, and expects the run to stop it
    /// for an operator instead of scheduling a retry.
    async fn expect_stop_for_operator(
        failure: fn() -> SpiderServiceError,
        expected_kind: &'static str,
        escalation_count: i32,
    ) {
        let expected_listing_source_id = ListingSourceId::new();
        let mut spider_candidates = failing_candidate_service(
            expected_listing_source_id,
            escalation_count - 1,
            expected_kind,
        );
        spider_candidates
            .expect_mark_crawl_needs_operator()
            .times(1)
            .withf(move |root, error_kind, failure_count| {
                root.listing_source_id == expected_listing_source_id
                    && error_kind == expected_kind
                    && *failure_count == escalation_count
            })
            .returning(|_, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        spider_candidates.expect_mark_crawl_failure().times(0);

        spider_job(spider_candidates, spider_service_failing_with(failure), 1)
            .run_spider_once()
            .await;
    }

    #[tokio::test]
    async fn should_stop_an_unreachable_shop_for_an_operator_at_the_escalation_count() {
        expect_stop_for_operator(
            || preflight_failed(CrawlFailureKind::ConnectError),
            "ConnectError",
            NEEDS_OPERATOR_FAILURE_COUNT,
        )
        .await;
    }

    #[tokio::test]
    async fn should_stop_a_refusing_shop_for_an_operator_on_its_second_failure() {
        expect_stop_for_operator(
            || crawl_failed(CrawlFailureKind::AccessDenied),
            "AccessDenied",
            DURABLE_BLOCK_NEEDS_OPERATOR_FAILURE_COUNT,
        )
        .await;
    }

    mod crawl_metrics {
        use super::*;
        use crate::metrics::test_support::capture_metrics;

        /// The one record a spider pass publishes: the failed runs and the pass duration, under
        /// the empty dimension set, with no figure that names a source.
        fn assert_one_dimensionless_record(
            metrics: &crate::metrics::test_support::CapturedMetrics,
            crawl_runs_failed: u64,
        ) {
            let records = metrics.records();
            assert_eq!(records.len(), 1, "one record per pass");
            let record = &records[0];
            assert_eq!(
                record["_aws"]["CloudWatchMetrics"][0]["Dimensions"],
                serde_json::json!([[]])
            );
            assert_eq!(
                record[CrawlerMetric::CrawlRunsFailed.name()],
                crawl_runs_failed
            );
            assert!(record[CrawlerMetric::SpiderPassDuration.name()].is_u64());
            assert!(record.get("listing_source_id").is_none());
        }

        #[tokio::test]
        async fn should_count_no_failed_run_when_a_crawl_is_truncated() {
            let (metrics, _guard) = capture_metrics();
            let listing_source_id = ListingSourceId::new();
            let mut spider_candidates = MockSpiderCandidateService::new();
            spider_candidates
                .expect_get_candidates()
                .returning(move |_, excluded| {
                    let first_call = excluded.is_empty();
                    Box::pin(async move {
                        Ok(if first_call {
                            vec![spider_candidate(listing_source_id)]
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

            assert_one_dimensionless_record(&metrics, 0);
        }

        /// A crawl that stops its source for an operator is a failed run: the pass counts it in
        /// the metric the escalation alarm reads, and the run's `crawler.spider.crawl_failed`
        /// event names the source.
        #[tokio::test]
        async fn should_count_a_source_stopped_for_an_operator_as_a_failed_run() {
            let (metrics, _guard) = capture_metrics();
            let listing_source_id = ListingSourceId::new();
            let mut spider_candidates = failing_candidate_service(
                listing_source_id,
                NEEDS_OPERATOR_FAILURE_COUNT - 1,
                "NoUrlPattern",
            );
            spider_candidates
                .expect_mark_crawl_needs_operator()
                .returning(|_, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

            spider_job(
                spider_candidates,
                spider_service_failing_with(no_url_pattern),
                1,
            )
            .run_spider_once()
            .await;

            assert_one_dimensionless_record(&metrics, 1);
        }

        #[tokio::test]
        async fn should_count_a_failed_crawl_that_is_retried_as_a_failed_run() {
            let (metrics, _guard) = capture_metrics();
            let mut spider_candidates =
                failing_candidate_service(ListingSourceId::new(), 0, "NoUrlPattern");
            spider_candidates
                .expect_mark_crawl_failure()
                .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));

            spider_job(
                spider_candidates,
                spider_service_failing_with(no_url_pattern),
                1,
            )
            .run_spider_once()
            .await;

            assert_one_dimensionless_record(&metrics, 1);
        }

        #[tokio::test]
        async fn should_publish_the_record_when_no_source_is_due() {
            let (metrics, _guard) = capture_metrics();
            let mut spider_candidates = MockSpiderCandidateService::new();
            spider_candidates
                .expect_get_candidates()
                .returning(|_, _| Box::pin(async { Ok(vec![]) }));

            spider_job(spider_candidates, MockSpiderService::new(), 1)
                .run_spider_once()
                .await;

            assert_one_dimensionless_record(&metrics, 0);
        }

        /// A review decided before the run records its wait, such as one rejected while the crawl
        /// ran, leaves the ListingSource's crawl state as the decision left it: the run writes
        /// nothing else and is no failed run.
        #[tokio::test]
        async fn should_count_no_failed_run_when_the_url_pattern_review_is_decided_before_the_wait()
        {
            let (metrics, _guard) = capture_metrics();
            let review_id = CrawlerReviewId::new();
            let spider_candidates = candidate_service_recording_review_wait(
                ListingSourceId::new(),
                review_id,
                UrlPatternReviewWaitWrite::ReviewNotPending,
            );

            spider_job(spider_candidates, spider_service_waiting_on(review_id), 1)
                .run_spider_once()
                .await;

            assert_one_dimensionless_record(&metrics, 0);
        }
    }

    /// A ListingSource whose presentation URL a scope refresh moved to another crawl root while a
    /// run crawled the old one (`docs/crawler/README.md#listingsource-scope`).
    mod crawl_root_moved_during_the_run {
        use super::*;
        use crate::review::repository::CrawlerReviewRepository;
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
        use crate::test_support::refresh_scope_with_crawl_root;
        use sqlx::PgPool;
        use test_api::IntegrationTestService as _;
        use tokio::sync::{mpsc, oneshot};

        /// A candidate service whose every write of a run's outcome names `root` and finds the
        /// ListingSource on another crawl root.
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
            let listing_source_id = ListingSourceId::new();
            let full = || spider_candidate(listing_source_id);
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
            }
        }

        const OLD_ROOT: &str = "https://moving-shop.example.com/";
        const NEW_ROOT: &str = "https://www.moving-shop.example.com/";

        /// A crawl of the old root during which a scope refresh moves the source to `NEW_ROOT`. It
        /// then delivers product and listing pages of the old root and leaves one unvisited.
        fn spider_moving_the_root_mid_crawl(
            pool: PgPool,
            listing_source_id: ListingSourceId,
        ) -> MockSpider {
            let mut spider = MockSpider::new();
            spider.expect_crawl().times(1).return_once(move |request| {
                Box::pin(async move {
                    assert_eq!(request.crawl_root_url, OLD_ROOT);
                    refresh_scope_with_crawl_root(&pool, listing_source_id, NEW_ROOT).await;
                    let page = |path: &str| {
                        let url = url::Url::parse(OLD_ROOT).unwrap().join(path).unwrap();
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

        /// The source's crawl root, last full crawl, failure count, outcome and last run.
        type CrawlState = (
            String,
            Option<time::OffsetDateTime>,
            i32,
            Option<String>,
            Option<time::OffsetDateTime>,
        );

        async fn crawl_state(pool: &PgPool, listing_source_id: ListingSourceId) -> CrawlState {
            sqlx::query_as(
                "SELECT crawl_root_url, last_crawled, crawl_failure_count, last_crawl_outcome, \
                        last_crawl_run_at \
                 FROM listing_sources WHERE listing_source_id = $1",
            )
            .bind(listing_source_id.as_uuid())
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
            refresh_scope_with_crawl_root(&pool, listing_source_id, OLD_ROOT).await;
            sqlx::query(
                "UPDATE listing_sources \
                 SET url_pattern = '/item/', url_pattern_state = 'MATCHED', url_pattern_matching = 'FULL_URL_SEARCH', \
                     crawl_failure_count = 1, last_crawl_error_kind = 'ConnectError', \
                     last_crawl_outcome = 'FAILED', last_crawl_run_at = NOW() - INTERVAL '1 day' \
                 WHERE listing_source_id = $1",
            )
            .bind(listing_source_id.as_uuid())
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
                crawl_state(&pool, listing_source_id).await,
                (NEW_ROOT.to_owned(), None, 0, None, None),
                "the move's cleared crawl state survives the old root's completed crawl"
            );
            assert!(
                CrawlFrontierRepositoryImpl::new(pool.clone())
                    .load_frontier(&listing_source_id)
                    .await
                    .unwrap()
                    .is_empty(),
                "the old root's frontier is not kept"
            );
            let stored_urls: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM listing_source_urls WHERE listing_source_id = $1",
            )
            .bind(listing_source_id.as_uuid())
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
                (next[0].crawl_root.url().as_str(), next[0].kind),
                (NEW_ROOT, CrawlKind::Full),
                "the next spider cycle crawls the new root in full"
            );
        }
    }

    /// A shop whose crawls are challenged, alternating with crawls that find no product URL,
    /// driven through the scheduler and the crawler's PostgreSQL candidate service: it escalates
    /// at the shared streak's count, leaves the schedule, and an operator's re-arm restarts the
    /// streak (`docs/crawler/scheduling-and-retries.md#spider-failure-escalation`).
    mod bot_protection_escalation {
        use super::*;
        use crate::spider::candidate_service::SpiderCandidateServiceImpl;
        use crate::test_support::refresh_scope_with_crawl_root;
        use sqlx::PgPool;
        use std::collections::VecDeque;
        use std::sync::Mutex;
        use test_api::IntegrationTestService as _;

        const ROOT: &str = "https://challenged-shop.example.com/";
        const MOVED_ROOT: &str = "https://www.challenged-shop.example.com/";

        /// How one spider run fails.
        type RunFailure = fn() -> SpiderServiceError;

        /// A spider service whose runs fail with `failures`, in order, one per run.
        fn spider_service_failing_in_turn(failures: Vec<RunFailure>) -> MockSpiderService {
            let failures = Arc::new(Mutex::new(VecDeque::from(failures)));
            let mut spider_service = MockSpiderService::new();
            let runs = failures.lock().unwrap().len();
            spider_service
                .expect_run()
                .times(runs)
                .returning(move |_, _, _, _, _, _| {
                    let failure = failures.lock().unwrap().pop_front().unwrap();
                    Box::pin(async move { Err(failure()) })
                });
            spider_service
        }

        fn scheduler(pool: &PgPool, spider_service: MockSpiderService) -> CrawlerCronJob {
            let mut scraper_candidates = MockScraperCandidateService::new();
            scraper_candidates
                .expect_get_candidates()
                .returning(|_, _, _| Box::pin(async { Ok(vec![]) }));
            CrawlerCronJob::new(
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
            )
        }

        /// The source's failure count, last error kind and outcome, and whether a cooldown holds it.
        async fn failure_state(
            pool: &PgPool,
            listing_source_id: ListingSourceId,
        ) -> (i32, Option<String>, Option<String>, bool) {
            sqlx::query_as(
                "SELECT crawl_failure_count, last_crawl_error_kind, last_crawl_outcome, \
                        next_crawl_at IS NOT NULL \
                 FROM listing_sources WHERE listing_source_id = $1",
            )
            .bind(listing_source_id.as_uuid())
            .fetch_one(pool)
            .await
            .unwrap()
        }

        /// Lets the next spider cycle select the source as if its cooldown had passed.
        async fn expire_cooldown(pool: &PgPool, listing_source_id: ListingSourceId) {
            sqlx::query(
                "UPDATE listing_sources SET next_crawl_at = NULL WHERE listing_source_id = $1",
            )
            .bind(listing_source_id.as_uuid())
            .execute(pool)
            .await
            .unwrap();
        }

        async fn is_candidate(pool: &PgPool) -> bool {
            !SpiderCandidateServiceImpl::new(pool.clone())
                .get_candidates(10, &[])
                .await
                .unwrap()
                .is_empty()
        }

        #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
        async fn should_stop_an_alternating_challenged_shop_for_an_operator_and_restart_on_rearm() {
            let pool = test_api::get_postgres_client().await;
            let listing_source_id = ListingSourceId::new();
            sqlx::query(
                "INSERT INTO listing_sources \
                 (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled) \
                 VALUES ($1, 'Challenged shop', 'challenged-shop', TRUE)",
            )
            .bind(listing_source_id.as_uuid())
            .execute(&pool)
            .await
            .unwrap();
            refresh_scope_with_crawl_root(&pool, listing_source_id, ROOT).await;
            let runs: [(RunFailure, &str); 5] = [
                (bot_challenged_crawl, "BotProtection"),
                (no_product_urls, "NoProductUrls"),
                (bot_challenged_crawl, "BotProtection"),
                (no_url_pattern, "NoUrlPattern"),
                (bot_challenged_crawl, "BotProtection"),
            ];
            let job = scheduler(
                &pool,
                spider_service_failing_in_turn(
                    runs.iter()
                        .map(|(failure, _)| *failure)
                        .chain([bot_challenged_crawl as RunFailure])
                        .collect(),
                ),
            );

            for (run, (_, error_kind)) in runs.iter().enumerate() {
                let failure_count = i32::try_from(run).unwrap() + 1;
                assert!(
                    is_candidate(&pool).await,
                    "run {failure_count} is scheduled"
                );
                job.run_spider_once().await;
                let escalated = failure_count == NEEDS_OPERATOR_FAILURE_COUNT;
                assert_eq!(
                    failure_state(&pool, listing_source_id).await,
                    (
                        failure_count,
                        Some((*error_kind).to_owned()),
                        Some(
                            if escalated {
                                NEEDS_OPERATOR_CRAWL_OUTCOME
                            } else {
                                FAILED_CRAWL_OUTCOME
                            }
                            .to_owned()
                        ),
                        !escalated,
                    ),
                    "run {failure_count} continues the shared streak"
                );
                expire_cooldown(&pool, listing_source_id).await;
            }
            assert!(
                !is_candidate(&pool).await,
                "a source waiting for an operator is not crawled again"
            );

            refresh_scope_with_crawl_root(&pool, listing_source_id, MOVED_ROOT).await;
            assert!(
                is_candidate(&pool).await,
                "the source re-armed by a moved crawl root is scheduled"
            );
            job.run_spider_once().await;

            assert_eq!(
                failure_state(&pool, listing_source_id).await,
                (
                    1,
                    Some("BotProtection".to_owned()),
                    Some(FAILED_CRAWL_OUTCOME.to_owned()),
                    true,
                ),
                "the re-arm restarts the streak"
            );
        }
    }
}
