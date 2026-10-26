//! Scraper domain circuit trip rules: which fetch failures turn into a
//! domain-wide disposition instead of staying with their URL.
//!
//! The rules are pure. The scraper cron gathers the batch evidence, asks for a
//! decision per failure and persists its outcome; `docs/crawler/scheduling-and-retries.md`
//! (scraper domain circuits) owns the durable contract.

use crate::network::policy::{DomainFailureKind, NetworkErrorKind, domain_failure_kind};
use std::collections::HashSet;
use url::Url;

/// Distinct URLs of one batch that answer 500 or 502 before the short
/// `HTTP_5XX_BURST` circuit opens; a single 500 or 502 stays URL-scoped.
pub const SERVER_FAILURE_BURST_URLS: usize = 3;

/// What one fetch failure means for its domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainCircuitDecision {
    /// Open the circuit with this kind: the domain cools down and the rest of
    /// the batch stops.
    Open(DomainFailureKind),
    /// The failure stays with its URL.
    UrlLocal,
}

/// Evidence gathered over one scraper domain batch. Every URL counts once,
/// so a URL that fails repeatedly is one piece of evidence, not several.
#[derive(Debug, Default)]
pub struct DomainBatchEvidence {
    server_failure_urls: HashSet<Url>,
}

impl DomainBatchEvidence {
    /// Decides what a candidate's fetch failure means for the domain, given
    /// the evidence gathered before it.
    pub fn decide(
        &self,
        candidate_url: &Url,
        kind: NetworkErrorKind,
        status_code: Option<u16>,
    ) -> DomainCircuitDecision {
        if let Some(kind) = domain_failure_kind(kind) {
            return DomainCircuitDecision::Open(kind);
        }
        match status_code {
            Some(500 | 502)
                if distinct_with(&self.server_failure_urls, candidate_url)
                    >= SERVER_FAILURE_BURST_URLS =>
            {
                DomainCircuitDecision::Open(DomainFailureKind::Http5xxBurst)
            }
            _ => DomainCircuitDecision::UrlLocal,
        }
    }

    /// Records the HTTP status a candidate's fetch failed with.
    pub fn observe_http_failure(&mut self, candidate_url: &Url, status_code: u16) {
        if matches!(status_code, 500 | 502) {
            self.server_failure_urls.insert(candidate_url.clone());
        }
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

    #[test]
    fn should_open_the_circuit_on_a_domain_failure_kind() {
        let evidence = DomainBatchEvidence::default();

        assert_eq!(
            evidence.decide(&url("1"), NetworkErrorKind::HttpStatus(429), Some(429)),
            DomainCircuitDecision::Open(DomainFailureKind::Http429)
        );
    }

    #[test]
    fn should_keep_a_single_server_failure_url_local() {
        let evidence = DomainBatchEvidence::default();

        assert_eq!(
            evidence.decide(&url("1"), NetworkErrorKind::HttpStatus(500), Some(500)),
            DomainCircuitDecision::UrlLocal
        );
    }

    #[test]
    fn should_open_a_5xx_burst_on_the_third_distinct_server_failure_url() {
        let mut evidence = DomainBatchEvidence::default();
        evidence.observe_http_failure(&url("1"), 500);
        evidence.observe_http_failure(&url("1"), 500);
        evidence.observe_http_failure(&url("2"), 502);

        assert_eq!(
            evidence.decide(&url("1"), NetworkErrorKind::HttpStatus(500), Some(500)),
            DomainCircuitDecision::UrlLocal,
            "a URL that already counts is not a third piece of evidence"
        );
        assert_eq!(
            evidence.decide(&url("3"), NetworkErrorKind::HttpStatus(502), Some(502)),
            DomainCircuitDecision::Open(DomainFailureKind::Http5xxBurst)
        );
    }

    #[test]
    fn should_ignore_statuses_outside_the_server_failure_burst() {
        let mut evidence = DomainBatchEvidence::default();
        evidence.observe_http_failure(&url("1"), 405);
        evidence.observe_http_failure(&url("2"), 405);

        assert_eq!(
            evidence.decide(&url("3"), NetworkErrorKind::HttpStatus(405), Some(405)),
            DomainCircuitDecision::UrlLocal
        );
    }
}
