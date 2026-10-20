use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UrlPatternReviewCandidate {
    pub decision: UrlPatternDecision,
    pub current_pattern: Option<String>,
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
}
