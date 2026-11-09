//! The one DEBUG event every scraped URL logs, `crawler.scraper.url_outcome`: what happened to the
//! URL and the evidence behind it, so an operator can answer "why wasn't URL X updated" from DEBUG
//! logs.

use crate::logging::SourceLogContext;
use crate::scraper::candidate_service::ScraperCandidate;
use crate::scraper::scraper_service::ScrapeTelemetry;
use crate::scraper::scraper_service::domain::errors::ListingRemoval;

pub(crate) const URL_OUTCOME_EVENT: &str = "crawler.scraper.url_outcome";

/// Why a scraped URL was left as it was without a capture or a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SkipReason {
    /// The ListingSource spent its LLM call budget earlier in the batch.
    LlmBudgetExhausted,
    /// Another writer changed the URL after it was selected, so the write was dropped.
    StaleWrite,
    /// The URL is another spelling of a product the ListingSource already stores.
    DuplicateUrl,
    /// The URL ended at a redirect no stored row settles.
    UnsettledRedirect,
    /// The mass-removal guard held the URL's removal back.
    MassRemovalHeld,
}

impl SkipReason {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::LlmBudgetExhausted => "llm_budget_exhausted",
            Self::StaleWrite => "stale_write",
            Self::DuplicateUrl => "duplicate_url",
            Self::UnsettledRedirect => "unsettled_redirect",
            Self::MassRemovalHeld => "mass_removal_held",
        }
    }
}

/// What a scraped URL's scrape came to. `Captured` means the observation was handed to raw
/// capture; its durable result is counted by the raw-capture batch summary. Whether a captured
/// product is sold is the worker's normalization to decide and is not known here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum UrlOutcome {
    Captured,
    /// The page or its raw input matches the URL's last capture.
    Unchanged,
    /// A verified removal; `evidence` is the kind of evidence that showed it.
    Removed {
        evidence: &'static str,
        /// The HTTP status of the answer that showed it, when it was one.
        status: Option<u16>,
        final_url: Option<String>,
        /// The removal's explanation.
        details: String,
    },
    /// The page waits on a pending product-schema review.
    AwaitingSchemaReview,
    Skipped(SkipReason),
    /// `kind` is the stable failure kind the URL records.
    Failed {
        kind: String,
    },
}

impl UrlOutcome {
    pub(super) fn removed(removal: &ListingRemoval) -> Self {
        Self::Removed {
            evidence: removal.evidence.kind.as_str(),
            status: removal.evidence.status,
            final_url: removal
                .evidence
                .final_url
                .as_ref()
                .map(url::Url::as_str)
                .map(str::to_owned),
            details: removal.details.clone(),
        }
    }

    pub(super) fn failed(kind: impl Into<String>) -> Self {
        Self::Failed { kind: kind.into() }
    }

    /// The stable label of the `outcome` field: `captured`, `unchanged`, `removed`,
    /// `awaiting_schema_review`, `skipped:<reason>` or `failed:<kind>`.
    pub(super) fn label(&self) -> String {
        match self {
            Self::Captured => "captured".to_owned(),
            Self::Unchanged => "unchanged".to_owned(),
            Self::Removed { .. } => "removed".to_owned(),
            Self::AwaitingSchemaReview => "awaiting_schema_review".to_owned(),
            Self::Skipped(reason) => format!("skipped:{}", reason.as_str()),
            Self::Failed { kind } => format!("failed:{kind}"),
        }
    }

    /// The reason a URL counted as skipped is reported under in the pass summary.
    pub(super) fn skip_label(&self) -> &'static str {
        match self {
            Self::Skipped(reason) => reason.as_str(),
            Self::Unchanged => "unchanged_page",
            _ => "other",
        }
    }

    fn removal_evidence(&self) -> Option<&'static str> {
        match self {
            Self::Removed { evidence, .. } => Some(evidence),
            _ => None,
        }
    }

    /// The removal's HTTP status, final URL and explanation, which `removal_evidence` names the
    /// kind of.
    fn removal_facts(&self) -> (Option<u16>, Option<&str>, Option<&str>) {
        match self {
            Self::Removed {
                status,
                final_url,
                details,
                ..
            } => (*status, final_url.as_deref(), Some(details)),
            _ => (None, None, None),
        }
    }
}

/// Logs the outcome of one scraped URL at DEBUG: it repeats for every URL of every batch.
pub(super) fn log_url_outcome(
    source: &SourceLogContext,
    candidate: &ScraperCandidate,
    outcome: &UrlOutcome,
    telemetry: &ScrapeTelemetry,
) {
    let (removal_status, removal_final_url, removal_details) = outcome.removal_facts();
    macro_rules! log_outcome {
        ($level:expr) => {
            tracing::event!(
                $level,
                event = URL_OUTCOME_EVENT,
                listing_source_id = %source.listing_source_id,
                listing_source_name = %source.listing_source_name,
                domain = %source.domain,
                url = %candidate.url,
                outcome = %outcome.label(),
                removal_evidence = outcome.removal_evidence(),
                removal_status,
                removal_final_url,
                removal_details,
                http_status = telemetry.http_status,
                fetch_ms = telemetry.fetch_ms,
                process_ms = telemetry.process_ms,
                schema_index = telemetry.schema_index,
                schema_score = telemetry.schema_score,
                fresh_schema = telemetry.fresh_schema,
                fields_present = telemetry.fields_present,
                image_probes = telemetry.image_probes,
                llm_calls = telemetry.llm_calls,
                "Scraped URL"
            )
        };
    }
    log_outcome!(tracing::Level::DEBUG);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_log_the_outcome_with_source_and_scrape_telemetry_when_a_url_was_scraped() {
        let (events, _guard) = crate::test_support::logged_events::LoggedEvents::capture();
        let candidate = crate::service::cron::test_support::scraper_candidate(
            "Antique Shop",
            url::Url::parse("https://antique.example/products/1").unwrap(),
        );
        let source = SourceLogContext::new(
            candidate.listing_source_id,
            "Antique Shop",
            "antique.example",
        );
        let telemetry = ScrapeTelemetry {
            http_status: Some(200),
            fetch_ms: Some(120),
            process_ms: Some(15),
            schema_index: Some(1),
            schema_score: Some(7),
            fields_present: Some(6),
            llm_calls: 0,
            ..Default::default()
        };

        log_url_outcome(&source, &candidate, &UrlOutcome::Captured, &telemetry);

        let logged = events.named(URL_OUTCOME_EVENT);
        assert_eq!(logged.len(), 1);
        let logged = &logged[0];
        assert_eq!(logged.level, tracing::Level::DEBUG);
        for (field, expected) in [
            ("listing_source_name", "Antique Shop"),
            ("domain", "antique.example"),
            ("url", "https://antique.example/products/1"),
            ("outcome", "captured"),
            ("http_status", "200"),
            ("fetch_ms", "120"),
            ("process_ms", "15"),
            ("schema_index", "1"),
            ("schema_score", "7"),
            ("fields_present", "6"),
            ("llm_calls", "0"),
        ] {
            assert_eq!(logged.field(field), Some(expected), "{field}");
        }
    }

    #[test]
    fn should_name_the_removal_evidence_when_a_url_was_removed() {
        let (events, _guard) = crate::test_support::logged_events::LoggedEvents::capture();
        let candidate = crate::service::cron::test_support::scraper_candidate(
            "Antique Shop",
            url::Url::parse("https://antique.example/products/2").unwrap(),
        );
        let source = SourceLogContext::new(
            candidate.listing_source_id,
            "Antique Shop",
            "antique.example",
        );

        log_url_outcome(
            &source,
            &candidate,
            &UrlOutcome::Removed {
                evidence: "gone_status",
                status: Some(410),
                final_url: Some("https://antique.example/gone".to_owned()),
                details: "the shop answered 410".to_owned(),
            },
            &ScrapeTelemetry::default(),
        );

        let logged = events.named(URL_OUTCOME_EVENT);
        assert_eq!(logged[0].field("outcome"), Some("removed"));
        assert_eq!(logged[0].field("removal_evidence"), Some("gone_status"));
        assert_eq!(logged[0].field("removal_status"), Some("410"));
        assert_eq!(
            logged[0].field("removal_details"),
            Some("the shop answered 410")
        );
    }

    #[test]
    fn should_log_every_outcome_at_debug() {
        let (events, _guard) = crate::test_support::logged_events::LoggedEvents::capture();
        let candidate = crate::service::cron::test_support::scraper_candidate(
            "Antique Shop",
            url::Url::parse("https://antique.example/products/3").unwrap(),
        );
        let source = SourceLogContext::new(
            candidate.listing_source_id,
            "Antique Shop",
            "antique.example",
        );
        let outcomes = [
            UrlOutcome::Unchanged,
            UrlOutcome::Captured,
            UrlOutcome::AwaitingSchemaReview,
            UrlOutcome::Skipped(SkipReason::DuplicateUrl),
            UrlOutcome::failed("ConnectError"),
        ];

        for outcome in &outcomes {
            log_url_outcome(&source, &candidate, outcome, &ScrapeTelemetry::default());
        }

        let levels: Vec<_> = events
            .named(URL_OUTCOME_EVENT)
            .iter()
            .map(|logged| logged.level)
            .collect();
        assert_eq!(
            levels,
            [tracing::Level::DEBUG; 5]
        );
    }

    #[test]
    fn should_label_every_outcome_with_its_stable_name() {
        let labels = [
            (UrlOutcome::Captured, "captured"),
            (UrlOutcome::Unchanged, "unchanged"),
            (
                UrlOutcome::Removed {
                    evidence: "gone_status",
                    status: Some(410),
                    final_url: None,
                    details: String::new(),
                },
                "removed",
            ),
            (UrlOutcome::AwaitingSchemaReview, "awaiting_schema_review"),
            (
                UrlOutcome::Skipped(SkipReason::StaleWrite),
                "skipped:stale_write",
            ),
            (UrlOutcome::failed("HttpError"), "failed:HttpError"),
        ];

        for (outcome, label) in labels {
            assert_eq!(outcome.label(), label);
        }
    }
}
