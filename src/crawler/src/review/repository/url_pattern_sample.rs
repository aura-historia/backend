//! The sample of a URL-pattern review: its URLs as the review's URL rows store them, classified
//! under the domain's current pattern and under the candidate's, and how many of them the
//! candidate takes as product pages. The contract is in `docs/crawler/extraction-and-review.md`
//! (review rail).

use crate::spider::classification::url_classifier::UrlClassifier;
use crate::spider::utils::url::CrawledUrl;
use regex::Regex;

/// How one sample URL classifies under the candidate's pattern.
pub(super) struct CandidateClassification {
    /// Whether the candidate's pattern takes the URL; `None` when the candidate has no pattern.
    pub(super) pattern_match: Option<bool>,
    /// The URL's class as the spider of the authoring mode classifies it under that pattern.
    pub(super) class: String,
}

impl CandidateClassification {
    pub(super) fn of(
        url_classifier: UrlClassifier,
        raw_url: &str,
        candidate_pattern: Option<&Regex>,
    ) -> Self {
        Self {
            pattern_match: candidate_pattern.map(|pattern| pattern_takes(pattern, raw_url)),
            class: url_classifier
                .classify(raw_url, candidate_pattern)
                .to_string(),
        }
    }
}

/// How many of `urls` the candidate's pattern takes; none without a pattern.
pub(super) fn candidate_product_count(urls: &[String], candidate_pattern: Option<&Regex>) -> usize {
    candidate_pattern
        .map(|pattern| {
            urls.iter()
                .filter(|url| pattern_takes(pattern, url))
                .count()
        })
        .unwrap_or(0)
}

/// Whether `pattern` takes the stored URL `raw_url` as the spider matches it, with or without its
/// trailing slash.
pub(super) fn pattern_takes(pattern: &Regex, raw_url: &str) -> bool {
    CrawledUrl::try_from(raw_url).is_ok_and(|url| url.matches_pattern(pattern))
}
