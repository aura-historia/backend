//! The captured-identity sample of a live schema matrix: a bounded sample of the ListingSource's
//! product URLs whose listings were captured, outside the review pages, fetched live with the
//! review pages and evaluated with the schemas approval would make live. A candidate that derives
//! another identity from such a page than the one its listing was captured under would leave the
//! URL in `SourceListingIdConflict` once approved, which the review's own pages, usually never
//! captured, cannot show. The [approval guard](crate::review::approval_guard) refuses it; the
//! contract is in `docs/crawler/extraction-and-review.md` (approval guard).

use crate::scraper::css_selector::product_schema::{ProductCssSelectorSchema, SchemaExtraction};
use crate::scraper::plausibility::{
    PlausibilityFindings, capturable_identity, is_link_or_path_identity,
};
use scraper::Html;
use serde::{Deserialize, Serialize};
use url::Url;

/// How many captured product URLs a live matrix samples. Each is fetched at the shop's pacing
/// next to the review pages, so the sample stays small; a refresh draws a new one.
pub(crate) const CAPTURED_IDENTITY_SAMPLE_SIZE: i64 = 5;

/// A product URL of the review's ListingSource and the identity its listing was captured under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedProductUrl {
    pub url: Url,
    pub captured_identity: String,
}

/// A sampled captured product URL as the shop served it: its HTML, or `None` when the fetch
/// failed.
#[derive(Debug, Clone)]
pub struct SampledCapturedPage {
    pub page: CapturedProductUrl,
    pub html: Option<String>,
}

/// What the schemas approval would make live derive from each sampled captured product URL.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedIdentitySample {
    pub pages: Vec<CapturedIdentityPage>,
}

/// One sampled captured product URL and what the schemas derive from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedIdentityPage {
    pub url: String,
    pub captured_identity: String,
    #[serde(flatten)]
    pub outcome: CapturedIdentityOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CapturedIdentityOutcome {
    /// Every schema runtime selection would use for the page derives the captured identity.
    Agrees,
    /// Schemas runtime selection would use for the page derive these other identities, distinct
    /// and in schema order.
    Differs { identities: Vec<String> },
    /// No schema derives an identity from the page, such as a product page the shop removed
    /// since; it shows nothing about the identity rule.
    NoIdentity,
    /// The page could not be fetched.
    NotFetched,
}

impl CapturedIdentitySample {
    /// Evaluates `schemas` on each sampled page. An extraction a rejecting plausibility finding
    /// withdraws is left out, as runtime selection never captures it.
    pub(crate) fn evaluate(
        schemas: &[ProductCssSelectorSchema],
        pages: &[SampledCapturedPage],
    ) -> Self {
        Self {
            pages: pages
                .iter()
                .map(|sampled| CapturedIdentityPage {
                    url: sampled.page.url.to_string(),
                    captured_identity: sampled.page.captured_identity.clone(),
                    outcome: sampled
                        .html
                        .as_deref()
                        .map_or(CapturedIdentityOutcome::NotFetched, |html| {
                            outcome_on(schemas, html, &sampled.page.captured_identity)
                        }),
                })
                .collect(),
        }
    }

    /// The sampled pages from which a schema derives another identity than the captured one.
    pub(crate) fn differing_pages(&self) -> Vec<CapturedIdentityPage> {
        self.pages
            .iter()
            .filter(|page| matches!(page.outcome, CapturedIdentityOutcome::Differs { .. }))
            .cloned()
            .collect()
    }

    /// Whether the sample holds pages but none of them could be fetched, so it shows nothing.
    pub(crate) fn fetched_none(&self) -> bool {
        !self.pages.is_empty()
            && self
                .pages
                .iter()
                .all(|page| page.outcome == CapturedIdentityOutcome::NotFetched)
    }

    /// Whether the candidate keeps URL identities the ListingSource's listings were captured
    /// under: every sampled listing was captured under a URL or path, and the schemas derive the
    /// same from at least one of them. Only then may an approval keep an identity that is a URL:
    /// such listings cannot move to another identity, while a source without them must not start
    /// using one. A sampled listing captured under a code shows a source whose identity an
    /// approval switched to the page URL after capture, whose code listings stay in conflict
    /// under a URL rule, so it keeps the URL identity out of approval.
    pub(crate) fn keeps_captured_link_identities(&self) -> bool {
        self.pages
            .iter()
            .all(|page| is_link_or_path_identity(&page.captured_identity))
            && self
                .pages
                .iter()
                .any(|page| page.outcome == CapturedIdentityOutcome::Agrees)
    }
}

fn outcome_on(
    schemas: &[ProductCssSelectorSchema],
    raw_html: &str,
    captured_identity: &str,
) -> CapturedIdentityOutcome {
    let html = Html::parse_document(raw_html);
    let identities: Vec<String> = schemas
        .iter()
        .filter_map(|schema| match schema.extract(&html) {
            Ok(SchemaExtraction { raw, .. })
                if PlausibilityFindings::of(&raw).rejection_reason().is_none() =>
            {
                capturable_identity(&raw.source_listing_id)
            }
            _ => None,
        })
        .collect();
    let mut differing: Vec<String> = Vec::new();
    for identity in &identities {
        if identity != captured_identity && !differing.contains(identity) {
            differing.push(identity.clone());
        }
    }
    match (identities.is_empty(), differing.is_empty()) {
        (true, _) => CapturedIdentityOutcome::NoIdentity,
        (false, true) => CapturedIdentityOutcome::Agrees,
        (false, false) => CapturedIdentityOutcome::Differs {
            identities: differing,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::css_selector::rule::{
        CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule,
    };
    use std::collections::BTreeMap;

    fn rule(selector: &str) -> ExtractionRule {
        rule_extracting(selector, ExtractionKind::Text)
    }

    fn rule_extracting(selector: &str, extract: ExtractionKind) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector.to_owned()),
            additional_selectors: Vec::new(),
            extract,
            cardinality: ExtractionCardinality::First,
        }
    }

    fn schema_with_identity(identity_selector: &str) -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            source_listing_id: Some(rule(identity_selector)),
            title: Some(rule("h1")),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: Some(rule(".state")),
            images: Some(rule_extracting(
                "img",
                ExtractionKind::Attribute { name: "src".into() },
            )),
            raw_attributes: BTreeMap::new(),
            sanitize: Default::default(),
        }
    }

    /// A product page whose SKU is `sku` and whose facts list ends with `code`.
    fn page(slug: &str, captured: &str, sku: &str, code: &str) -> SampledCapturedPage {
        SampledCapturedPage {
            page: CapturedProductUrl {
                url: Url::parse(&format!("https://shop.example/products/{slug}")).unwrap(),
                captured_identity: captured.to_owned(),
            },
            html: Some(format!(
                "<html><body><h1>Botanical print {slug}</h1>\
                 <span class=\"sku\">{sku}</span><dl class=\"facts\"><dd>{code}</dd></dl>\
                 <span class=\"state\">Available</span><img src=\"{slug}.jpg\"></body></html>"
            )),
        }
    }

    #[test]
    fn should_report_the_identity_a_schema_derives_instead_of_the_captured_one() {
        let sample = CapturedIdentitySample::evaluate(
            &[schema_with_identity("dl.facts dd:last-child")],
            &[page("rose", "f_42759492", "f_42759492", "BG-13692-3-54")],
        );

        assert_eq!(
            sample.differing_pages(),
            vec![CapturedIdentityPage {
                url: "https://shop.example/products/rose".to_owned(),
                captured_identity: "f_42759492".to_owned(),
                outcome: CapturedIdentityOutcome::Differs {
                    identities: vec!["BG-13692-3-54".to_owned()],
                },
            }]
        );
    }

    #[test]
    fn should_agree_when_every_schema_derives_the_captured_identity() {
        let sample = CapturedIdentitySample::evaluate(
            &[schema_with_identity(".sku"), schema_with_identity(".sku")],
            &[page("rose", "f_1", "f_1", "BG-1")],
        );

        assert_eq!(sample.pages[0].outcome, CapturedIdentityOutcome::Agrees);
        assert!(sample.differing_pages().is_empty());
    }

    #[test]
    fn should_report_a_second_schema_that_derives_another_identity_from_the_page() {
        let sample = CapturedIdentitySample::evaluate(
            &[
                schema_with_identity(".sku"),
                schema_with_identity("dl.facts dd"),
            ],
            &[page("rose", "f_1", "f_1", "BG-1")],
        );

        assert_eq!(
            sample.pages[0].outcome,
            CapturedIdentityOutcome::Differs {
                identities: vec!["BG-1".to_owned()],
            }
        );
    }

    #[test]
    fn should_show_nothing_for_a_page_no_schema_derives_an_identity_from() {
        let sample = CapturedIdentitySample::evaluate(
            &[schema_with_identity(".missing")],
            &[page("rose", "f_1", "f_1", "BG-1")],
        );

        assert_eq!(sample.pages[0].outcome, CapturedIdentityOutcome::NoIdentity);
        assert!(sample.differing_pages().is_empty());
        assert!(!sample.fetched_none());
    }

    #[test]
    fn should_tell_a_sample_whose_pages_were_all_unfetched() {
        let mut unfetched = page("rose", "f_1", "f_1", "BG-1");
        unfetched.html = None;
        let sample =
            CapturedIdentitySample::evaluate(&[schema_with_identity(".sku")], &[unfetched]);

        assert_eq!(sample.pages[0].outcome, CapturedIdentityOutcome::NotFetched);
        assert!(sample.fetched_none());
        assert!(!CapturedIdentitySample::default().fetched_none());
    }

    #[test]
    fn should_store_each_outcome_under_its_code() {
        let json = serde_json::to_value(CapturedIdentityPage {
            url: "https://shop.example/products/rose".to_owned(),
            captured_identity: "J054".to_owned(),
            outcome: CapturedIdentityOutcome::Differs {
                identities: vec!["https://shop.example/products/rose".to_owned()],
            },
        })
        .unwrap();

        assert_eq!(json["outcome"], "DIFFERS");
        assert_eq!(json["captured_identity"], "J054");
        assert_eq!(
            serde_json::from_value::<CapturedIdentityPage>(json)
                .unwrap()
                .outcome,
            CapturedIdentityOutcome::Differs {
                identities: vec!["https://shop.example/products/rose".to_owned()],
            }
        );
    }

    mod captured_link_identities {
        use super::*;

        const ROSE_URL: &str = "https://shop.example/products/rose";

        fn sampled(
            captured_identity: &str,
            outcome: CapturedIdentityOutcome,
        ) -> CapturedIdentityPage {
            CapturedIdentityPage {
                url: ROSE_URL.to_owned(),
                captured_identity: captured_identity.to_owned(),
                outcome,
            }
        }

        fn sample(pages: Vec<CapturedIdentityPage>) -> CapturedIdentitySample {
            CapturedIdentitySample { pages }
        }

        #[test]
        fn should_keep_them_when_every_sampled_listing_has_one_and_the_schemas_derive_it() {
            let sample = sample(vec![
                sampled(ROSE_URL, CapturedIdentityOutcome::Agrees),
                sampled("/products/tulip", CapturedIdentityOutcome::NotFetched),
            ]);

            assert!(sample.keeps_captured_link_identities());
        }

        /// HaresFur after its second approval: listings captured under SKUs beside listings
        /// captured under the og:url the new rule derives.
        #[test]
        fn should_not_keep_them_when_a_sampled_listing_was_captured_under_a_code() {
            let sample = sample(vec![
                sampled(ROSE_URL, CapturedIdentityOutcome::Agrees),
                sampled(
                    "J054",
                    CapturedIdentityOutcome::Differs {
                        identities: vec!["https://shop.example/products/tea-bowl".to_owned()],
                    },
                ),
            ]);

            assert!(!sample.keeps_captured_link_identities());
        }

        #[rstest::rstest]
        #[case::differs(CapturedIdentityOutcome::Differs { identities: vec!["R-17".to_owned()] })]
        #[case::no_identity(CapturedIdentityOutcome::NoIdentity)]
        #[case::not_fetched(CapturedIdentityOutcome::NotFetched)]
        fn should_not_keep_them_when_the_schemas_derive_none_of_them(
            #[case] outcome: CapturedIdentityOutcome,
        ) {
            assert!(!sample(vec![sampled(ROSE_URL, outcome)]).keeps_captured_link_identities());
        }

        #[test]
        fn should_not_keep_them_for_a_source_without_captured_listings() {
            assert!(!CapturedIdentitySample::default().keeps_captured_link_identities());
        }
    }
}
