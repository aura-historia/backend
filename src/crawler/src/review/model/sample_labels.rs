use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// What the author agent or a human says a URL of a URL-pattern review's sample is. In `agent`
/// authoring mode these labels, not the crawler, say which URLs are product pages; the contract is
/// in `docs/crawler/extraction-and-review.md` (approval guard).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SampleUrlLabel {
    /// The URL is the page of one product: the decision must take it.
    Product,
    /// The URL is no product page, such as a category, an article or the imprint: the decision
    /// must leave it.
    NotProduct,
}

/// One labelled URL: what it is and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelledSampleUrl {
    /// The URL in the crawler's spelling, the one the spider stores and a pattern is matched
    /// against.
    pub url: String,
    pub label: SampleUrlLabel,
    /// Why the URL has its label, in a few words.
    pub reason: String,
}

/// The sample labels of a URL-pattern candidate, set as a whole, and when they were set. They
/// belong to the candidate: a change bumps its version and is recorded as a candidate edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SampleLabels {
    #[serde(with = "time::serde::rfc3339")]
    pub labelled_at: OffsetDateTime,
    pub labels: Vec<LabelledSampleUrl>,
}

impl SampleLabels {
    /// The labelled URLs with `label`.
    pub fn labelled(&self, label: SampleUrlLabel) -> impl Iterator<Item = &LabelledSampleUrl> {
        self.labels.iter().filter(move |url| url.label == label)
    }

    /// How many URLs carry each label.
    pub fn counts(&self) -> SampleLabelCounts {
        SampleLabelCounts {
            product: self.labelled(SampleUrlLabel::Product).count(),
            not_product: self.labelled(SampleUrlLabel::NotProduct).count(),
        }
    }
}

/// How many URLs the sample labels mark as products and as no products.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct SampleLabelCounts {
    pub product: usize,
    pub not_product: usize,
}

/// A sample label as a client sends it, before the crawler checks its URL and reason.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SampleLabelInput {
    pub url: String,
    pub label: SampleUrlLabel,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn labelled(url: &str, label: SampleUrlLabel) -> LabelledSampleUrl {
        LabelledSampleUrl {
            url: url.to_owned(),
            label,
            reason: "seen on the page".to_owned(),
        }
    }

    #[test]
    fn should_persist_each_label_in_screaming_snake_case_next_to_its_url_and_reason() {
        let labels = SampleLabels {
            labelled_at: OffsetDateTime::UNIX_EPOCH,
            labels: vec![
                labelled("https://shop.example/item/1", SampleUrlLabel::Product),
                labelled("https://shop.example/journal", SampleUrlLabel::NotProduct),
            ],
        };

        let stored = serde_json::to_value(&labels).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            stored,
            json!({
                "labelled_at": "1970-01-01T00:00:00Z",
                "labels": [
                    { "url": "https://shop.example/item/1", "label": "PRODUCT", "reason": "seen on the page" },
                    { "url": "https://shop.example/journal", "label": "NOT_PRODUCT", "reason": "seen on the page" },
                ],
            })
        );
        assert_eq!(
            serde_json::from_value::<SampleLabels>(stored).ok(),
            Some(labels)
        );
    }

    #[test]
    fn should_refuse_a_label_no_url_can_carry() {
        for label in ["product", "UNKNOWN", "CATEGORY", ""] {
            assert!(
                serde_json::from_value::<SampleUrlLabel>(json!(label)).is_err(),
                "{label}"
            );
        }
    }

    #[test]
    fn should_count_the_urls_of_each_label() {
        let labels = SampleLabels {
            labelled_at: OffsetDateTime::UNIX_EPOCH,
            labels: vec![
                labelled("https://shop.example/item/1", SampleUrlLabel::Product),
                labelled("https://shop.example/item/2", SampleUrlLabel::Product),
                labelled("https://shop.example/journal", SampleUrlLabel::NotProduct),
            ],
        };

        assert_eq!(
            labels.counts(),
            SampleLabelCounts {
                product: 2,
                not_product: 1,
            }
        );
    }
}
