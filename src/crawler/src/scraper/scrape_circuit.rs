//! Scrape circuit trip rules: which fetch failures turn into a
//! ListingSource-wide disposition instead of staying with their URL.
//!
//! The rules are pure. The scraper cron gathers the batch evidence, asks for a
//! decision per failure and persists its outcome; `docs/crawler/scheduling-and-retries.md`
//! (scraper circuits) owns the durable contract.

use crate::network::policy::{
    DomainFailureKind, NetworkErrorKind, domain_failure_kind, is_crawler_rejection_status,
};
use crate::network::politeness::{RATE_LIMIT_MAX_PAUSE, politeness_group};
use crate::scraper::candidate_service::{ScrapeHealthSnapshot, next_scrape_failure_streak};
use crate::scraper::scraper_service::DomainFetchHealth;
use std::collections::HashSet;
use std::time::Duration;
use url::Url;

/// Consecutive same-kind failures of a slow-host kind (timeout, connect, DNS
/// timeout) that open the circuit. The streak is persisted, so the failures
/// may span scheduler passes; a responsive fetch between them resets it.
pub const SLOW_HOST_CONSECUTIVE_FAILURES: u32 = 2;
/// Distinct URLs of one batch that must fail with a slow-host kind before
/// their share of the attempted URLs is judged.
pub const SLOW_HOST_BATCH_MIN_URLS: usize = 3;
/// Distinct URLs of one batch that answer 500 or 502 before the short
/// `HTTP_5XX_BURST` circuit opens; a single 500 or 502 stays URL-scoped.
pub const SERVER_FAILURE_BURST_URLS: usize = 3;
/// Distinct URLs of one batch that answer the same crawler-rejection status,
/// or a bot challenge with the same status, in a row before the
/// `HTTP_STATUS_BURST` circuit opens; a single such answer stays URL-scoped.
pub const REJECTION_STATUS_BURST_URLS: usize = 3;
/// Distinct URLs of one batch that answer 429 on a shared platform before the
/// `HTTP_429` circuit opens although each answer asked for a short wait
/// (`docs/crawler/scheduling-and-retries.md#scraper-circuits`).
pub const SHARED_PLATFORM_RATE_LIMIT_URLS: usize = 3;

/// The HTTP answer a failed fetch got, as the trip rules read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailedAnswer {
    status: u16,
    bot_challenge: bool,
}

impl FailedAnswer {
    /// The answer a fetch that failed with `kind` and `status` got; `None`
    /// when the failure carries no HTTP status.
    pub fn of(kind: NetworkErrorKind, status: Option<u16>) -> Option<Self> {
        status.map(|status| Self {
            status,
            bot_challenge: kind == NetworkErrorKind::BotChallenge,
        })
    }

    /// Whether the answer rejects the crawler as such rather than one page:
    /// a crawler-rejection status, or a bot challenge, whatever status it was
    /// answered with.
    fn rejects_crawler(self) -> bool {
        self.bot_challenge || is_crawler_rejection_status(self.status)
    }

    fn is_server_failure(self) -> bool {
        matches!(self.status, 500 | 502)
    }
}

/// What one fetch failure means for its ListingSource's scrape circuit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrapeCircuitDecision {
    /// Open the circuit with this kind: the ListingSource cools down and the rest of
    /// the batch stops.
    Open(DomainFailureKind),
    /// Persist the kind and streak as evidence; the ListingSource stays open for
    /// scraping and the batch continues.
    RecordFailure(DomainFailureKind),
    /// The failure stays with its URL.
    UrlLocal,
    /// A 429 on a shared platform that asked for a short wait: the URL is
    /// deferred while the platform's politeness group waits, and the circuit
    /// stays closed.
    DeferRateLimited,
}

/// Evidence gathered over one scraper batch. Every URL counts once,
/// so a URL that fails repeatedly is one piece of evidence, not several.
#[derive(Debug, Default)]
pub struct BatchEvidence {
    /// Candidates whose fetch reached a transport observation.
    attempted_urls: HashSet<Url>,
    slow_host_urls: HashSet<Url>,
    server_failure_urls: HashSet<Url>,
    /// The URLs that answered 429.
    rate_limited_urls: HashSet<Url>,
    /// The URLs that answered the same crawler-rejection status, or a bot
    /// challenge with the same status, since the last other outcome.
    rejection_run: Option<RejectionRun>,
}

#[derive(Debug)]
struct RejectionRun {
    status: u16,
    urls: HashSet<Url>,
}

impl BatchEvidence {
    /// Decides what a candidate's fetch failure means for the ListingSource, given
    /// its current scrape-health snapshot, the evidence gathered before it and
    /// the `Retry-After` the answer carried.
    pub fn decide(
        &self,
        scrape_health: &ScrapeHealthSnapshot,
        candidate_url: &Url,
        kind: NetworkErrorKind,
        status_code: Option<u16>,
        retry_after: Option<Duration>,
    ) -> ScrapeCircuitDecision {
        if kind == NetworkErrorKind::HttpStatus(429)
            && self.defers_rate_limit(scrape_health, candidate_url, retry_after)
        {
            return ScrapeCircuitDecision::DeferRateLimited;
        }
        match domain_failure_kind(kind) {
            Some(kind) if kind.opens_on_first_failure() => ScrapeCircuitDecision::Open(kind),
            Some(kind) => {
                let sustained = next_scrape_failure_streak(scrape_health, kind)
                    >= SLOW_HOST_CONSECUTIVE_FAILURES
                    || self.slow_host_share_reached_with(candidate_url);
                if sustained {
                    ScrapeCircuitDecision::Open(kind)
                } else {
                    ScrapeCircuitDecision::RecordFailure(kind)
                }
            }
            None => match FailedAnswer::of(kind, status_code) {
                Some(answer)
                    if answer.is_server_failure()
                        && distinct_with(&self.server_failure_urls, candidate_url)
                            >= SERVER_FAILURE_BURST_URLS =>
                {
                    ScrapeCircuitDecision::Open(DomainFailureKind::Http5xxBurst)
                }
                Some(answer)
                    if answer.rejects_crawler()
                        && (is_status_burst_probe(scrape_health)
                            || self.rejection_run_with(answer, candidate_url)
                                >= REJECTION_STATUS_BURST_URLS) =>
                {
                    ScrapeCircuitDecision::Open(DomainFailureKind::HttpStatusBurst)
                }
                _ => ScrapeCircuitDecision::UrlLocal,
            },
        }
    }

    /// Records what a candidate's fetch observed: the transport health and,
    /// for an HTTP failure, its answer.
    pub fn observe(
        &mut self,
        candidate_url: &Url,
        transport: &DomainFetchHealth,
        failure: Option<FailedAnswer>,
    ) {
        if matches!(transport, DomainFetchHealth::NotObserved) {
            return;
        }
        self.extend_or_end_rejection_run(candidate_url, transport, failure);
        match transport {
            DomainFetchHealth::Responsive => {
                if failure.is_some_and(FailedAnswer::is_server_failure) {
                    self.server_failure_urls.insert(candidate_url.clone());
                }
            }
            DomainFetchHealth::CircuitFailure { kind, .. } if !kind.opens_on_first_failure() => {
                self.slow_host_urls.insert(candidate_url.clone());
            }
            _ => {}
        }
        if failure.is_some_and(|answer| answer.status == 429) {
            self.rate_limited_urls.insert(candidate_url.clone());
        }
        self.attempted_urls.insert(candidate_url.clone());
    }

    /// Candidates of the batch whose fetch reached a transport observation.
    pub fn attempted_urls(&self) -> usize {
        self.attempted_urls.len()
    }

    /// Candidates of the batch that failed with a slow-host kind.
    pub fn slow_host_urls(&self) -> usize {
        self.slow_host_urls.len()
    }

    /// Distinct URLs in the current run of answers with `answer`'s status
    /// once `candidate_url` answers it too; zero when `answer` does not reject
    /// the crawler as such.
    pub fn rejection_run_with(&self, answer: FailedAnswer, candidate_url: &Url) -> usize {
        if !answer.rejects_crawler() {
            return 0;
        }
        match &self.rejection_run {
            Some(run) if run.status == answer.status => distinct_with(&run.urls, candidate_url),
            _ => 1,
        }
    }

    /// An answer that rejects the crawler extends the run of its status or
    /// starts a new one; any other observed outcome ends the run.
    fn extend_or_end_rejection_run(
        &mut self,
        candidate_url: &Url,
        transport: &DomainFetchHealth,
        failure: Option<FailedAnswer>,
    ) {
        let rejection_status = match (transport, failure) {
            (DomainFetchHealth::Responsive, Some(answer)) if answer.rejects_crawler() => {
                answer.status
            }
            _ => {
                self.rejection_run = None;
                return;
            }
        };
        match &mut self.rejection_run {
            Some(run) if run.status == rejection_status => {
                run.urls.insert(candidate_url.clone());
            }
            run => {
                *run = Some(RejectionRun {
                    status: rejection_status,
                    urls: HashSet::from([candidate_url.clone()]),
                });
            }
        }
    }

    /// Whether a 429 stays with its URL: the shop is on a listed shared
    /// platform, whose politeness group waits out the answer's `Retry-After`
    /// in full, and fewer than [`SHARED_PLATFORM_RATE_LIMIT_URLS`] distinct
    /// URLs of the batch answered 429 once `candidate_url` counts. A 429
    /// without `Retry-After`, or one asking for more than the group waits,
    /// opens the circuit as before, and so does a 429 to the probe of an open
    /// circuit, so its cooldown keeps growing instead of the ListingSource
    /// being probed again on every pass.
    fn defers_rate_limit(
        &self,
        scrape_health: &ScrapeHealthSnapshot,
        candidate_url: &Url,
        retry_after: Option<Duration>,
    ) -> bool {
        let shared_platform = candidate_url
            .host_str()
            .is_some_and(|host| politeness_group(host).shared_platform);
        shared_platform
            && scrape_health.next_scrape_at.is_none()
            && retry_after.is_some_and(|wait| wait <= RATE_LIMIT_MAX_PAUSE)
            && distinct_with(&self.rate_limited_urls, candidate_url)
                < SHARED_PLATFORM_RATE_LIMIT_URLS
    }

    /// Whether, once `candidate_url` fails too, at least
    /// [`SLOW_HOST_BATCH_MIN_URLS`] and half of the attempted URLs failed
    /// with a slow-host kind.
    fn slow_host_share_reached_with(&self, candidate_url: &Url) -> bool {
        let failed = distinct_with(&self.slow_host_urls, candidate_url);
        let attempted = distinct_with(&self.attempted_urls, candidate_url);
        failed >= SLOW_HOST_BATCH_MIN_URLS && failed * 2 >= attempted
    }
}

/// Whether the ListingSource is being probed after a `HTTP_STATUS_BURST` cooldown:
/// a probe that is rejected the same way keeps the circuit open, with a
/// grown cooldown, instead of closing it as a responsive fetch.
fn is_status_burst_probe(scrape_health: &ScrapeHealthSnapshot) -> bool {
    scrape_health.next_scrape_at.is_some()
        && scrape_health.last_scrape_error_kind.as_deref()
            == Some(DomainFailureKind::HttpStatusBurst.as_str())
}

/// How many distinct URLs `urls` holds once `url` is counted.
fn distinct_with(urls: &HashSet<Url>, url: &Url) -> usize {
    urls.len() + usize::from(!urls.contains(url))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(path: &str) -> Url {
        Url::parse(&format!("https://shop.example.com/{path}")).expect("test URL")
    }

    fn healthy() -> ScrapeHealthSnapshot {
        ScrapeHealthSnapshot::default()
    }

    fn recorded(kind: DomainFailureKind, streak: i32) -> ScrapeHealthSnapshot {
        ScrapeHealthSnapshot {
            scrape_failure_streak: streak,
            last_scrape_error_kind: Some(kind.as_str().to_owned()),
            next_scrape_at: None,
        }
    }

    fn timeout(transport: &DomainFetchHealth) -> bool {
        matches!(
            transport,
            DomainFetchHealth::CircuitFailure {
                kind: DomainFailureKind::Timeout,
                ..
            }
        )
    }

    fn status(code: u16) -> Option<FailedAnswer> {
        FailedAnswer::of(NetworkErrorKind::HttpStatus(code), Some(code))
    }

    fn challenge(code: u16) -> Option<FailedAnswer> {
        FailedAnswer::of(NetworkErrorKind::BotChallenge, Some(code))
    }

    fn timed_out() -> DomainFetchHealth {
        DomainFetchHealth::CircuitFailure {
            kind: DomainFailureKind::Timeout,
            status_code: None,
            retry_after: None,
        }
    }

    #[test]
    fn should_open_the_circuit_on_a_first_observation_kind() {
        let evidence = BatchEvidence::default();

        assert_eq!(
            evidence.decide(
                &healthy(),
                &url("1"),
                NetworkErrorKind::HttpStatus(429),
                Some(429),
                None
            ),
            ScrapeCircuitDecision::Open(DomainFailureKind::Http429)
        );
        assert_eq!(
            evidence.decide(
                &healthy(),
                &url("1"),
                NetworkErrorKind::DnsResolution,
                None,
                None
            ),
            ScrapeCircuitDecision::Open(DomainFailureKind::DnsResolution)
        );
    }

    #[test]
    fn should_record_a_single_timeout_without_opening() {
        let evidence = BatchEvidence::default();

        assert_eq!(
            evidence.decide(&healthy(), &url("1"), NetworkErrorKind::Timeout, None, None),
            ScrapeCircuitDecision::RecordFailure(DomainFailureKind::Timeout)
        );
        assert_eq!(
            evidence.decide(&healthy(), &url("1"), NetworkErrorKind::Connect, None, None),
            ScrapeCircuitDecision::RecordFailure(DomainFailureKind::Connect)
        );
    }

    #[test]
    fn should_open_on_the_second_consecutive_timeout() {
        let evidence = BatchEvidence::default();

        assert_eq!(
            evidence.decide(
                &recorded(DomainFailureKind::Timeout, 1),
                &url("1"),
                NetworkErrorKind::Timeout,
                None,
                None
            ),
            ScrapeCircuitDecision::Open(DomainFailureKind::Timeout)
        );
    }

    #[test]
    fn should_restart_the_streak_when_the_slow_host_kind_changes() {
        let evidence = BatchEvidence::default();

        assert_eq!(
            evidence.decide(
                &recorded(DomainFailureKind::Timeout, 1),
                &url("1"),
                NetworkErrorKind::Connect,
                None,
                None
            ),
            ScrapeCircuitDecision::RecordFailure(DomainFailureKind::Connect)
        );
    }

    #[test]
    fn should_open_when_half_of_a_batch_times_out() {
        let mut evidence = BatchEvidence::default();
        evidence.observe(&url("1"), &timed_out(), None);
        evidence.observe(&url("2"), &DomainFetchHealth::Responsive, None);
        evidence.observe(&url("3"), &timed_out(), None);
        evidence.observe(&url("4"), &DomainFetchHealth::Responsive, None);
        evidence.observe(&url("5"), &DomainFetchHealth::Responsive, None);
        assert!(timeout(&timed_out()));

        assert_eq!(
            evidence.decide(&healthy(), &url("6"), NetworkErrorKind::Timeout, None, None),
            ScrapeCircuitDecision::Open(DomainFailureKind::Timeout),
            "three of six attempted URLs timed out"
        );
        assert_eq!(
            evidence.decide(&healthy(), &url("3"), NetworkErrorKind::Timeout, None, None),
            ScrapeCircuitDecision::RecordFailure(DomainFailureKind::Timeout),
            "a URL that already counts is not a third piece of evidence"
        );
    }

    #[test]
    fn should_not_open_on_a_small_timeout_share() {
        let mut evidence = BatchEvidence::default();
        evidence.observe(&url("1"), &timed_out(), None);
        for index in 2..=6 {
            evidence.observe(
                &url(&index.to_string()),
                &DomainFetchHealth::Responsive,
                None,
            );
        }
        evidence.observe(&url("7"), &timed_out(), None);
        evidence.observe(&url("8"), &DomainFetchHealth::NotObserved, None);

        assert_eq!(evidence.attempted_urls(), 7);
        assert_eq!(evidence.slow_host_urls(), 2);
        assert_eq!(
            evidence.decide(&healthy(), &url("9"), NetworkErrorKind::Timeout, None, None),
            ScrapeCircuitDecision::RecordFailure(DomainFailureKind::Timeout),
            "three of eight attempted URLs is below half"
        );
    }

    #[test]
    fn should_keep_a_single_server_failure_url_local() {
        let evidence = BatchEvidence::default();

        assert_eq!(
            evidence.decide(
                &healthy(),
                &url("1"),
                NetworkErrorKind::HttpStatus(500),
                Some(500),
                None
            ),
            ScrapeCircuitDecision::UrlLocal
        );
    }

    #[test]
    fn should_open_a_5xx_burst_on_the_third_distinct_server_failure_url() {
        let mut evidence = BatchEvidence::default();
        evidence.observe(&url("1"), &DomainFetchHealth::Responsive, status(500));
        evidence.observe(&url("1"), &DomainFetchHealth::Responsive, status(500));
        evidence.observe(&url("2"), &DomainFetchHealth::Responsive, status(502));

        assert_eq!(
            evidence.decide(
                &healthy(),
                &url("1"),
                NetworkErrorKind::HttpStatus(500),
                Some(500),
                None
            ),
            ScrapeCircuitDecision::UrlLocal,
            "a URL that already counts is not a third piece of evidence"
        );
        assert_eq!(
            evidence.decide(
                &healthy(),
                &url("3"),
                NetworkErrorKind::HttpStatus(502),
                Some(502),
                None
            ),
            ScrapeCircuitDecision::Open(DomainFailureKind::Http5xxBurst)
        );
    }

    #[test]
    fn should_ignore_statuses_outside_the_server_failure_burst() {
        let mut evidence = BatchEvidence::default();
        evidence.observe(&url("1"), &DomainFetchHealth::Responsive, status(400));
        evidence.observe(&url("2"), &DomainFetchHealth::Responsive, status(400));

        assert_eq!(
            evidence.decide(
                &healthy(),
                &url("3"),
                NetworkErrorKind::HttpStatus(400),
                Some(400),
                None
            ),
            ScrapeCircuitDecision::UrlLocal
        );
    }

    fn rejected(evidence: &BatchEvidence, path: &str, status: u16) -> ScrapeCircuitDecision {
        rejected_with(evidence, &healthy(), path, status)
    }

    fn rejected_with(
        evidence: &BatchEvidence,
        scrape_health: &ScrapeHealthSnapshot,
        path: &str,
        status: u16,
    ) -> ScrapeCircuitDecision {
        evidence.decide(
            scrape_health,
            &url(path),
            NetworkErrorKind::HttpStatus(status),
            Some(status),
            None,
        )
    }

    #[test]
    fn should_keep_a_single_rejection_status_url_local() {
        let evidence = BatchEvidence::default();

        assert_eq!(
            rejected(&evidence, "1", 405),
            ScrapeCircuitDecision::UrlLocal
        );
    }

    #[test]
    fn should_open_a_status_burst_on_the_third_distinct_url_answering_the_same_rejection_status() {
        let mut evidence = BatchEvidence::default();
        evidence.observe(&url("1"), &DomainFetchHealth::Responsive, status(405));
        evidence.observe(&url("1"), &DomainFetchHealth::Responsive, status(405));
        evidence.observe(&url("2"), &DomainFetchHealth::Responsive, status(405));

        assert_eq!(
            rejected(&evidence, "1", 405),
            ScrapeCircuitDecision::UrlLocal,
            "a URL that already counts is not a third piece of evidence"
        );
        assert_eq!(
            evidence.rejection_run_with(status(405).unwrap(), &url("3")),
            3
        );
        assert_eq!(
            rejected(&evidence, "3", 405),
            ScrapeCircuitDecision::Open(DomainFailureKind::HttpStatusBurst)
        );
    }

    #[test]
    fn should_end_the_rejection_run_on_any_other_observed_outcome() {
        let outcomes = [
            (DomainFetchHealth::Responsive, None),
            (DomainFetchHealth::Responsive, status(500)),
            (DomainFetchHealth::Responsive, status(403)),
            (timed_out(), None),
        ];
        for (transport, failure_status) in outcomes {
            let mut evidence = BatchEvidence::default();
            evidence.observe(&url("1"), &DomainFetchHealth::Responsive, status(405));
            evidence.observe(&url("2"), &DomainFetchHealth::Responsive, status(405));
            evidence.observe(&url("3"), &transport, failure_status);

            assert_eq!(
                rejected(&evidence, "4", 405),
                ScrapeCircuitDecision::UrlLocal,
                "{transport:?} with {failure_status:?} ends the run"
            );
        }
    }

    #[test]
    fn should_keep_the_rejection_run_across_unobserved_candidates() {
        let mut evidence = BatchEvidence::default();
        evidence.observe(&url("1"), &DomainFetchHealth::Responsive, status(405));
        evidence.observe(&url("2"), &DomainFetchHealth::NotObserved, None);
        evidence.observe(&url("3"), &DomainFetchHealth::Responsive, status(405));

        assert_eq!(
            rejected(&evidence, "4", 405),
            ScrapeCircuitDecision::Open(DomainFailureKind::HttpStatusBurst)
        );
    }

    #[test]
    fn should_never_count_product_removal_or_other_terminal_statuses() {
        for code in [400, 404, 410, 422] {
            let mut evidence = BatchEvidence::default();
            evidence.observe(&url("1"), &DomainFetchHealth::Responsive, status(code));
            evidence.observe(&url("2"), &DomainFetchHealth::Responsive, status(code));

            assert_eq!(
                rejected(&evidence, "3", code),
                ScrapeCircuitDecision::UrlLocal,
                "HTTP {code} stays with its URL"
            );
        }
    }

    #[test]
    fn should_keep_a_status_burst_open_when_its_probe_is_rejected_again() {
        let evidence = BatchEvidence::default();
        let mut cooled_down = recorded(DomainFailureKind::HttpStatusBurst, 1);
        cooled_down.next_scrape_at = Some(time::OffsetDateTime::now_utc());

        assert_eq!(
            rejected_with(&evidence, &cooled_down, "1", 405),
            ScrapeCircuitDecision::Open(DomainFailureKind::HttpStatusBurst)
        );
        assert_eq!(
            rejected_with(
                &evidence,
                &recorded(DomainFailureKind::HttpStatusBurst, 1),
                "1",
                405
            ),
            ScrapeCircuitDecision::UrlLocal,
            "a burst without a cooldown is not a probe"
        );
        let mut other_circuit = recorded(DomainFailureKind::Http429, 1);
        other_circuit.next_scrape_at = Some(time::OffsetDateTime::now_utc());
        assert_eq!(
            rejected_with(&evidence, &other_circuit, "1", 405),
            ScrapeCircuitDecision::UrlLocal
        );
    }

    fn challenged_with(
        evidence: &BatchEvidence,
        scrape_health: &ScrapeHealthSnapshot,
        path: &str,
        status: u16,
    ) -> ScrapeCircuitDecision {
        evidence.decide(
            scrape_health,
            &url(path),
            NetworkErrorKind::BotChallenge,
            Some(status),
            None,
        )
    }

    /// A shop that answers every page with a challenge refuses the crawler as
    /// a whole, whether the challenge comes as a `202` interstitial or a `200`
    /// bot check, so its URLs are not refetched every cooldown.
    #[test]
    fn should_open_a_status_burst_on_the_third_distinct_url_answering_a_bot_challenge() {
        for code in [200, 202, 403] {
            let mut evidence = BatchEvidence::default();
            assert_eq!(
                challenged_with(&evidence, &healthy(), "1", code),
                ScrapeCircuitDecision::UrlLocal,
                "a single challenge answered with {code} stays with its URL"
            );
            evidence.observe(&url("1"), &DomainFetchHealth::Responsive, challenge(code));
            evidence.observe(&url("2"), &DomainFetchHealth::Responsive, challenge(code));

            assert_eq!(
                challenged_with(&evidence, &healthy(), "3", code),
                ScrapeCircuitDecision::Open(DomainFailureKind::HttpStatusBurst),
                "three challenges answered with {code}"
            );
        }
    }

    #[test]
    fn should_end_a_challenge_run_on_a_page_or_a_challenge_with_another_status() {
        for (transport, failure) in [
            (DomainFetchHealth::Responsive, None),
            (DomainFetchHealth::Responsive, challenge(200)),
        ] {
            let mut evidence = BatchEvidence::default();
            evidence.observe(&url("1"), &DomainFetchHealth::Responsive, challenge(202));
            evidence.observe(&url("2"), &DomainFetchHealth::Responsive, challenge(202));
            evidence.observe(&url("3"), &transport, failure);

            assert_eq!(
                challenged_with(&evidence, &healthy(), "4", 202),
                ScrapeCircuitDecision::UrlLocal,
                "{failure:?} ends the run"
            );
        }
    }

    #[test]
    fn should_keep_a_status_burst_open_when_its_probe_meets_a_challenge_again() {
        let evidence = BatchEvidence::default();
        let mut cooled_down = recorded(DomainFailureKind::HttpStatusBurst, 1);
        cooled_down.next_scrape_at = Some(time::OffsetDateTime::now_utc());

        assert_eq!(
            challenged_with(&evidence, &cooled_down, "1", 202),
            ScrapeCircuitDecision::Open(DomainFailureKind::HttpStatusBurst)
        );
    }

    fn platform_url(path: &str) -> Url {
        Url::parse(&format!("https://antiquariat.shopnetzwerk.com/{path}")).expect("test URL")
    }

    fn rate_limited(
        evidence: &BatchEvidence,
        url: &Url,
        retry_after: Option<Duration>,
    ) -> ScrapeCircuitDecision {
        evidence.decide(
            &healthy(),
            url,
            NetworkErrorKind::HttpStatus(429),
            Some(429),
            retry_after,
        )
    }

    #[test]
    fn should_defer_a_shared_platform_429_with_a_short_retry_after_until_it_repeats() {
        let short = Some(Duration::from_secs(8));
        let mut evidence = BatchEvidence::default();

        let first = rate_limited(&evidence, &platform_url("1"), short);
        evidence.observe(
            &platform_url("1"),
            &DomainFetchHealth::Responsive,
            status(429),
        );
        let second = rate_limited(&evidence, &platform_url("2"), short);
        evidence.observe(
            &platform_url("2"),
            &DomainFetchHealth::Responsive,
            status(429),
        );
        let third = rate_limited(&evidence, &platform_url("3"), short);

        assert_eq!(first, ScrapeCircuitDecision::DeferRateLimited);
        assert_eq!(second, ScrapeCircuitDecision::DeferRateLimited);
        assert_eq!(
            third,
            ScrapeCircuitDecision::Open(DomainFailureKind::Http429)
        );
    }

    #[test]
    fn should_open_the_circuit_on_a_shared_platform_429_without_a_short_retry_after() {
        let evidence = BatchEvidence::default();

        for retry_after in [None, Some(Duration::from_secs(300))] {
            assert_eq!(
                rate_limited(&evidence, &platform_url("1"), retry_after),
                ScrapeCircuitDecision::Open(DomainFailureKind::Http429),
                "{retry_after:?}"
            );
        }
    }

    #[test]
    fn should_reopen_the_circuit_when_its_probe_answers_a_short_429_on_a_shared_platform() {
        let probe = ScrapeHealthSnapshot {
            scrape_failure_streak: 1,
            last_scrape_error_kind: Some(DomainFailureKind::Http429.as_str().to_owned()),
            next_scrape_at: Some(time::OffsetDateTime::UNIX_EPOCH),
        };

        assert_eq!(
            BatchEvidence::default().decide(
                &probe,
                &platform_url("1"),
                NetworkErrorKind::HttpStatus(429),
                Some(429),
                Some(Duration::from_secs(8)),
            ),
            ScrapeCircuitDecision::Open(DomainFailureKind::Http429)
        );
    }

    #[test]
    fn should_open_the_circuit_on_a_429_off_a_shared_platform() {
        let evidence = BatchEvidence::default();

        assert_eq!(
            rate_limited(&evidence, &url("1"), Some(Duration::from_secs(8))),
            ScrapeCircuitDecision::Open(DomainFailureKind::Http429)
        );
    }
}
