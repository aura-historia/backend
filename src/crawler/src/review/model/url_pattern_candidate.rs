use super::SampleLabels;
use crate::spider::classification::product_url_pattern::{
    ProductUrlPattern, ProductUrlPatternError, UrlPatternMatching,
};
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UrlPatternReviewCandidate {
    pub decision: UrlPatternDecision,
    pub current_pattern: Option<String>,
    /// The exclusion of the live pattern, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_exclude: Option<String>,
    /// What the live pattern is matched against; left out for `FULL_URL_SEARCH`, which every
    /// candidate stored before the matching existed has.
    #[serde(
        default,
        skip_serializing_if = "UrlPatternMatching::is_full_url_search"
    )]
    pub current_matching: UrlPatternMatching,
    /// Which URLs on the crawl host of the review's ListingSource, its sample's among them, are
    /// product pages, as the author agent or a human labelled them; set only through the review's
    /// sample labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_labels: Option<SampleLabels>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UrlPatternDecision {
    /// A URL on the crawl host is a product URL when it matches `value` and does not match
    /// `exclude`, which takes the non-items that share the product URL shape, both matched as
    /// `matching` says.
    Pattern {
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exclude: Option<String>,
        /// Left out for `FULL_URL_SEARCH`, which every decision stored before the matching
        /// existed has.
        #[serde(
            default,
            skip_serializing_if = "UrlPatternMatching::is_full_url_search"
        )]
        matching: UrlPatternMatching,
    },
    NoPattern,
    /// No pattern proposed yet: an operator must enter one before the review can be approved.
    Undecided,
}

impl UrlPatternDecision {
    /// The pattern of a `PATTERN` decision, `None` for a decision without a pattern.
    pub fn candidate_pattern(&self) -> Result<Option<ProductUrlPattern>, ProductUrlPatternError> {
        match self {
            Self::Pattern {
                value,
                exclude,
                matching,
            } => ProductUrlPattern::parse_matching(value, *matching)
                .and_then(|pattern| pattern.with_exclude(exclude.as_deref()))
                .map(Some),
            Self::NoPattern | Self::Undecided => Ok(None),
        }
    }

    /// The decision for an inferred pattern, matched anywhere in the URL as the inference writes
    /// it, where `None` means no product pattern exists.
    pub fn inferred(pattern: Option<&Regex>) -> Self {
        pattern.map_or(Self::NoPattern, |pattern| Self::Pattern {
            value: pattern.as_str().to_owned(),
            exclude: None,
            matching: UrlPatternMatching::FullUrlSearch,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UrlPatternCandidateError {
    #[error("URL pattern review has no decision yet")]
    Undecided,
    #[error(transparent)]
    InvalidPattern(#[from] ProductUrlPatternError),
}

impl UrlPatternReviewCandidate {
    pub fn new(decision: UrlPatternDecision, current_pattern: Option<&ProductUrlPattern>) -> Self {
        Self {
            decision,
            current_pattern: current_pattern.map(|pattern| pattern.include_source().to_owned()),
            current_exclude: current_pattern
                .and_then(|pattern| pattern.exclude_source())
                .map(str::to_owned),
            current_matching: current_pattern
                .map(ProductUrlPattern::matching)
                .unwrap_or_default(),
            sample_labels: None,
        }
    }

    /// The ListingSource's live pattern when the review opened, as the candidate records it.
    pub fn current(&self) -> Result<Option<ProductUrlPattern>, ProductUrlPatternError> {
        ProductUrlPattern::from_stored(
            self.current_pattern.as_deref(),
            self.current_exclude.as_deref(),
            Some(self.current_matching),
        )
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
    /// that is no regex, or an exclusion that is blank or no regex. An undecided candidate is valid, it only waits for a decision. `None`
    /// once it is valid.
    pub fn validation_failure(payload: &serde_json::Value) -> Option<String> {
        match serde_json::from_value::<Self>(payload.clone()) {
            Err(error) => Some(format!("the candidate does not decode: {error}")),
            Ok(candidate) => match candidate.validated_pattern() {
                Err(error @ UrlPatternCandidateError::InvalidPattern(_)) => Some(error.to_string()),
                _ => None,
            },
        }
    }

    /// The pattern approval would store: `Some` pattern, or `None` for a decided `NO_PATTERN`.
    pub fn validated_pattern(&self) -> Result<Option<ProductUrlPattern>, UrlPatternCandidateError> {
        match &self.decision {
            UrlPatternDecision::Undecided => Err(UrlPatternCandidateError::Undecided),
            decision => Ok(decision.candidate_pattern()?),
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
    current_exclude: Option<String>,
    #[serde(default)]
    current_matching: UrlPatternMatching,
    #[serde(default)]
    sample_labels: Option<SampleLabels>,
}

impl From<PostedUrlPatternReviewCandidate> for UrlPatternReviewCandidate {
    fn from(posted: PostedUrlPatternReviewCandidate) -> Self {
        let PostedUrlPatternReviewCandidate {
            decision,
            current_pattern,
            current_exclude,
            current_matching,
            sample_labels,
        } = posted;
        Self {
            decision: decision.into(),
            current_pattern,
            current_exclude,
            current_matching,
            sample_labels,
        }
    }
}

/// A [`UrlPatternDecision`] as an edit posts it, with exactly the decision's keys. Its variants
/// are struct variants because serde ignores extra keys of an internally tagged unit variant
/// despite `deny_unknown_fields`. A posted pattern without a `matching` is matched anywhere in
/// the URL, as before the matching existed.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum PostedUrlPatternDecision {
    Pattern {
        value: String,
        #[serde(default)]
        exclude: Option<String>,
        #[serde(default)]
        matching: UrlPatternMatching,
    },
    NoPattern {},
    Undecided {},
}

impl From<PostedUrlPatternDecision> for UrlPatternDecision {
    fn from(posted: PostedUrlPatternDecision) -> Self {
        match posted {
            PostedUrlPatternDecision::Pattern {
                value,
                exclude,
                matching,
            } => Self::Pattern {
                value,
                exclude,
                matching,
            },
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
                exclude: None,
                matching: UrlPatternMatching::FullUrlSearch,
            },
            None,
        );
        let no_pattern = UrlPatternReviewCandidate::new(UrlPatternDecision::NoPattern, None);

        assert_eq!(
            pattern
                .validated_pattern()
                .ok()
                .flatten()
                .map(|pattern| pattern.include_source().to_owned()),
            Some("/product/".to_owned())
        );
        assert!(matches!(no_pattern.validated_pattern(), Ok(None)));
    }

    /// A stored candidate without an exclusion, every candidate stored before the exclusion
    /// existed, decodes and stores unchanged; one with an exclusion keeps it through both decoders.
    #[test]
    fn should_keep_an_absent_exclusion_absent_and_a_present_one_through_serde() {
        let without = serde_json::json!({
            "decision": { "type": "PATTERN", "value": "/products/" },
            "current_pattern": "/p/",
        });
        let with = serde_json::json!({
            "decision": { "type": "PATTERN", "value": "/products/", "exclude": "gift-card" },
            "current_pattern": "/p/",
            "current_exclude": "voucher",
        });

        for payload in [without, with] {
            let stored: UrlPatternReviewCandidate = serde_json::from_value(payload.clone())
                .unwrap_or_else(|error| panic!("{payload}: {error}"));
            let posted = UrlPatternReviewCandidate::decode_edit(payload.clone())
                .unwrap_or_else(|error| panic!("{payload}: {error}"));

            assert_eq!(serde_json::to_value(&stored).ok(), Some(payload.clone()));
            assert_eq!(stored, posted, "{payload}");
        }
    }

    #[test]
    fn should_validate_the_exclusion_as_a_regex_that_is_not_blank() {
        let candidate = |exclude: &str| {
            UrlPatternReviewCandidate::new(
                UrlPatternDecision::Pattern {
                    value: "/products/".to_owned(),
                    exclude: Some(exclude.to_owned()),
                    matching: UrlPatternMatching::FullUrlSearch,
                },
                None,
            )
        };

        let valid = candidate("gift-card|voucher")
            .validated_pattern()
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            valid.and_then(|pattern| pattern.exclude_source().map(str::to_owned)),
            Some("gift-card|voucher".to_owned())
        );
        for invalid in ["(", "", "  "] {
            let failure = UrlPatternReviewCandidate::validation_failure(
                &serde_json::to_value(candidate(invalid)).unwrap(),
            );
            assert!(
                failure.is_some_and(|failure| failure.starts_with("invalid URL pattern")
                    || failure.contains("exclusion is blank")),
                "{invalid:?}"
            );
            assert!(
                candidate(invalid).validated_pattern().is_err(),
                "{invalid:?}"
            );
        }
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

    /// A decision or a live pattern without a `matching`, every one stored before it existed, is
    /// matched anywhere in the URL; a path-and-query one keeps its matching through both decoders
    /// and refuses a value that names a scheme or host.
    #[test]
    fn should_default_the_matching_to_a_full_url_search_and_keep_a_path_and_query_one() {
        let path_and_query = serde_json::json!({
            "decision": {
                "type": "PATTERN",
                "value": "/item/[0-9]+",
                "matching": "PATH_AND_QUERY_FULL_MATCH",
            },
            "current_pattern": "/p/",
            "current_matching": "PATH_AND_QUERY_FULL_MATCH",
        });
        let old = serde_json::json!({
            "decision": { "type": "PATTERN", "value": "/item/" },
            "current_pattern": "/p/",
        });

        let stored: UrlPatternReviewCandidate = serde_json::from_value(path_and_query.clone())
            .unwrap_or_else(|error| panic!("{error}"));
        let posted = UrlPatternReviewCandidate::decode_edit(path_and_query.clone())
            .unwrap_or_else(|error| panic!("{error}"));
        let old =
            UrlPatternReviewCandidate::decode_edit(old).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(stored, posted);
        assert_eq!(serde_json::to_value(&stored).ok(), Some(path_and_query));
        assert_eq!(
            stored
                .validated_pattern()
                .ok()
                .flatten()
                .map(|pattern| pattern.matching()),
            Some(UrlPatternMatching::PathAndQueryFullMatch)
        );
        assert_eq!(
            stored
                .current()
                .ok()
                .flatten()
                .map(|pattern| pattern.matching()),
            Some(UrlPatternMatching::PathAndQueryFullMatch)
        );
        assert_eq!(
            old.validated_pattern()
                .ok()
                .flatten()
                .map(|pattern| pattern.matching()),
            Some(UrlPatternMatching::FullUrlSearch)
        );
        assert_eq!(old.current_matching, UrlPatternMatching::FullUrlSearch);
        let naming_host = serde_json::json!({
            "decision": {
                "type": "PATTERN",
                "value": "https://shop\\.example/item/.+",
                "matching": "PATH_AND_QUERY_FULL_MATCH",
            },
            "current_pattern": null,
        });
        assert!(
            UrlPatternReviewCandidate::validation_failure(&naming_host)
                .is_some_and(|failure| failure.contains("must not name a scheme or host"))
        );
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
