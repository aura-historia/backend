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
    Pattern { value: String },
    NoPattern,
}

impl UrlPatternReviewCandidate {
    pub fn pattern(pattern: Option<&Regex>, current_pattern: Option<&Regex>) -> Self {
        Self {
            decision: pattern.map_or(UrlPatternDecision::NoPattern, |pattern| {
                UrlPatternDecision::Pattern {
                    value: pattern.as_str().to_owned(),
                }
            }),
            current_pattern: current_pattern.map(|pattern| pattern.as_str().to_owned()),
        }
    }

    pub fn validated_pattern(&self) -> Result<Option<&str>, regex::Error> {
        match &self.decision {
            UrlPatternDecision::Pattern { value } => {
                Regex::new(value)?;
                Ok(Some(value))
            }
            UrlPatternDecision::NoPattern => Ok(None),
        }
    }
}
