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
    unchanged: usize,
    deferred: usize,
    /// Skipped URLs by reason; empty when nothing was skipped.
    skip_reasons: BTreeMap<&'static str, usize>,
    /// Deferred URLs by reason; empty when nothing was deferred.
    deferred_reasons: BTreeMap<&'static str, usize>,
}

/// The most domains one pass summary lists, so a pass over every ListingSource stays one record
/// of bounded size.
const MAX_LOGGED_DOMAINS: usize = 50;

impl DomainSummary {
    fn total(&self) -> usize {
        self.succeeded + self.failed + self.awaiting_schema_review + self.skipped + self.deferred
    }
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
            unchanged: outcome.unchanged,
            deferred: outcome.deferred_urls,
            skip_reasons: outcome.skip_reasons.clone(),
            deferred_reasons: outcome.deferred_reasons.clone(),
        });
    }

    /// The skipped URLs of every domain, by reason.
    pub(super) fn skip_reasons(&self) -> BTreeMap<&'static str, usize> {
        Self::sum_by_reason(self.domains.iter().map(|domain| &domain.skip_reasons))
    }

    /// The deferred URLs of every domain, by reason.
    pub(super) fn deferred_reasons(&self) -> BTreeMap<&'static str, usize> {
        Self::sum_by_reason(self.domains.iter().map(|domain| &domain.deferred_reasons))
    }

    fn sum_by_reason<'a>(
        per_domain: impl Iterator<Item = &'a BTreeMap<&'static str, usize>>,
    ) -> BTreeMap<&'static str, usize> {
        let mut total = BTreeMap::new();
        for reasons in per_domain {
            for (reason, count) in reasons {
                *total.entry(*reason).or_default() += count;
            }
        }
        total
    }

    /// The domains that ran, as the JSON array the event carries them in: a log field cannot
    /// nest, and the domains differ in number from pass to pass. A pass over many ListingSources
    /// lists the [`MAX_LOGGED_DOMAINS`] with the most URLs, and
    /// [`PassSummary::domains_left_out`] counts the rest.
    pub(super) fn by_domain_json(&self) -> String {
        let mut busiest: Vec<&DomainSummary> = self.domains.iter().collect();
        busiest.sort_by(|a, b| {
            b.total()
                .cmp(&a.total())
                .then_with(|| a.domain.cmp(&b.domain))
        });
        busiest.truncate(MAX_LOGGED_DOMAINS);
        serde_json::to_string(&busiest).unwrap_or_else(|_| "[]".to_owned())
    }

    pub(super) fn domains_left_out(&self) -> usize {
        self.domains.len().saturating_sub(MAX_LOGGED_DOMAINS)
    }

    /// `reasons` as the JSON object the event carries it in.
    pub(super) fn reasons_json(reasons: &BTreeMap<&'static str, usize>) -> String {
        serde_json::to_string(reasons).unwrap_or_else(|_| "{}".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::cron::scraper::DeferralReason;

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
    fn should_sum_the_deferral_reasons_and_count_unchanged_urls_when_the_pass_ends() {
        let mut first = ScrapeBatchOutcome::default();
        first.record_deferred(DeferralReason::TimeBudget, 5);
        first.unchanged = 2;
        let mut second = ScrapeBatchOutcome::default();
        second.record_deferred(DeferralReason::TimeBudget, 1);
        second.record_deferred(DeferralReason::UrlLock, 1);
        let mut summary = PassSummary::default();
        summary.observe_batch("a.example", "A", &first);
        summary.observe_batch("b.example", "B", &second);

        let reasons = summary.deferred_reasons();

        assert_eq!(reasons.get("time_budget"), Some(&6));
        assert_eq!(reasons.get("url_lock"), Some(&1));
        let json: serde_json::Value = serde_json::from_str(&summary.by_domain_json())
            .unwrap_or_else(|error| panic!("the breakdown is JSON: {error}"));
        assert_eq!(json[0]["unchanged"], 2);
        assert_eq!(json[0]["deferred"], 5);
    }

    #[test]
    fn should_list_the_busiest_domains_only_when_a_pass_ran_over_many() {
        let mut summary = PassSummary::default();
        for index in 0..60 {
            let outcome = ScrapeBatchOutcome {
                succeeded: index,
                ..Default::default()
            };
            summary.observe_batch(&format!("shop-{index:02}.example"), "Shop", &outcome);
        }

        let json: Vec<serde_json::Value> = serde_json::from_str(&summary.by_domain_json())
            .unwrap_or_else(|error| panic!("the breakdown is JSON: {error}"));

        assert_eq!(json.len(), MAX_LOGGED_DOMAINS);
        assert_eq!(json[0]["domain"], "shop-59.example");
        assert_eq!(summary.domains_left_out(), 10);
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
