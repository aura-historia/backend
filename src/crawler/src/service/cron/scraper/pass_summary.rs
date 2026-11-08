//! The per-domain breakdown of one scraper scheduler pass, logged on
//! `crawler.scraper.pass_completed` so a pass that scraped nothing for a shop says why.

use super::ScrapeBatchOutcome;
use serde::Serialize;
use std::collections::BTreeMap;

pub(crate) const PASS_COMPLETED_EVENT: &str = "crawler.scraper.pass_completed";

/// What one ListingSource's scraper batch of the pass did, by candidate URL.
#[derive(Debug, Serialize, PartialEq, Eq)]
struct DomainSummary {
    domain: String,
    listing_source_name: String,
    succeeded: usize,
    failed: usize,
    awaiting_schema_review: usize,
    skipped: usize,
    deferred: usize,
    /// Skipped URLs by reason; empty when nothing was skipped.
    skip_reasons: BTreeMap<&'static str, usize>,
}

#[derive(Debug, Default)]
pub(super) struct PassSummary {
    domains: Vec<DomainSummary>,
}

impl PassSummary {
    /// Adds the batch a ListingSource's worker finished.
    pub(super) fn observe_batch(
        &mut self,
        domain: &str,
        listing_source_name: &str,
        outcome: &ScrapeBatchOutcome,
    ) {
        self.domains.push(DomainSummary {
            domain: domain.to_owned(),
            listing_source_name: listing_source_name.to_owned(),
            succeeded: outcome.succeeded,
            failed: outcome.failed,
            awaiting_schema_review: outcome.awaiting_schema_review,
            skipped: outcome.skipped,
            deferred: outcome.deferred_urls,
            skip_reasons: outcome.skip_reasons.clone(),
        });
    }

    /// The skipped URLs of every domain, by reason.
    pub(super) fn skip_reasons(&self) -> BTreeMap<&'static str, usize> {
        let mut total = BTreeMap::new();
        for domain in &self.domains {
            for (reason, count) in &domain.skip_reasons {
                *total.entry(*reason).or_default() += count;
            }
        }
        total
    }

    /// The domains that ran, as the JSON array the event carries them in: a log field cannot
    /// nest, and the domains differ in number from pass to pass.
    pub(super) fn by_domain_json(&self) -> String {
        serde_json::to_string(&self.domains).unwrap_or_else(|_| "[]".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(skip_reasons: &[(&'static str, usize)]) -> ScrapeBatchOutcome {
        let mut outcome = ScrapeBatchOutcome::default();
        for (reason, count) in skip_reasons {
            outcome.record_skipped(reason, *count);
        }
        outcome
    }

    #[test]
    fn should_sum_the_skip_reasons_of_every_domain_when_the_pass_ends() {
        let mut summary = PassSummary::default();
        summary.observe_batch("a.example", "A", &batch(&[("stale_write", 1)]));
        summary.observe_batch(
            "b.example",
            "B",
            &batch(&[("stale_write", 2), ("scrape_circuit_open", 4)]),
        );

        let reasons = summary.skip_reasons();

        assert_eq!(reasons.get("stale_write"), Some(&3));
        assert_eq!(reasons.get("scrape_circuit_open"), Some(&4));
    }

    #[test]
    fn should_list_every_domain_with_its_counts_when_the_pass_ends() {
        let mut summary = PassSummary::default();
        summary.observe_batch("a.example", "A", &batch(&[("stale_write", 1)]));

        let json: serde_json::Value = serde_json::from_str(&summary.by_domain_json())
            .unwrap_or_else(|error| panic!("the breakdown is JSON: {error}"));

        assert_eq!(json[0]["domain"], "a.example");
        assert_eq!(json[0]["skipped"], 1);
        assert_eq!(json[0]["skip_reasons"]["stale_write"], 1);
    }
}
