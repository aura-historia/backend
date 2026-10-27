use super::SampleLabels;
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UrlPatternReviewCandidate {
    pub decision: UrlPatternDecision,
    pub current_pattern: Option<String>,
    /// Which URLs of the review's sample, or of its ListingSource's domains, are product pages, as
    /// the author agent or a human labelled them; set only through the review's sample labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_labels: Option<SampleLabels>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UrlPatternDecision {
    Pattern {
        value: String,
    },
    NoPattern,
    /// No pattern proposed yet: an operator must enter one before the review can be approved.
    Undecided,
}

impl UrlPatternDecision {
    /// The decision for an inferred pattern, where `None` means no product pattern exists.
    pub fn inferred(pattern: Option<&Regex>) -> Self {
        pattern.map_or(Self::NoPattern, |pattern| Self::Pattern {
            value: pattern.as_str().to_owned(),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UrlPatternCandidateError {
    #[error("URL pattern review has no decision yet")]
    Undecided,
    #[error("invalid URL pattern: {0}")]
    InvalidRegex(#[from] regex::Error),
    #[error("a URL-pattern candidate is a JSON object")]
    NotAnObject,
    #[error("the sample labels of a URL-pattern candidate do not serialize")]
    Payload(#[source] serde_json::Error),
}

impl UrlPatternReviewCandidate {
    pub fn new(decision: UrlPatternDecision, current_pattern: Option<&Regex>) -> Self {
        Self {
            decision,
            current_pattern: current_pattern.map(|pattern| pattern.as_str().to_owned()),
            sample_labels: None,
        }
    }

    /// The stored or posted candidate `payload` with `labels` as its sample labels, and without
    /// any when `labels` is `None`; its other keys stay as they are. A payload that is no JSON
    /// object holds no candidate.
    pub fn payload_with_sample_labels(
        mut payload: serde_json::Value,
        labels: Option<&SampleLabels>,
    ) -> Result<serde_json::Value, UrlPatternCandidateError> {
        let candidate = payload
            .as_object_mut()
            .ok_or(UrlPatternCandidateError::NotAnObject)?;
        match labels {
            Some(labels) => {
                candidate.insert(
                    SAMPLE_LABELS_KEY.to_owned(),
                    serde_json::to_value(labels).map_err(UrlPatternCandidateError::Payload)?,
                );
            }
            None => {
                candidate.remove(SAMPLE_LABELS_KEY);
            }
        }
        Ok(payload)
    }

    /// The pattern approval would store: `Some` regex, or `None` for a decided `NO_PATTERN`.
    pub fn validated_pattern(&self) -> Result<Option<&str>, UrlPatternCandidateError> {
        match &self.decision {
            UrlPatternDecision::Pattern { value } => {
                Regex::new(value)?;
                Ok(Some(value))
            }
            UrlPatternDecision::NoPattern => Ok(None),
            UrlPatternDecision::Undecided => Err(UrlPatternCandidateError::Undecided),
        }
    }
}

/// The key of a URL-pattern candidate that holds its sample labels.
const SAMPLE_LABELS_KEY: &str = "sample_labels";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_undecided_candidate() {
        let candidate = UrlPatternReviewCandidate::new(UrlPatternDecision::Undecided, None);

        assert!(matches!(
            candidate.validated_pattern(),
            Err(UrlPatternCandidateError::Undecided)
        ));
    }

    #[test]
    fn should_persist_undecided_decision_as_screaming_snake_case() {
        let candidate = UrlPatternReviewCandidate::new(UrlPatternDecision::Undecided, None);

        let payload = serde_json::to_value(&candidate).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            payload["decision"],
            serde_json::json!({ "type": "UNDECIDED" })
        );
    }

    #[test]
    fn should_accept_valid_pattern_and_no_pattern() {
        let pattern = UrlPatternReviewCandidate::new(
            UrlPatternDecision::Pattern {
                value: "/product/".to_owned(),
            },
            None,
        );
        let no_pattern = UrlPatternReviewCandidate::new(UrlPatternDecision::NoPattern, None);

        assert_eq!(pattern.validated_pattern().ok(), Some(Some("/product/")));
        assert_eq!(no_pattern.validated_pattern().ok(), Some(None));
    }

    /// Sample labels go in and out of a candidate's payload without touching its other keys,
    /// and a candidate without labels reads and stores as before they existed.
    #[test]
    fn should_set_and_drop_sample_labels_keeping_the_other_keys_of_the_payload() {
        let labels = SampleLabels {
            labelled_at: time::OffsetDateTime::UNIX_EPOCH,
            labels: Vec::new(),
        };
        let stored = serde_json::json!({
            "decision": { "type": "PATTERN", "value": "/item/" },
            "current_pattern": null,
            "note": "kept",
        });

        let labelled =
            UrlPatternReviewCandidate::payload_with_sample_labels(stored.clone(), Some(&labels))
                .unwrap_or_else(|error| panic!("{error}"));
        let unlabelled =
            UrlPatternReviewCandidate::payload_with_sample_labels(labelled.clone(), None)
                .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(labelled["note"], "kept");
        assert_eq!(
            serde_json::from_value::<UrlPatternReviewCandidate>(labelled)
                .ok()
                .and_then(|candidate| candidate.sample_labels),
            Some(labels)
        );
        assert_eq!(unlabelled, stored);
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<UrlPatternReviewCandidate>(stored.clone())
                    .unwrap_or_else(|error| panic!("{error}"))
            )
            .ok()
            .map(|payload| payload.get("sample_labels").is_none()),
            Some(true)
        );
        assert!(matches!(
            UrlPatternReviewCandidate::payload_with_sample_labels(serde_json::json!([]), None),
            Err(UrlPatternCandidateError::NotAnObject)
        ));
    }
}
