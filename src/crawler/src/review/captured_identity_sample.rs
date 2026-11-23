//! The captured-identity sample of a live schema matrix: a bounded sample of the ListingSource's
//! product URLs whose listings were captured, outside the review pages, fetched live with the
//! review pages and evaluated with the schemas approval would make live. A candidate that derives
//! another identity from such a page than the one its listing was captured under would leave the
//! URL in `SourceListingIdConflict` once approved, which the review's own pages, usually never
//! captured, cannot show. The [approval guard](crate::review::approval_guard) refuses it; the
//! contract is in `docs/crawler/extraction-and-review.md` (approval guard). The sample also shows
//! the titles the ListingSource's live schemas and the candidate extract from each page, so a
//! candidate that adds or drops text on titles already captured shows before approval; that
//! blocks nothing, since a title may rightly improve.

use crate::scraper::css_selector::product_schema::{
    ProductCssSelectorSchema, RawExtractedProduct, SchemaExtraction,
};
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

/// What the schemas approval would make live, and the live ones, derive from each sampled
/// captured product URL.
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
    /// The distinct titles the ListingSource's live schemas extract from the page, in schema
    /// order; empty when the page was not fetched or no live schema extracts a title from it, and
    /// in a sample stored before the titles.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub live_titles: Vec<String>,
    /// The same of the schemas approval would make live.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidate_titles: Vec<String>,
}

impl CapturedIdentityPage {
    /// Whether the candidate extracts other titles from the page than the live schemas, which
    /// extract one: it adds or drops text on a title already captured, or loses it.
    pub(crate) fn title_changes(&self) -> bool {
        !self.live_titles.is_empty() && self.live_titles != self.candidate_titles
    }
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
    /// Evaluates `schemas`, the ones approval would make live, and the ListingSource's
    /// `live_schemas` on each sampled page. An extraction a rejecting plausibility finding
    /// withdraws is left out, as runtime selection never captures it.
    pub(crate) fn evaluate(
        schemas: &[ProductCssSelectorSchema],
        live_schemas: &[ProductCssSelectorSchema],
        pages: &[SampledCapturedPage],
    ) -> Self {
        Self {
            pages: pages
                .iter()
                .map(|sampled| evaluate_page(schemas, live_schemas, sampled))
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

    /// The sampled pages from which the candidate extracts other titles than the live schemas
    /// ([`CapturedIdentityPage::title_changes`]).
    pub(crate) fn title_changes(&self) -> impl Iterator<Item = &CapturedIdentityPage> {
        self.pages.iter().filter(|page| page.title_changes())
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

fn evaluate_page(
    schemas: &[ProductCssSelectorSchema],
    live_schemas: &[ProductCssSelectorSchema],
    sampled: &SampledCapturedPage,
) -> CapturedIdentityPage {
    let (outcome, live_titles, candidate_titles) = match sampled.html.as_deref() {
        None => (CapturedIdentityOutcome::NotFetched, Vec::new(), Vec::new()),
        Some(raw_html) => {
            let html = Html::parse_document(raw_html);
            let extractions = plausible_extractions(schemas, &html);
            (
                identity_outcome(&extractions, &sampled.page.captured_identity),
                distinct_titles(&plausible_extractions(live_schemas, &html)),
                distinct_titles(&extractions),
            )
        }
    };
    CapturedIdentityPage {
        url: sampled.page.url.to_string(),
        captured_identity: sampled.page.captured_identity.clone(),
        outcome,
        live_titles,
        candidate_titles,
    }
}

/// The distinct non-blank titles of `extractions`, trimmed and in their order.
fn distinct_titles(extractions: &[RawExtractedProduct]) -> Vec<String> {
    let mut titles: Vec<String> = Vec::new();
    for title in extractions.iter().filter_map(RawExtractedProduct::title) {
        let title = title.trim();
        if !titles.iter().any(|known| known == title) {
            titles.push(title.to_owned());
        }
    }
    titles
}

/// What each of `schemas` that runtime selection could use extracts from `html`, in schema order:
/// an extraction a rejecting plausibility finding withdraws is left out.
fn plausible_extractions(
    schemas: &[ProductCssSelectorSchema],
    html: &Html,
) -> Vec<RawExtractedProduct> {
    schemas
        .iter()
        .filter_map(|schema| match schema.extract(html) {
            Ok(SchemaExtraction { raw, .. })
                if PlausibilityFindings::of(&raw).rejection_reason().is_none() =>
            {
                Some(raw)
            }
            _ => None,
        })
        .collect()
}

fn identity_outcome(
    extractions: &[RawExtractedProduct],
    captured_identity: &str,
) -> CapturedIdentityOutcome {
    let identities: Vec<String> = extractions
        .iter()
        .filter_map(|raw| capturable_identity(&raw.source_listing_id))
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
            &[],
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
                live_titles: Vec::new(),
                candidate_titles: vec!["Botanical print rose".to_owned()],
            }]
        );
    }

    #[test]
    fn should_agree_when_every_schema_derives_the_captured_identity() {
        let sample = CapturedIdentitySample::evaluate(
            &[schema_with_identity(".sku"), schema_with_identity(".sku")],
            &[],
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
            &[],
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
            &[],
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
            CapturedIdentitySample::evaluate(&[schema_with_identity(".sku")], &[], &[unfetched]);

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
            live_titles: Vec::new(),
            candidate_titles: Vec::new(),
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

    mod captured_titles {
        use super::*;

        fn schema_with_title(title_selector: &str) -> ProductCssSelectorSchema {
            ProductCssSelectorSchema {
                title: Some(rule(title_selector)),
                ..schema_with_identity(".sku")
            }
        }

        /// A captured product page whose heading names the product and whose `.page-title`
        /// adds the shop's name, as a document `<title>` does.
        fn page_with_titles() -> SampledCapturedPage {
            let mut sampled = page("rose", "f_1", "f_1", "BG-1");
            sampled.html = Some(
                "<html><body><span class=\"page-title\">Botanical print rose | Shop</span>\
                 <h1>Botanical print rose</h1><span class=\"sku\">f_1</span>\
                 <span class=\"state\">Available</span><img src=\"rose.jpg\"></body></html>"
                    .to_owned(),
            );
            sampled
        }

        /// Antike Fundgrube's rule moved from the heading to the document `<title>`, which adds
        /// the shop's name to every title it had captured.
        #[test]
        fn should_show_the_titles_the_live_schemas_and_the_candidate_extract() {
            let sample = CapturedIdentitySample::evaluate(
                &[schema_with_title(".page-title")],
                &[schema_with_title("h1")],
                &[page_with_titles()],
            );

            let changed: Vec<_> = sample.title_changes().collect();
            assert_eq!(changed.len(), 1);
            assert_eq!(changed[0].live_titles, vec!["Botanical print rose"]);
            assert_eq!(
                changed[0].candidate_titles,
                vec!["Botanical print rose | Shop"]
            );
        }

        #[test]
        fn should_show_no_change_when_the_candidate_extracts_the_live_titles() {
            let sample = CapturedIdentitySample::evaluate(
                &[schema_with_title("h1"), schema_with_title("h1")],
                &[schema_with_title("h1")],
                &[page_with_titles()],
            );

            assert_eq!(
                sample.pages[0].candidate_titles,
                vec!["Botanical print rose"]
            );
            assert_eq!(sample.title_changes().count(), 0);
        }

        /// A source without live schemas, or whose live schemas no longer apply to the page,
        /// captured no title to compare with.
        #[test]
        fn should_show_no_change_where_no_live_schema_extracts_a_title() {
            let sample = CapturedIdentitySample::evaluate(
                &[schema_with_title(".page-title")],
                &[schema_with_title(".missing")],
                &[page_with_titles()],
            );

            assert!(sample.pages[0].live_titles.is_empty());
            assert_eq!(sample.title_changes().count(), 0);
        }

        #[test]
        fn should_load_a_sampled_page_stored_before_the_titles() {
            let page: CapturedIdentityPage = serde_json::from_value(serde_json::json!({
                "url": "https://shop.example/products/rose",
                "captured_identity": "f_1",
                "outcome": "AGREES"
            }))
            .unwrap();

            assert!(page.live_titles.is_empty() && page.candidate_titles.is_empty());
            assert!(!page.title_changes());
        }
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
                live_titles: Vec::new(),
                candidate_titles: Vec::new(),
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
