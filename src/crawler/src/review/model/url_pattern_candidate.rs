use super::SampleLabels;
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UrlPatternReviewCandidate {
    pub decision: UrlPatternDecision,
    pub current_pattern: Option<String>,
    /// Which URLs of the review's domain, its sample's among them, are product pages, as the
    /// author agent or a human labelled them; set only through the review's sample labels.
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
}

impl UrlPatternReviewCandidate {
    pub fn new(decision: UrlPatternDecision, current_pattern: Option<&Regex>) -> Self {
        Self {
            decision,
            current_pattern: current_pattern.map(|pattern| pattern.as_str().to_owned()),
            sample_labels: None,
        }
    }

    /// Decodes a candidate a reviewer posts as an edit, refusing a key the candidate does not
    /// have, so the stored candidate holds only what it is decoded from.
    pub fn decode_edit(payload: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value::<PostedUrlPatternReviewCandidate>(payload).map(Self::from)
    }

    /// This candidate with `labels` as its sample labels, and without any when `labels` is `None`.
    pub fn with_sample_labels(self, labels: Option<SampleLabels>) -> Self {
        Self {
            sample_labels: labels,
            ..self
        }
    }

    /// Why the stored candidate `payload` is invalid: it does not decode, or it proposes a pattern
    /// that is no regex. An undecided candidate is valid, it only waits for a decision. `None`
    /// once it is valid.
    pub fn validation_failure(payload: &serde_json::Value) -> Option<String> {
        match serde_json::from_value::<Self>(payload.clone()) {
            Err(error) => Some(format!("the candidate does not decode: {error}")),
            Ok(candidate) => match candidate.validated_pattern() {
                Err(error @ UrlPatternCandidateError::InvalidRegex(_)) => Some(error.to_string()),
                _ => None,
            },
        }
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

/// A [`UrlPatternReviewCandidate`] as an edit posts it, with exactly the candidate's keys.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PostedUrlPatternReviewCandidate {
    decision: PostedUrlPatternDecision,
    current_pattern: Option<String>,
    #[serde(default)]
    sample_labels: Option<SampleLabels>,
}

impl From<PostedUrlPatternReviewCandidate> for UrlPatternReviewCandidate {
    fn from(posted: PostedUrlPatternReviewCandidate) -> Self {
        let PostedUrlPatternReviewCandidate {
            decision,
            current_pattern,
            sample_labels,
        } = posted;
        Self {
            decision: decision.into(),
            current_pattern,
            sample_labels,
        }
    }
}

/// A [`UrlPatternDecision`] as an edit posts it, with exactly the decision's keys. Its variants
/// are struct variants because serde ignores extra keys of an internally tagged unit variant
/// despite `deny_unknown_fields`.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum PostedUrlPatternDecision {
    Pattern { value: String },
    NoPattern {},
    Undecided {},
}

impl From<PostedUrlPatternDecision> for UrlPatternDecision {
    fn from(posted: PostedUrlPatternDecision) -> Self {
        match posted {
            PostedUrlPatternDecision::Pattern { value } => Self::Pattern { value },
            PostedUrlPatternDecision::NoPattern {} => Self::NoPattern,
            PostedUrlPatternDecision::Undecided {} => Self::Undecided,
        }
    }
}

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

    /// A posted candidate is decoded with exactly the candidate's keys, so an unknown key is
    /// refused rather than stored, and one without labels reads as before they existed.
    #[test]
    fn should_decode_a_posted_candidate_and_refuse_unknown_keys() {
        let posted = serde_json::json!({
            "decision": { "type": "PATTERN", "value": "/item/" },
            "current_pattern": null,
        });
        let mut with_unknown_key = posted.clone();
        with_unknown_key["note"] = serde_json::json!("kept");

        let decoded = UrlPatternReviewCandidate::decode_edit(posted.clone())
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(serde_json::to_value(&decoded).ok(), Some(posted));
        assert!(UrlPatternReviewCandidate::decode_edit(with_unknown_key).is_err());
        for decision in [
            serde_json::json!({ "type": "PATTERN", "value": "/item/" }),
            serde_json::json!({ "type": "NO_PATTERN" }),
            serde_json::json!({ "type": "UNDECIDED" }),
        ] {
            let candidate =
                serde_json::json!({ "decision": decision.clone(), "current_pattern": null });
            let mut with_unknown_decision_key = candidate.clone();
            with_unknown_decision_key["decision"]["note"] = serde_json::json!("kept");

            assert_eq!(
                UrlPatternReviewCandidate::decode_edit(candidate.clone())
                    .ok()
                    .and_then(|decoded| serde_json::to_value(decoded).ok()),
                Some(candidate),
                "{decision}"
            );
            assert!(
                UrlPatternReviewCandidate::decode_edit(with_unknown_decision_key).is_err(),
                "{decision}"
            );
        }
    }

    #[test]
    fn should_set_and_drop_sample_labels() {
        let labels = SampleLabels {
            labelled_at: time::OffsetDateTime::UNIX_EPOCH,
            labels: Vec::new(),
        };
        let candidate = UrlPatternReviewCandidate::new(UrlPatternDecision::NoPattern, None);

        let labelled = candidate.clone().with_sample_labels(Some(labels.clone()));

        assert_eq!(labelled.sample_labels.as_ref(), Some(&labels));
        assert_eq!(labelled.with_sample_labels(None), candidate);
    }

    /// A stored candidate that does not decode or proposes no regex is invalid; an undecided one
    /// only waits for a decision.
    #[test]
    fn should_name_why_a_stored_candidate_is_invalid() {
        let stored =
            |decision| serde_json::json!({ "decision": decision, "current_pattern": null });

        assert!(
            UrlPatternReviewCandidate::validation_failure(&stored(
                serde_json::json!({ "type": "UNDECIDED" })
            ))
            .is_none()
        );
        assert!(
            UrlPatternReviewCandidate::validation_failure(&stored(
                serde_json::json!({ "type": "PATTERN", "value": "/item/" })
            ))
            .is_none()
        );
        assert!(
            UrlPatternReviewCandidate::validation_failure(&stored(
                serde_json::json!({ "type": "PATTERN", "value": "(" })
            ))
            .is_some_and(|failure| failure.starts_with("invalid URL pattern"))
        );
        assert!(
            UrlPatternReviewCandidate::validation_failure(&serde_json::json!([]))
                .is_some_and(|failure| failure.starts_with("the candidate does not decode"))
        );
    }
}
