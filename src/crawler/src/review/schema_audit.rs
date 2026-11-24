//! The live schema audit's judgement of a ListingSource's live schemas: the approval guard's
//! blocking findings, evaluated on a sample of the source's captured product pages, and a title
//! rule that reads a source that usually carries more than the product's name. A schema passes
//! those checks once, at approval; the audit applies them again later, so a check added since,
//! or a shop that changed its pages, reopens the source for repair. The contract is in
//! `docs/crawler/extraction-and-review.md` (live schema audit).

use crate::review::captured_identity_sample::{CapturedIdentitySample, SampledCapturedPage};
use crate::review::model::{LinkIdentity, PAGE_ROLE_SEED, SchemaReviewPageInput};
use crate::review::schema_evaluation::{
    ImplausiblePage, evaluate_schema_matrix_for_captured_pages, implausible_pages,
};
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::css_selector::rule::{ExtractionKind, ExtractionRule};
use std::collections::{BTreeSet, HashMap};

/// The reason of a product-schema review the audit opens. While one is pending, the
/// ListingSource's listings that were never captured wait, so none takes the defect into its
/// permanent slug; captured listings keep updating.
pub const SCHEMA_AUDIT_REASON: &str = "schema_audit_required";

/// Where a title rule reads the title from, when that is a source that usually adds the shop's
/// name, a path or a tail to the product's name. No cross-page check sees what differs per page,
/// such as a category path, so the audit names the source for the review agents to judge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TitleSource {
    /// The document `<title>`.
    DocumentTitle,
    /// An `og:title` or `twitter:title` meta tag.
    MetaTitle,
    /// An image's `alt` attribute.
    ImageAlt,
}

impl TitleSource {
    const fn code(self) -> &'static str {
        match self {
            Self::DocumentTitle => "READS_DOCUMENT_TITLE",
            Self::MetaTitle => "READS_META_TITLE",
            Self::ImageAlt => "READS_IMAGE_ALT",
        }
    }

    /// The source `rule` reads, if it is one of these. Every selector of the rule counts, its
    /// fallbacks included.
    fn of(rule: &ExtractionRule) -> Option<Self> {
        let reads_alt = matches!(
            &rule.extract,
            ExtractionKind::Attribute { name } | ExtractionKind::AttributeOrText { name }
                if name.eq_ignore_ascii_case("alt")
        );
        if reads_alt {
            return Some(Self::ImageAlt);
        }
        rule.selectors().find_map(|selector| {
            let selector = selector.to_ascii_lowercase();
            if selector.contains("og:title") || selector.contains("twitter:title") {
                Some(Self::MetaTitle)
            } else if selects_document_title(&selector) {
                Some(Self::DocumentTitle)
            } else {
                None
            }
        })
    }
}

/// Whether a lowercased selector selects the `<title>` element in one of its alternatives: its
/// last compound names the element `title`, as `title` or `head > title` do, and not a `title`
/// attribute or class.
fn selects_document_title(selector: &str) -> bool {
    selector.split(',').any(|alternative| {
        alternative
            .trim()
            .rsplit([' ', '>', '+', '~'])
            .next()
            .is_some_and(|compound| {
                compound == "title"
                    || compound
                        .strip_prefix("title")
                        .is_some_and(|rest| rest.starts_with([':', '.', '#', '[']))
            })
    })
}

/// What the audit found wrong with a ListingSource's live schemas, as `field CHECK` codes, such
/// as `title LISTING_SOURCE_NAME` or `title READS_DOCUMENT_TITLE`, distinct and sorted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SchemaAuditFindings(BTreeSet<String>);

impl SchemaAuditFindings {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn codes(&self) -> Vec<String> {
        self.0.iter().cloned().collect()
    }

    /// Whether every finding is among the `accepted` ones: an earlier audit review of the same
    /// live schemas was approved with them, so reopening the source would only repeat it.
    pub(crate) fn accepted_by(&self, accepted: &[String]) -> bool {
        self.0.iter().all(|code| accepted.contains(code))
    }
}

/// Audits the ListingSource's `live_schemas` on its `sampled` captured product pages: every
/// finding the approval guard refuses an agent's approval for, except price text in a `state`,
/// which runtime capture accepts and an approval may have accepted on a status slot, and an
/// identity that is the page URL the listings were captured under; and the title source of every
/// schema. A page that could not be fetched is left out; with none fetched the audit finds nothing.
pub(crate) fn audit_live_schemas(
    listing_source_name: &str,
    live_schemas: &[ProductCssSelectorSchema],
    sampled: &[SampledCapturedPage],
) -> SchemaAuditFindings {
    let pages: Vec<SchemaReviewPageInput> = sampled
        .iter()
        .filter_map(|sampled| {
            Some(SchemaReviewPageInput {
                url: sampled.page.url.to_string(),
                role: PAGE_ROLE_SEED.to_owned(),
                raw_html: sampled.html.clone()?,
            })
        })
        .collect();
    if pages.is_empty() {
        return SchemaAuditFindings::default();
    }
    let captured_identities: HashMap<String, String> = sampled
        .iter()
        .map(|sampled| {
            (
                sampled.page.url.to_string(),
                sampled.page.captured_identity.clone(),
            )
        })
        .collect();
    let matrix = evaluate_schema_matrix_for_captured_pages(
        listing_source_name,
        live_schemas,
        &pages,
        &captured_identities,
    );
    let link_identity = if CapturedIdentitySample::evaluate(live_schemas, live_schemas, sampled)
        .keeps_captured_link_identities()
    {
        LinkIdentity::Captured
    } else {
        LinkIdentity::Blocks
    };
    let page_findings = implausible_pages(&matrix, link_identity)
        .iter()
        .flat_map(blocking_codes)
        .collect::<Vec<_>>();
    let title_sources = live_schemas
        .iter()
        .filter_map(|schema| schema.title.as_ref().and_then(TitleSource::of))
        .map(|source| format!("title {}", source.code()));
    SchemaAuditFindings(page_findings.into_iter().chain(title_sources).collect())
}

/// The `field CHECK` codes of the findings that keep `page` out of approval, price text in a
/// `state` aside.
fn blocking_codes(page: &ImplausiblePage) -> Vec<String> {
    let per_page = page
        .plausibility_findings
        .as_slice()
        .iter()
        .filter(|finding| !finding.is_price_text_on_state())
        .map(|finding| field_code(finding.field.as_str(), &finding.check));
    let cross_page = page
        .cross_page_findings
        .as_slice()
        .iter()
        .map(|finding| field_code(finding.field.as_str(), &finding.check));
    let identity = page
        .identity_agreement_findings
        .as_slice()
        .iter()
        .map(|finding| field_code(finding.field.as_str(), &finding.check));
    per_page.chain(cross_page).chain(identity).collect()
}

/// `field CHECK`, the check named by its persisted code.
fn field_code(field: &str, check: &impl serde::Serialize) -> String {
    let code = serde_json::to_value(check)
        .ok()
        .and_then(|code| code.as_str().map(str::to_owned))
        .unwrap_or_default();
    format!("{field} {code}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::captured_identity_sample::CapturedProductUrl;
    use crate::scraper::css_selector::rule::{CssSelector, ExtractionCardinality};
    use url::Url;

    fn rule(selector: &str, extract: ExtractionKind) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: Vec::new(),
            extract,
            cardinality: ExtractionCardinality::First,
        }
    }

    fn text(selector: &str) -> ExtractionRule {
        rule(selector, ExtractionKind::Text)
    }

    fn schema_with_title(title: ExtractionRule) -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            source_listing_id: Some(text(".sku")),
            title: Some(title),
            description: None,
            price: Some(text(".price")),
            price_estimate_min: None,
            price_estimate_max: None,
            state: Some(text(".state")),
            images: Some(rule("img", ExtractionKind::ImageUrl)),
            raw_attributes: Default::default(),
        }
    }

    /// A captured product page of Bauman Rare Books, whose document `<title>` adds the edition,
    /// the author and the shop's name to the heading.
    fn captured(id: &str, name: &str, state: &str) -> SampledCapturedPage {
        SampledCapturedPage {
            page: CapturedProductUrl {
                url: Url::parse(&format!("https://shop.example/rare-books/{id}.aspx"))
                    .unwrap_or_else(|error| panic!("a test URL must parse: {error}")),
                captured_identity: id.to_owned(),
            },
            html: Some(format!(
                "<html><head><title>{name} First Edition - Bauman Rare Books</title></head>\
                 <body><h4>{name}</h4><span class=\"sku\">{id}</span>\
                 <span class=\"price\">$1,800</span><span class=\"state\">{state}</span>\
                 <img src=\"/images/{id}.jpg\"></body></html>"
            )),
        }
    }

    fn sample() -> Vec<SampledCapturedPage> {
        vec![
            captured("131033", "Human Condition", "In stock"),
            captured("130034", "Suttree", "In stock"),
        ]
    }

    mod title_sources {
        use super::*;

        #[rstest::rstest]
        #[case::document_title(text("title"), Some(TitleSource::DocumentTitle))]
        #[case::head_title(text("head > title"), Some(TitleSource::DocumentTitle))]
        #[case::alternative(text("h1.name, title"), Some(TitleSource::DocumentTitle))]
        #[case::og_title(
            rule("meta[property='og:title']", ExtractionKind::Attribute { name: "content".into() }),
            Some(TitleSource::MetaTitle)
        )]
        #[case::twitter_title(
            rule("meta[name=\"twitter:title\"]", ExtractionKind::Attribute { name: "content".into() }),
            Some(TitleSource::MetaTitle)
        )]
        #[case::image_alt(
            rule("img.image-entity", ExtractionKind::Attribute { name: "alt".into() }),
            Some(TitleSource::ImageAlt)
        )]
        #[case::image_alt_or_text(
            rule("img.hero", ExtractionKind::AttributeOrText { name: "ALT".into() }),
            Some(TitleSource::ImageAlt)
        )]
        #[case::heading(text("h1"), None)]
        #[case::title_class(text("div.product .title"), None)]
        #[case::title_attribute(text("a[title]"), None)]
        #[case::title_in_a_name(text("h2.title_lace"), None)]
        fn should_name_a_title_rule_that_reads_a_source_beside_the_heading(
            #[case] title: ExtractionRule,
            #[case] expected: Option<TitleSource>,
        ) {
            assert_eq!(TitleSource::of(&title), expected);
        }

        #[test]
        fn should_count_a_fallback_selector_of_the_title_rule() {
            let mut title = text("h1");
            title.additional_selectors = vec![CssSelector::from("title")];

            assert_eq!(TitleSource::of(&title), Some(TitleSource::DocumentTitle));
        }
    }

    mod audit {
        use super::*;

        #[test]
        fn should_report_a_title_rule_on_the_document_title_that_adds_the_shop_name() {
            let findings = audit_live_schemas(
                "Bauman Rare Books",
                &[schema_with_title(text("title"))],
                &sample(),
            );

            assert_eq!(
                findings.codes(),
                vec!["title LISTING_SOURCE_NAME", "title READS_DOCUMENT_TITLE"]
            );
        }

        #[test]
        fn should_find_nothing_when_the_title_rule_reads_the_product_heading() {
            let findings = audit_live_schemas(
                "Bauman Rare Books",
                &[schema_with_title(text("h4"))],
                &sample(),
            );

            assert!(findings.is_empty(), "{findings:?}");
        }

        /// Runtime capture accepts price text in a `state`, and an approval may have accepted it
        /// on a status slot whose sold page is not in the sample.
        #[test]
        fn should_leave_price_text_in_a_state_alone() {
            let sampled = vec![
                captured("131033", "Human Condition", "Price: $1,800"),
                captured("130034", "Suttree", "In stock"),
            ];

            let findings = audit_live_schemas(
                "Bauman Rare Books",
                &[schema_with_title(text("h4"))],
                &sampled,
            );

            assert!(findings.is_empty(), "{findings:?}");
        }

        #[test]
        fn should_report_an_identity_that_differs_from_the_captured_one() {
            let mut sampled = sample();
            sampled[0].page.captured_identity = "SKU-0815".to_owned();

            let findings = audit_live_schemas(
                "Bauman Rare Books",
                &[schema_with_title(text("h4"))],
                &sampled,
            );

            assert_eq!(
                findings.codes(),
                vec!["source_listing_id DIFFERS_FROM_CAPTURED_IDENTITY"]
            );
        }

        #[test]
        fn should_find_nothing_without_a_fetched_page() {
            let mut sampled = sample();
            for page in &mut sampled {
                page.html = None;
            }

            let findings = audit_live_schemas(
                "Bauman Rare Books",
                &[schema_with_title(text("title"))],
                &sampled,
            );

            assert!(findings.is_empty(), "{findings:?}");
        }
    }

    mod acceptance {
        use super::*;

        fn findings(codes: &[&str]) -> SchemaAuditFindings {
            SchemaAuditFindings(codes.iter().map(|code| (*code).to_owned()).collect())
        }

        #[test]
        fn should_be_accepted_when_an_approval_covered_every_finding() {
            let accepted = vec![
                "title READS_META_TITLE".to_owned(),
                "images NO_IMAGE_ON_ANY_PAGE".to_owned(),
            ];

            assert!(findings(&["title READS_META_TITLE"]).accepted_by(&accepted));
        }

        /// A shop that changed its `og:title` after the approval adds a finding the approval
        /// never saw.
        #[test]
        fn should_not_be_accepted_when_a_finding_is_new() {
            let accepted = vec!["title READS_META_TITLE".to_owned()];

            assert!(
                !findings(&["title READS_META_TITLE", "title LISTING_SOURCE_NAME"])
                    .accepted_by(&accepted)
            );
        }
    }
}
