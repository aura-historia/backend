//! The live schema audit's judgement of a ListingSource's live schemas: the approval guard's
//! blocking findings, evaluated on a sample of the source's captured product pages, and a title
//! rule that reads a source that usually carries more than the product's name. A schema passes
//! those checks once, at approval; the audit applies them again later, so a check added since,
//! or a shop that changed its pages, reopens the source for repair. The contract is in
//! `docs/crawler/extraction-and-review.md` (live schema audit).

use crate::review::captured_identity_sample::{CapturedIdentitySample, SampledCapturedPage};
use crate::review::model::{LinkIdentity, PAGE_ROLE_SEED, SchemaReviewPageInput};
use crate::review::schema_evaluation::{
    ImplausiblePage, PriceTextAcceptance, evaluate_schema_matrix_for_captured_pages,
    implausible_pages,
};
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::css_selector::rule::{ExtractionKind, ExtractionRule};
use std::collections::{BTreeSet, HashMap};

mod title_hints;

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

    /// The findings that are not among the `accepted` ones: an earlier audit review of the same
    /// live schemas judged those, so reopening the source for them would only repeat it.
    pub(crate) fn excluding(&self, accepted: &[String]) -> Self {
        Self(
            self.0
                .iter()
                .filter(|code| !accepted.contains(code))
                .cloned()
                .collect(),
        )
    }

    /// Whether a finding is one the approval guard refuses, so the source's never-captured
    /// listings wait for the repair. A hint for the agents to judge, such as the title source or
    /// [`title_hints`], may be no defect, so it holds nothing back.
    pub(crate) fn holds_new_listings(&self) -> bool {
        self.0.iter().any(|code| !is_hint(code))
    }
}

/// Audits the ListingSource's `live_schemas` on its `sampled` captured product pages: every
/// finding the approval guard refuses an agent's approval for, except price text in a `state` on
/// fewer than most of the pages a schema applies to, which runtime capture accepts and an approval
/// may have accepted on a status slot (price text on most of them leaves most listings without an
/// availability, on a status slot too), and an identity that is the page URL the listings were
/// captured under; the title source of every schema; and the [title hints](title_hints) the agents
/// judge. A page that could not be fetched is left out; with none fetched the audit finds nothing.
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
    let page_findings = implausible_pages(
        &matrix,
        PriceTextAcceptance::ShortOfMostPages,
        link_identity,
    )
    .iter()
    .flat_map(blocking_codes)
    .collect::<Vec<_>>();
    let title_sources = live_schemas
        .iter()
        .filter_map(|schema| schema.title.as_ref().and_then(TitleSource::of))
        .map(|source| format!("title {}", source.code()));
    let title_hints = title_hints::title_hints(&matrix)
        .into_iter()
        .map(str::to_owned);
    SchemaAuditFindings(
        page_findings
            .into_iter()
            .chain(title_sources)
            .chain(title_hints)
            .collect(),
    )
}

/// The `field CHECK` codes of the findings that keep `page` out of approval.
fn blocking_codes(page: &ImplausiblePage) -> Vec<String> {
    let per_page = page
        .plausibility_findings
        .as_slice()
        .iter()
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

/// Whether `code` is a hint for the agents to judge rather than a finding the guard refuses.
fn is_hint(code: &str) -> bool {
    [
        TitleSource::DocumentTitle,
        TitleSource::MetaTitle,
        TitleSource::ImageAlt,
    ]
    .iter()
    .any(|source| code == format!("title {}", source.code()))
        || [
            title_hints::HOLDS_IDENTITY,
            title_hints::SHARED_LEADING_WORDS,
        ]
        .contains(&code)
}

/// `field CHECK`, the check named by its persisted code. Every check is a unit variant persisted
/// as a `SCREAMING_SNAKE_CASE` string, which `tests::should_name_every_check_by_its_code` pins.
fn field_code(field: &str, check: &impl serde::Serialize) -> String {
    match serde_json::to_value(check) {
        Ok(serde_json::Value::String(code)) => format!("{field} {code}"),
        other => panic!("a plausibility check must persist as its code, not {other:?}"),
    }
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
            sanitize: Default::default(),
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

        /// Napoli Antic's live `state` rule reads a form button that every captured page shows,
        /// so none of its listings gets an availability (#2878).
        #[test]
        fn should_report_a_state_rule_that_reads_the_same_unresolved_value_on_every_page() {
            let sampled = vec![
                captured("131033", "Human Condition", "Valider"),
                captured("130034", "Suttree", "Valider"),
            ];

            let findings = audit_live_schemas(
                "Bauman Rare Books",
                &[schema_with_title(text("h4"))],
                &sampled,
            );

            assert_eq!(findings.codes(), vec!["state CONSTANT_UNRESOLVED_STATE"]);
        }

        /// Runtime capture accepts price text in a `state`, so one odd page reopens nothing.
        #[test]
        fn should_leave_price_text_in_the_state_of_an_odd_page_alone() {
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

        /// anticstore.com's `state` fallback reads the price block, snackersantiques.com's the
        /// list item that shows `195,-`: a `state` that reads price text on most pages leaves most
        /// listings without an availability, so the audit reopens the source and holds its new
        /// listings back, even where the same rule shows the sold label on a sold page.
        #[rstest::rstest]
        #[case::every_page(["2 800 €", "195,-", "€100.00"])]
        #[case::status_slot(["£795.00", "Sold", "1250.-"])]
        fn should_report_price_text_in_the_state_of_most_pages(#[case] states: [&str; 3]) {
            let sampled: Vec<SampledCapturedPage> = ["131033", "130034", "128871"]
                .into_iter()
                .zip(["Human Condition", "Suttree", "Blood Meridian"])
                .zip(states)
                .map(|((id, name), state)| captured(id, name, state))
                .collect();

            let findings = audit_live_schemas(
                "Bauman Rare Books",
                &[schema_with_title(text("h4"))],
                &sampled,
            );

            assert_eq!(findings.codes(), vec!["state PRICE_TEXT"]);
            assert!(findings.holds_new_listings());
            // The code names no page, so a human's decision on one sample's review accepts it
            // for every later sample of the same live schemas.
            assert!(
                findings
                    .excluding(&["state PRICE_TEXT".to_owned()])
                    .is_empty()
            );
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

        /// Burbri Antiek's heading adds the article number, which is also the product's identity.
        /// The audit names it for the agents; it blocks no approval.
        #[test]
        fn should_hint_at_a_heading_that_holds_the_product_identity() {
            let sampled: Vec<SampledCapturedPage> = ["2118031", "2118047"]
                .into_iter()
                .map(|id| {
                    let mut page = captured(id, "Antiek hek", "In stock");
                    page.html = page.html.map(|html| {
                        html.replace("<h4>Antiek hek</h4>", &format!("<h4>Antiek hek {id}</h4>"))
                    });
                    page
                })
                .collect();

            let findings =
                audit_live_schemas("Burbri Antiek", &[schema_with_title(text("h4"))], &sampled);

            assert_eq!(findings.codes(), vec![title_hints::HOLDS_IDENTITY]);
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

    #[test]
    fn should_name_every_check_by_its_code() {
        use crate::scraper::plausibility::{
            CrossPageCheck, IdentityAgreementCheck, PlausibilityCheck,
        };
        for check in PlausibilityCheck::ALL {
            assert!(!field_code("title", &check).ends_with(' '));
        }
        for check in CrossPageCheck::ALL {
            assert!(!field_code("title", &check).ends_with(' '));
        }
        for check in IdentityAgreementCheck::ALL {
            assert!(!field_code("source_listing_id", &check).ends_with(' '));
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

            assert!(
                findings(&["title READS_META_TITLE"])
                    .excluding(&accepted)
                    .is_empty()
            );
        }

        /// A shop that changed its `og:title` after the approval adds a finding the approval
        /// never saw.
        #[test]
        fn should_hold_new_listings_back_only_for_a_finding_the_guard_refuses() {
            assert!(
                !findings(&["title READS_DOCUMENT_TITLE", "title HOLDS_IDENTITY"])
                    .holds_new_listings()
            );
            assert!(!findings(&["title SHARED_LEADING_WORDS"]).holds_new_listings());
            assert!(
                findings(&["title READS_META_TITLE", "title LISTING_SOURCE_NAME"])
                    .holds_new_listings()
            );
            assert!(!findings(&[]).holds_new_listings());
        }

        #[test]
        fn should_leave_out_the_accepted_findings() {
            let accepted = vec!["title READS_META_TITLE".to_owned()];

            assert_eq!(
                findings(&["title READS_META_TITLE", "title LISTING_SOURCE_NAME"])
                    .excluding(&accepted)
                    .codes(),
                vec!["title LISTING_SOURCE_NAME"]
            );
        }

        #[test]
        fn should_not_be_accepted_when_a_finding_is_new() {
            let accepted = vec!["title READS_META_TITLE".to_owned()];

            assert!(
                !findings(&["title READS_META_TITLE", "title LISTING_SOURCE_NAME"])
                    .excluding(&accepted)
                    .is_empty()
            );
        }
    }
}
