//! The page classifier's verdicts on the opening pages of a fresh `manual_schema_required` review,
//! which the review agent runner sends with the rejection it bases on them and which the review's
//! `REJECTED` event keeps (`docs/crawler/extraction-and-review.md#page-classifier`).

use crate::CrawlerReviewPageId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The most characters of one verdict's reason.
pub const MAX_PAGE_VERDICT_REASON_CHARS: usize = 500;

/// What the page classifier read one page as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PageVerdict {
    /// One item offered or once offered by the shop, sold or reserved included.
    Product,
    /// A removal or not-found notice for the item, answered with HTTP 200.
    Removed,
    /// A category, collection, search or overview page that lists several items.
    Listing,
    /// Any other page without an item of its own, such as contact, about or a blog post.
    Other,
    /// The content does not decide it.
    Unsure,
}

impl PageVerdict {
    /// Whether the verdict says the page is no product page and shows no removal of one, the only
    /// verdicts a classifier rejection rests on.
    pub const fn rules_out_product(self) -> bool {
        matches!(self, Self::Listing | Self::Other)
    }

    /// The persisted and serialized name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Product => "PRODUCT",
            Self::Removed => "REMOVED",
            Self::Listing => "LISTING",
            Self::Other => "OTHER",
            Self::Unsure => "UNSURE",
        }
    }
}

/// Why the classifier gave its verdict: one non-blank sentence of at most
/// [`MAX_PAGE_VERDICT_REASON_CHARS`] characters, trimmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PageVerdictReason(String);

impl PageVerdictReason {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PageVerdictReason {
    type Error = PageClassificationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(PageClassificationError::BlankReason);
        }
        if trimmed.chars().count() > MAX_PAGE_VERDICT_REASON_CHARS {
            return Err(PageClassificationError::ReasonTooLong);
        }
        Ok(Self(trimmed.to_owned()))
    }
}

impl From<PageVerdictReason> for String {
    fn from(reason: PageVerdictReason) -> Self {
        reason.0
    }
}

/// The classifier's verdict on one review page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassifiedPage {
    pub review_page_id: CrawlerReviewPageId,
    pub verdict: PageVerdict,
    pub reason: PageVerdictReason,
}

/// The classifier's verdicts on a review's pages: at least one, each page once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PageClassificationPayload")]
pub struct PageClassification {
    pages: Vec<ClassifiedPage>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageClassificationPayload {
    pages: Vec<ClassifiedPage>,
}

impl PageClassification {
    pub fn new(pages: Vec<ClassifiedPage>) -> Result<Self, PageClassificationError> {
        if pages.is_empty() {
            return Err(PageClassificationError::NoPages);
        }
        let mut seen = BTreeSet::new();
        if !pages.iter().all(|page| seen.insert(page.review_page_id)) {
            return Err(PageClassificationError::DuplicatePage);
        }
        Ok(Self { pages })
    }

    pub fn pages(&self) -> &[ClassifiedPage] {
        &self.pages
    }

    /// The pages the verdicts name.
    pub fn page_ids(&self) -> BTreeSet<CrawlerReviewPageId> {
        self.pages.iter().map(|page| page.review_page_id).collect()
    }

    /// Whether every verdict rules a product out, so a rejection may rest on them.
    pub fn rules_out_every_product(&self) -> bool {
        self.pages
            .iter()
            .all(|page| page.verdict.rules_out_product())
    }
}

impl TryFrom<PageClassificationPayload> for PageClassification {
    type Error = PageClassificationError;

    fn try_from(payload: PageClassificationPayload) -> Result<Self, Self::Error> {
        Self::new(payload.pages)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PageClassificationError {
    #[error("a page classification names at least one page")]
    NoPages,
    #[error("a page classification names each page once")]
    DuplicatePage,
    #[error("a verdict's reason must not be blank")]
    BlankReason,
    #[error("a verdict's reason has at most {MAX_PAGE_VERDICT_REASON_CHARS} characters")]
    ReasonTooLong,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn page(verdict: &str) -> serde_json::Value {
        json!({
            "review_page_id": CrawlerReviewPageId::new().to_string(),
            "verdict": verdict,
            "reason": " Lists twelve chairs with prices. ",
        })
    }

    fn classification(pages: Vec<serde_json::Value>) -> Result<PageClassification, String> {
        serde_json::from_value(json!({ "pages": pages })).map_err(|error| error.to_string())
    }

    #[test]
    fn should_read_verdicts_and_trim_their_reasons() {
        let read = classification(vec![page("LISTING"), page("OTHER")]).expect("classification");

        assert_eq!(read.pages().len(), 2);
        assert_eq!(read.pages()[0].verdict, PageVerdict::Listing);
        assert_eq!(
            read.pages()[0].reason.as_str(),
            "Lists twelve chairs with prices."
        );
        assert!(read.rules_out_every_product());
    }

    #[rstest::rstest]
    #[case::product("PRODUCT")]
    #[case::removed("REMOVED")]
    #[case::unsure("UNSURE")]
    fn should_not_rule_out_every_product_when_a_verdict_leaves_one_possible(#[case] verdict: &str) {
        let read = classification(vec![page("LISTING"), page(verdict)]).expect("classification");

        assert!(!read.rules_out_every_product());
    }

    #[test]
    fn should_refuse_a_classification_without_pages_or_with_a_page_twice() {
        assert!(classification(vec![]).is_err());
        let twice = page("LISTING");
        assert!(classification(vec![twice.clone(), twice]).is_err());
    }

    #[test]
    fn should_refuse_blank_or_overlong_reasons_and_unknown_verdicts_or_keys() {
        let mut blank = page("OTHER");
        blank["reason"] = json!("  ");
        let mut long = page("OTHER");
        long["reason"] = json!("a".repeat(MAX_PAGE_VERDICT_REASON_CHARS + 1));
        let mut extra = page("OTHER");
        extra["confidence"] = json!(0.9);

        for refused in [blank, long, page("CATEGORY"), extra] {
            assert!(classification(vec![refused.clone()]).is_err(), "{refused}");
        }
    }

    #[test]
    fn should_name_each_verdict_as_it_serializes() {
        for verdict in [
            PageVerdict::Product,
            PageVerdict::Removed,
            PageVerdict::Listing,
            PageVerdict::Other,
            PageVerdict::Unsure,
        ] {
            assert_eq!(
                serde_json::to_value(verdict).ok(),
                Some(json!(verdict.as_str()))
            );
        }
    }
}
