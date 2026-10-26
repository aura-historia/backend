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
        match transport {
            DomainFetchHealth::NotObserved => return,
            DomainFetchHealth::Responsive => {
                if let Some(500 | 502) = failure_status {
                    self.server_failure_urls.insert(candidate_url.clone());
                }
            }
            DomainFetchHealth::CircuitFailure { kind, .. } if !kind.opens_on_first_failure() => {
                self.slow_host_urls.insert(candidate_url.clone());
            }
            DomainFetchHealth::CircuitFailure { .. } => {}
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

    /// Whether, once `candidate_url` fails too, at least
    /// [`SLOW_HOST_BATCH_MIN_URLS`] and half of the attempted URLs failed
    /// with a slow-host kind.
    fn slow_host_share_reached_with(&self, candidate_url: &Url) -> bool {
        let failed = distinct_with(&self.slow_host_urls, candidate_url);
        let attempted = distinct_with(&self.attempted_urls, candidate_url);
        failed >= SLOW_HOST_BATCH_MIN_URLS && failed * 2 >= attempted
    }
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
        evidence.observe(&url("1"), &DomainFetchHealth::Responsive, Some(405));
        evidence.observe(&url("2"), &DomainFetchHealth::Responsive, Some(405));

        assert_eq!(
            evidence.decide(
                &healthy(),
                &url("3"),
                NetworkErrorKind::HttpStatus(405),
                Some(405)
            ),
            DomainCircuitDecision::UrlLocal
        );
    }
}
