//! Withdrawal of a stored listing whose page keeps refusing capture by its robots directives
//! (`docs/crawler/networking.md#page-robots-directives`). A refusal alone keeps the listing; once
//! the URL's [refusal run](PageDirectiveRefusalRun) is long enough, the refusal becomes a verified
//! removal, which the batch's mass-removal guard judges like any other. It never says the item
//! sold.

use crate::network::policy::NetworkErrorKind;
use crate::scraper::candidate_service::{PageDirectiveRefusalRun, ScraperCandidate};
use crate::scraper::scraper_service::ScraperError;
use crate::scraper::scraper_service::domain::errors::{
    ListingRemoval, RemovalEvidence, RemovalEvidenceKind,
};
use time::OffsetDateTime;

/// The removal a page-directive refusal of `candidate`'s own URL at `now` amounts to, when the
/// URL holds a captured listing and the refusal makes its run long enough; `None` otherwise, and
/// the refusal is recorded as a failure.
pub(super) fn page_directive_withdrawal(
    candidate: &ScraperCandidate,
    error: &ScraperError,
    now: OffsetDateTime,
) -> Option<ListingRemoval> {
    let ScraperError::HttpError {
        url,
        kind: NetworkErrorKind::PageDirectiveDisallowed,
        details,
    } = error
    else {
        return None;
    };
    // A refusal names the page that refused: the candidate's own, or a redirect target it led
    // to, which says nothing about whether the listing is still offered.
    if *url != candidate.url || candidate.last_captured_raw_input_sha256.is_none() {
        return None;
    }
    let run = PageDirectiveRefusalRun::after(candidate.page_directive_refusals, now);
    if !run.withdraws_listing(now) {
        return None;
    }
    Some(ListingRemoval {
        url: url.clone(),
        details: format!(
            "{details}; refused {} times in a row since {}",
            run.refusals, run.since
        ),
        evidence: RemovalEvidence {
            kind: RemovalEvidenceKind::PersistentPageDirectiveRefusal,
            status: None,
            final_url: None,
        },
        removed_by_templates: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::cron::test_support::scraper_candidate;
    use time::Duration;

    fn url() -> url::Url {
        url::Url::parse("https://shop.example/product/vase").unwrap()
    }

    fn refusal() -> ScraperError {
        ScraperError::HttpError {
            url: url(),
            kind: NetworkErrorKind::PageDirectiveDisallowed,
            details: "meta robots tag forbids capture (noindex)".to_owned(),
        }
    }

    /// A stored listing whose URL refused `refusals` times in a row, the first `hours_ago`.
    fn listed_candidate(refusals: u32, hours_ago: i64, now: OffsetDateTime) -> ScraperCandidate {
        let mut candidate = scraper_candidate("ListingSource", url());
        candidate.last_captured_raw_input_sha256 = Some(vec![7; 32]);
        candidate.page_directive_refusals = (refusals > 0).then(|| PageDirectiveRefusalRun {
            refusals,
            since: now - Duration::hours(hours_ago),
        });
        candidate
    }

    #[test]
    fn should_keep_the_listing_while_the_run_is_below_the_threshold() {
        let now = OffsetDateTime::now_utc();

        for (prior_refusals, hours_ago) in [(1, 24), (2, 48), (3, 71)] {
            assert!(
                page_directive_withdrawal(
                    &listed_candidate(prior_refusals, hours_ago, now),
                    &refusal(),
                    now
                )
                .is_none(),
                "{prior_refusals} refusals since {hours_ago} h"
            );
        }
    }

    #[test]
    fn should_withdraw_the_listing_once_the_run_reaches_the_threshold() {
        let now = OffsetDateTime::now_utc();

        let removal = page_directive_withdrawal(&listed_candidate(3, 72, now), &refusal(), now)
            .expect("the fourth daily refusal withdraws the listing");

        assert_eq!(removal.url, url());
        assert_eq!(
            removal.evidence.kind,
            RemovalEvidenceKind::PersistentPageDirectiveRefusal
        );
        assert!(removal.removed_by_templates.is_empty());
        assert!(removal.details.contains("refused 4 times in a row"));
    }

    /// A successful scrape ends the stored run, so the candidate carries none and the refusal
    /// after it starts over, whatever came before.
    #[test]
    fn should_start_over_after_a_successful_scrape() {
        let now = OffsetDateTime::now_utc();

        let after_success = listed_candidate(0, 0, now);

        assert!(page_directive_withdrawal(&after_success, &refusal(), now).is_none());
    }

    #[test]
    fn should_withdraw_only_a_captured_listing_refused_at_its_own_url() {
        let now = OffsetDateTime::now_utc();
        let mut never_captured = listed_candidate(3, 72, now);
        never_captured.last_captured_raw_input_sha256 = None;
        let other_url = ScraperError::HttpError {
            url: url::Url::parse("https://shop.example/product/other").unwrap(),
            kind: NetworkErrorKind::PageDirectiveDisallowed,
            details: "noindex".to_owned(),
        };
        let robots_txt = ScraperError::HttpError {
            url: url(),
            kind: NetworkErrorKind::RobotsDisallowed,
            details: "robots.txt disallows".to_owned(),
        };

        assert!(page_directive_withdrawal(&never_captured, &refusal(), now).is_none());
        for error in [other_url, robots_txt] {
            assert!(
                page_directive_withdrawal(&listed_candidate(3, 72, now), &error, now).is_none(),
                "{error}"
            );
        }
    }
}
