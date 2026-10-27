use super::LiveMatrixEvaluation;
use crate::CrawlerReviewId;
use serde::{Deserialize, Serialize};

/// The live evaluation of a URL-pattern review's URL sample: the sample URLs the server fetched at
/// the shop's pacing for schema.org `Product` markup, chosen for the review's decision at a
/// candidate version, and what each page showed. A review stores it next to its candidate; a
/// candidate change drops it. The contract is in `docs/crawler/extraction-and-review.md`
/// (approval guard).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UrlSampleEvaluation {
    pub review_id: CrawlerReviewId,
    /// The candidate version and the fingerprint of the decision the URLs were chosen for, and
    /// when the first URL was fetched.
    pub live_evaluation: LiveMatrixEvaluation,
    pub urls: Vec<EvaluatedSampleUrl>,
}

impl UrlSampleEvaluation {
    /// The key of a review's validation summary that stores it.
    pub const STORED_KEY: &'static str = "url_sample_evaluation";

    /// The evaluation a review's validation summary stores, `None` when it stores none, or the
    /// decoding error of one that no longer decodes.
    pub fn stored_in(
        validation_summary: &serde_json::Value,
    ) -> Option<Result<Self, serde_json::Error>> {
        let stored = validation_summary.get(Self::STORED_KEY)?;
        Some(serde_json::from_value(stored.clone()))
    }

    /// The sample URLs whose page showed one schema.org `Product`.
    pub fn product_markup_urls(&self) -> impl Iterator<Item = &str> {
        self.urls
            .iter()
            .filter(|url| url.outcome == SampleUrlOutcome::ProductMarkup)
            .map(|url| url.url.as_str())
    }
}

/// A sample URL a live evaluation fetched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluatedSampleUrl {
    pub url: String,
    /// Whether the evaluated decision classifies the URL as a product page.
    pub decision_matches: bool,
    #[serde(flatten)]
    pub outcome: SampleUrlOutcome,
}

/// What a fetched sample URL's page showed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SampleUrlOutcome {
    /// The page describes one schema.org `Product`: the URL is a product page.
    ProductMarkup,
    /// The page describes no product of its own, which shows nothing either way.
    NoProductMarkup,
    /// The URL redirected to another page, whose markup shows nothing about the URL.
    Redirected { final_url: String },
    /// The page could not be fetched, which shows nothing either way.
    FetchFailed { error: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::OffsetDateTime;

    #[test]
    fn should_store_each_outcome_by_its_name_next_to_the_url() {
        let review_id = CrawlerReviewId::new();
        let evaluation = UrlSampleEvaluation {
            review_id,
            live_evaluation: LiveMatrixEvaluation {
                candidate_version: 2,
                configuration_sha256: "decision-sha256".to_owned(),
                fetched_at: OffsetDateTime::UNIX_EPOCH,
            },
            urls: vec![
                EvaluatedSampleUrl {
                    url: "https://shop.example/item/1".to_owned(),
                    decision_matches: true,
                    outcome: SampleUrlOutcome::ProductMarkup,
                },
                EvaluatedSampleUrl {
                    url: "https://shop.example/item/2".to_owned(),
                    decision_matches: true,
                    outcome: SampleUrlOutcome::Redirected {
                        final_url: "https://shop.example/".to_owned(),
                    },
                },
            ],
        };
        let stored = json!({ "sample_count": 2, "url_sample_evaluation": evaluation });

        assert_eq!(
            stored["url_sample_evaluation"]["urls"],
            json!([
                { "url": "https://shop.example/item/1", "decision_matches": true, "outcome": "PRODUCT_MARKUP" },
                {
                    "url": "https://shop.example/item/2",
                    "decision_matches": true,
                    "outcome": "REDIRECTED",
                    "final_url": "https://shop.example/",
                },
            ])
        );
        let read = UrlSampleEvaluation::stored_in(&stored).and_then(Result::ok);
        assert_eq!(read.as_ref(), Some(&evaluation));
        assert_eq!(
            evaluation.product_markup_urls().collect::<Vec<_>>(),
            vec!["https://shop.example/item/1"]
        );
        assert!(UrlSampleEvaluation::stored_in(&json!({ "sample_count": 2 })).is_none());
    }
}
