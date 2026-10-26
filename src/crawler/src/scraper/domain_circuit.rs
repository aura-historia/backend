//! Scraper domain circuit trip rules: which fetch failures turn into a
//! domain-wide disposition instead of staying with their URL.
//!
//! The rules are pure. The scraper cron gathers the batch evidence, asks for a
//! decision per failure and persists its outcome; `docs/crawler/scheduling-and-retries.md`
//! (scraper domain circuits) owns the durable contract.

use crate::network::policy::{DomainFailureKind, NetworkErrorKind, domain_failure_kind};
use crate::scraper::candidate_service::{DomainHealthSnapshot, next_domain_failure_streak};
use crate::scraper::scraper_service::DomainFetchHealth;
use std::collections::HashSet;
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
/// Distinct URLs of one batch that answer the same crawler-rejection status
/// in a row before the `HTTP_STATUS_BURST` circuit opens; a single such
/// status stays URL-scoped.
pub const REJECTION_STATUS_BURST_URLS: usize = 3;

/// Terminal statuses that reject the crawler or its request as such rather
/// than one page, so the same answer from several distinct URLs means the
/// domain fails uniformly. HTTP 404 and 410 are product removals and never
/// belong here.
pub fn is_crawler_rejection_status(status: u16) -> bool {
    matches!(status, 401 | 403 | 405 | 418 | 421 | 451 | 501)
}

/// What one fetch failure means for its domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainCircuitDecision {
    /// Open the circuit with this kind: the domain cools down and the rest of
    /// the batch stops.
    Open(DomainFailureKind),
    /// Persist the kind and streak as evidence; the domain stays open for
    /// scraping and the batch continues.
    RecordFailure(DomainFailureKind),
    /// The failure stays with its URL.
    UrlLocal,
}

/// Evidence gathered over one scraper domain batch. Every URL counts once,
/// so a URL that fails repeatedly is one piece of evidence, not several.
#[derive(Debug, Default)]
pub struct DomainBatchEvidence {
    /// Candidates whose fetch reached a transport observation.
    attempted_urls: HashSet<Url>,
    slow_host_urls: HashSet<Url>,
    server_failure_urls: HashSet<Url>,
    /// The URLs that answered the same crawler-rejection status since the
    /// last other outcome.
    rejection_run: Option<RejectionRun>,
}

#[derive(Debug)]
struct RejectionRun {
    status: u16,
    urls: HashSet<Url>,
}

impl DomainBatchEvidence {
    /// Decides what a candidate's fetch failure means for the domain, given
    /// the domain's current snapshot and the evidence gathered before it.
    pub fn decide(
        &self,
        domain_health: &DomainHealthSnapshot,
        candidate_url: &Url,
        kind: NetworkErrorKind,
        status_code: Option<u16>,
    ) -> DomainCircuitDecision {
        match domain_failure_kind(kind) {
            Some(kind) if kind.opens_on_first_failure() => DomainCircuitDecision::Open(kind),
            Some(kind) => {
                let sustained = next_domain_failure_streak(domain_health, kind)
                    >= SLOW_HOST_CONSECUTIVE_FAILURES
                    || self.slow_host_share_reached_with(candidate_url);
                if sustained {
                    DomainCircuitDecision::Open(kind)
                } else {
                    DomainCircuitDecision::RecordFailure(kind)
                }
            }
            None => match status_code {
                Some(500 | 502)
                    if distinct_with(&self.server_failure_urls, candidate_url)
                        >= SERVER_FAILURE_BURST_URLS =>
                {
                    DomainCircuitDecision::Open(DomainFailureKind::Http5xxBurst)
                }
                Some(status)
                    if is_crawler_rejection_status(status)
                        && (is_status_burst_probe(domain_health)
                            || self.rejection_run_with(status, candidate_url)
                                >= REJECTION_STATUS_BURST_URLS) =>
                {
                    DomainCircuitDecision::Open(DomainFailureKind::HttpStatusBurst)
                }
                _ => DomainCircuitDecision::UrlLocal,
            },
        }
    }

    /// Records what a candidate's fetch observed: the transport health and,
    /// for an HTTP failure, its status.
    pub fn observe(
        &mut self,
        candidate_url: &Url,
        transport: &DomainFetchHealth,
        failure_status: Option<u16>,
    ) {
        if matches!(transport, DomainFetchHealth::NotObserved) {
            return;
        }
        self.extend_or_end_rejection_run(candidate_url, transport, failure_status);
        match transport {
            DomainFetchHealth::Responsive => {
                if let Some(500 | 502) = failure_status {
                    self.server_failure_urls.insert(candidate_url.clone());
                }
            }
            DomainFetchHealth::CircuitFailure { kind, .. } if !kind.opens_on_first_failure() => {
                self.slow_host_urls.insert(candidate_url.clone());
            }
            _ => {}
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

    /// Distinct URLs in the current run of `status` answers once
    /// `candidate_url` answers it too; zero when `status` is not a
    /// crawler-rejection status.
    pub fn rejection_run_with(&self, status: u16, candidate_url: &Url) -> usize {
        if !is_crawler_rejection_status(status) {
            return 0;
        }
        match &self.rejection_run {
            Some(run) if run.status == status => distinct_with(&run.urls, candidate_url),
            _ => 1,
        }
    }

    /// A crawler-rejection status extends the run of that status or starts a
    /// new one; any other observed outcome ends the run.
    fn extend_or_end_rejection_run(
        &mut self,
        candidate_url: &Url,
        transport: &DomainFetchHealth,
        failure_status: Option<u16>,
    ) {
        let rejection_status = match (transport, failure_status) {
            (DomainFetchHealth::Responsive, Some(status))
                if is_crawler_rejection_status(status) =>
            {
                status
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

    /// Whether, once `candidate_url` fails too, at least
    /// [`SLOW_HOST_BATCH_MIN_URLS`] and half of the attempted URLs failed
    /// with a slow-host kind.
    fn slow_host_share_reached_with(&self, candidate_url: &Url) -> bool {
        let failed = distinct_with(&self.slow_host_urls, candidate_url);
        let attempted = distinct_with(&self.attempted_urls, candidate_url);
        failed >= SLOW_HOST_BATCH_MIN_URLS && failed * 2 >= attempted
    }
}

/// Whether the domain is being probed after a `HTTP_STATUS_BURST` cooldown:
/// a probe that is rejected the same way keeps the circuit open, with a
/// grown cooldown, instead of closing it as a responsive fetch.
fn is_status_burst_probe(domain_health: &DomainHealthSnapshot) -> bool {
    domain_health.next_scrape_at.is_some()
        && domain_health.last_scrape_error_kind.as_deref()
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

    fn healthy() -> DomainHealthSnapshot {
        DomainHealthSnapshot::default()
    }

    fn recorded(kind: DomainFailureKind, streak: i32) -> DomainHealthSnapshot {
        DomainHealthSnapshot {
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

    fn timed_out() -> DomainFetchHealth {
        DomainFetchHealth::CircuitFailure {
            kind: DomainFailureKind::Timeout,
            status_code: None,
            retry_after: None,
        }
    }

    #[test]
    fn should_open_the_circuit_on_a_first_observation_kind() {
        let evidence = DomainBatchEvidence::default();

        assert_eq!(
            evidence.decide(
                &healthy(),
                &url("1"),
                NetworkErrorKind::HttpStatus(429),
                Some(429)
            ),
            DomainCircuitDecision::Open(DomainFailureKind::Http429)
        );
        assert_eq!(
            evidence.decide(&healthy(), &url("1"), NetworkErrorKind::DnsResolution, None),
            DomainCircuitDecision::Open(DomainFailureKind::DnsResolution)
        );
    }

    #[test]
    fn should_record_a_single_timeout_without_opening() {
        let evidence = DomainBatchEvidence::default();

        assert_eq!(
            evidence.decide(&healthy(), &url("1"), NetworkErrorKind::Timeout, None),
            DomainCircuitDecision::RecordFailure(DomainFailureKind::Timeout)
        );
        assert_eq!(
            evidence.decide(&healthy(), &url("1"), NetworkErrorKind::Connect, None),
            DomainCircuitDecision::RecordFailure(DomainFailureKind::Connect)
        );
    }

    #[test]
    fn should_open_on_the_second_consecutive_timeout() {
        let evidence = DomainBatchEvidence::default();

        assert_eq!(
            evidence.decide(
                &recorded(DomainFailureKind::Timeout, 1),
                &url("1"),
                NetworkErrorKind::Timeout,
                None
            ),
            DomainCircuitDecision::Open(DomainFailureKind::Timeout)
        );
    }

    #[test]
    fn should_restart_the_streak_when_the_slow_host_kind_changes() {
        let evidence = DomainBatchEvidence::default();

        assert_eq!(
            evidence.decide(
                &recorded(DomainFailureKind::Timeout, 1),
                &url("1"),
                NetworkErrorKind::Connect,
                None
            ),
            DomainCircuitDecision::RecordFailure(DomainFailureKind::Connect)
        );
    }

    #[test]
    fn should_open_when_half_of_a_batch_times_out() {
        let mut evidence = DomainBatchEvidence::default();
        evidence.observe(&url("1"), &timed_out(), None);
        evidence.observe(&url("2"), &DomainFetchHealth::Responsive, None);
        evidence.observe(&url("3"), &timed_out(), None);
        evidence.observe(&url("4"), &DomainFetchHealth::Responsive, None);
        evidence.observe(&url("5"), &DomainFetchHealth::Responsive, None);
        assert!(timeout(&timed_out()));

        assert_eq!(
            evidence.decide(&healthy(), &url("6"), NetworkErrorKind::Timeout, None),
            DomainCircuitDecision::Open(DomainFailureKind::Timeout),
            "three of six attempted URLs timed out"
        );
        assert_eq!(
            evidence.decide(&healthy(), &url("3"), NetworkErrorKind::Timeout, None),
            DomainCircuitDecision::RecordFailure(DomainFailureKind::Timeout),
            "a URL that already counts is not a third piece of evidence"
        );
    }

    #[test]
    fn should_not_open_on_a_small_timeout_share() {
        let mut evidence = DomainBatchEvidence::default();
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
            evidence.decide(&healthy(), &url("9"), NetworkErrorKind::Timeout, None),
            DomainCircuitDecision::RecordFailure(DomainFailureKind::Timeout),
            "three of eight attempted URLs is below half"
        );
    }

    #[test]
    fn should_keep_a_single_server_failure_url_local() {
        let evidence = DomainBatchEvidence::default();

        assert_eq!(
            evidence.decide(
                &healthy(),
                &url("1"),
                NetworkErrorKind::HttpStatus(500),
                Some(500)
            ),
            DomainCircuitDecision::UrlLocal
        );
    }

    #[test]
    fn should_open_a_5xx_burst_on_the_third_distinct_server_failure_url() {
        let mut evidence = DomainBatchEvidence::default();
        evidence.observe(&url("1"), &DomainFetchHealth::Responsive, Some(500));
        evidence.observe(&url("1"), &DomainFetchHealth::Responsive, Some(500));
        evidence.observe(&url("2"), &DomainFetchHealth::Responsive, Some(502));

        assert_eq!(
            evidence.decide(
                &healthy(),
                &url("1"),
                NetworkErrorKind::HttpStatus(500),
                Some(500)
            ),
            DomainCircuitDecision::UrlLocal,
            "a URL that already counts is not a third piece of evidence"
        );
        assert_eq!(
            evidence.decide(
                &healthy(),
                &url("3"),
                NetworkErrorKind::HttpStatus(502),
                Some(502)
            ),
            DomainCircuitDecision::Open(DomainFailureKind::Http5xxBurst)
        );
    }

    #[test]
    fn should_ignore_statuses_outside_the_server_failure_burst() {
        let mut evidence = DomainBatchEvidence::default();
        evidence.observe(&url("1"), &DomainFetchHealth::Responsive, Some(400));
        evidence.observe(&url("2"), &DomainFetchHealth::Responsive, Some(400));

        assert_eq!(
            evidence.decide(
                &healthy(),
                &url("3"),
                NetworkErrorKind::HttpStatus(400),
                Some(400)
            ),
            DomainCircuitDecision::UrlLocal
        );
    }

    fn rejected(evidence: &DomainBatchEvidence, path: &str, status: u16) -> DomainCircuitDecision {
        rejected_with(evidence, &healthy(), path, status)
    }

    fn rejected_with(
        evidence: &DomainBatchEvidence,
        domain_health: &DomainHealthSnapshot,
        path: &str,
        status: u16,
    ) -> DomainCircuitDecision {
        evidence.decide(
            domain_health,
            &url(path),
            NetworkErrorKind::HttpStatus(status),
            Some(status),
        )
    }

    #[test]
    fn should_keep_a_single_rejection_status_url_local() {
        let evidence = DomainBatchEvidence::default();

        assert_eq!(
            rejected(&evidence, "1", 405),
            DomainCircuitDecision::UrlLocal
        );
    }

    #[test]
    fn should_open_a_status_burst_on_the_third_distinct_url_answering_the_same_rejection_status() {
        let mut evidence = DomainBatchEvidence::default();
        evidence.observe(&url("1"), &DomainFetchHealth::Responsive, Some(405));
        evidence.observe(&url("1"), &DomainFetchHealth::Responsive, Some(405));
        evidence.observe(&url("2"), &DomainFetchHealth::Responsive, Some(405));

        assert_eq!(
            rejected(&evidence, "1", 405),
            DomainCircuitDecision::UrlLocal,
            "a URL that already counts is not a third piece of evidence"
        );
        assert_eq!(evidence.rejection_run_with(405, &url("3")), 3);
        assert_eq!(
            rejected(&evidence, "3", 405),
            DomainCircuitDecision::Open(DomainFailureKind::HttpStatusBurst)
        );
    }

    #[test]
    fn should_end_the_rejection_run_on_any_other_observed_outcome() {
        let outcomes = [
            (DomainFetchHealth::Responsive, None),
            (DomainFetchHealth::Responsive, Some(500)),
            (DomainFetchHealth::Responsive, Some(403)),
            (timed_out(), None),
        ];
        for (transport, failure_status) in outcomes {
            let mut evidence = DomainBatchEvidence::default();
            evidence.observe(&url("1"), &DomainFetchHealth::Responsive, Some(405));
            evidence.observe(&url("2"), &DomainFetchHealth::Responsive, Some(405));
            evidence.observe(&url("3"), &transport, failure_status);

            assert_eq!(
                rejected(&evidence, "4", 405),
                DomainCircuitDecision::UrlLocal,
                "{transport:?} with {failure_status:?} ends the run"
            );
        }
    }

    #[test]
    fn should_keep_the_rejection_run_across_unobserved_candidates() {
        let mut evidence = DomainBatchEvidence::default();
        evidence.observe(&url("1"), &DomainFetchHealth::Responsive, Some(405));
        evidence.observe(&url("2"), &DomainFetchHealth::NotObserved, None);
        evidence.observe(&url("3"), &DomainFetchHealth::Responsive, Some(405));

        assert_eq!(
            rejected(&evidence, "4", 405),
            DomainCircuitDecision::Open(DomainFailureKind::HttpStatusBurst)
        );
    }

    #[test]
    fn should_never_count_product_removal_or_other_terminal_statuses() {
        for status in [400, 404, 410, 422] {
            let mut evidence = DomainBatchEvidence::default();
            evidence.observe(&url("1"), &DomainFetchHealth::Responsive, Some(status));
            evidence.observe(&url("2"), &DomainFetchHealth::Responsive, Some(status));

            assert_eq!(
                rejected(&evidence, "3", status),
                DomainCircuitDecision::UrlLocal,
                "HTTP {status} stays with its URL"
            );
        }
    }

    #[test]
    fn should_keep_a_status_burst_open_when_its_probe_is_rejected_again() {
        let evidence = DomainBatchEvidence::default();
        let mut cooled_down = recorded(DomainFailureKind::HttpStatusBurst, 1);
        cooled_down.next_scrape_at = Some(time::OffsetDateTime::now_utc());

        assert_eq!(
            rejected_with(&evidence, &cooled_down, "1", 405),
            DomainCircuitDecision::Open(DomainFailureKind::HttpStatusBurst)
        );
        assert_eq!(
            rejected_with(
                &evidence,
                &recorded(DomainFailureKind::HttpStatusBurst, 1),
                "1",
                405
            ),
            DomainCircuitDecision::UrlLocal,
            "a burst without a cooldown is not a probe"
        );
        let mut other_circuit = recorded(DomainFailureKind::Http429, 1);
        other_circuit.next_scrape_at = Some(time::OffsetDateTime::now_utc());
        assert_eq!(
            rejected_with(&evidence, &other_circuit, "1", 405),
            DomainCircuitDecision::UrlLocal
        );
    }
}
