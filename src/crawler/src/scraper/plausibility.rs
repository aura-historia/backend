//! Plausibility checks on raw extracted values. They inspect the strings a schema
//! extracted, from one page here and across the review pages in `cross_page`, and never
//! convert them into typed values: price parsing, currency, availability and language
//! belong to the ProductListing normalization worker, which reports what it cannot
//! normalize.

use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use crate::scraper::css_selector::rule::split_image_candidate_group;
use product_listing_core::source_listing_id::SourceListingId;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;

mod cross_page;

pub use cross_page::{CrossPageCheck, CrossPageFinding, CrossPageFindings, PageExtraction};

/// Longest plausible `state`, price or estimate, in characters after collapsing
/// whitespace runs. These fields hold labels such as "In stock", "£1,250" or
/// "POA – please contact us"; a longer value is a selector that captured a container.
const MAX_LABEL_CHARS: usize = 100;

/// Fewest words of a `source_listing_id` that lists names rather than identifying the
/// product; a label and its code, such as "Ref A12", stay below it.
const MIN_WORD_LIST_WORDS: usize = 3;

/// A raw extracted field, named as in the product schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlausibilityField {
    SourceListingId,
    Title,
    Description,
    Price,
    PriceEstimateMin,
    PriceEstimateMax,
    State,
    Images,
}

impl PlausibilityField {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourceListingId => "source_listing_id",
            Self::Title => "title",
            Self::Description => "description",
            Self::Price => "price",
            Self::PriceEstimateMin => "price_estimate_min",
            Self::PriceEstimateMax => "price_estimate_max",
            Self::State => "state",
            Self::Images => "images",
        }
    }
}

/// The check a raw extracted value failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PlausibilityCheck {
    /// Present but empty after trimming. For `source_listing_id` the capture falls back
    /// to the URL hash, so the identity changes once a later extraction succeeds.
    Blank,
    /// Longer than a label.
    TooLong,
    /// Has digits and every one is zero, as a sold page rendering `£0.00`.
    AllZeroDigits,
    /// Carries CSS rules or script source.
    MarkupTokens,
    /// The same value appears more than once.
    DuplicateValues,
    /// Cannot be captured as a source listing id.
    InvalidIdentifier,
    /// A list of names, such as a class attribute or a phrase, rather than one identifier.
    WordList,
}

impl PlausibilityCheck {
    /// Every check, so tests can pin how the review console names each code.
    #[cfg(test)]
    pub(crate) const ALL: [Self; 7] = [
        Self::Blank,
        Self::TooLong,
        Self::AllZeroDigits,
        Self::MarkupTokens,
        Self::DuplicateValues,
        Self::InvalidIdentifier,
        Self::WordList,
    ];

    /// How the reason of a rejection and the review console describe the check.
    pub(crate) const fn description(self) -> &'static str {
        match self {
            Self::Blank => "is blank",
            Self::TooLong => "is longer than a label",
            Self::AllZeroDigits => "has only zero digits",
            Self::MarkupTokens => "contains CSS or script tokens",
            Self::DuplicateValues => "repeats a value",
            Self::InvalidIdentifier => "is not a valid source listing id",
            Self::WordList => "is a list of words, not an identifier",
        }
    }
}

/// What a finding does to a cached schema candidate at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlausibilityImpact {
    /// The candidate is not used for the page.
    RejectsCandidate,
    /// The candidate ranks below every candidate with fewer such findings.
    LowersRank,
    /// The candidate is unaffected; the finding is reported to the reviewer.
    ReportOnly,
}

/// One failed check on one field of a page's raw extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlausibilityFinding {
    pub field: PlausibilityField,
    pub check: PlausibilityCheck,
}

impl PlausibilityFinding {
    const fn new(field: PlausibilityField, check: PlausibilityCheck) -> Self {
        Self { field, check }
    }

    pub const fn impact(self) -> PlausibilityImpact {
        use PlausibilityCheck::{InvalidIdentifier, MarkupTokens};
        use PlausibilityField::{
            Description, Images, Price, PriceEstimateMax, PriceEstimateMin, SourceListingId, State,
            Title,
        };
        match (self.field, self.check) {
            (_, MarkupTokens) | (Title | State, _) | (SourceListingId, InvalidIdentifier) => {
                PlausibilityImpact::RejectsCandidate
            }
            (Price | PriceEstimateMin | PriceEstimateMax | Images, _) => {
                PlausibilityImpact::LowersRank
            }
            (SourceListingId | Description, _) => PlausibilityImpact::ReportOnly,
        }
    }
}

impl fmt::Display for PlausibilityFinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "`{}` {}",
            self.field.as_str(),
            self.check.description()
        )
    }
}

/// The failed checks of one page's raw extraction, in schema field order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlausibilityFindings(Vec<PlausibilityFinding>);

impl PlausibilityFindings {
    /// Checks every field of `raw` on its own, without comparing it to other pages.
    pub fn of(raw: &RawExtractedProduct) -> Self {
        let mut findings = Vec::new();
        let mut report = |field, checks: Vec<PlausibilityCheck>| {
            findings.extend(
                checks
                    .into_iter()
                    .map(|check| PlausibilityFinding::new(field, check)),
            );
        };
        report(
            PlausibilityField::SourceListingId,
            identity_checks(&raw.source_listing_id),
        );
        report(PlausibilityField::Title, title_checks(&raw.title));
        report(
            PlausibilityField::Description,
            description_checks(&raw.description),
        );
        for (field, price) in [
            (PlausibilityField::Price, &raw.price),
            (PlausibilityField::PriceEstimateMin, &raw.price_estimate_min),
            (PlausibilityField::PriceEstimateMax, &raw.price_estimate_max),
        ] {
            report(
                field,
                price.as_deref().map(price_checks).unwrap_or_default(),
            );
        }
        report(PlausibilityField::State, state_checks(&raw.state));
        report(PlausibilityField::Images, image_checks(&raw.images));
        Self(findings)
    }

    pub fn as_slice(&self) -> &[PlausibilityFinding] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn rejects_candidate(&self) -> bool {
        self.with_impact(PlausibilityImpact::RejectsCandidate)
            .next()
            .is_some()
    }

    /// The number of findings that lower a candidate's rank.
    pub fn rank_penalty(&self) -> usize {
        self.with_impact(PlausibilityImpact::LowersRank).count()
    }

    /// Why the page's extraction is rejected, or `None` when it is not.
    pub fn rejection_reason(&self) -> Option<String> {
        let reasons = self
            .with_impact(PlausibilityImpact::RejectsCandidate)
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        (!reasons.is_empty()).then(|| format!("implausible extraction: {}", reasons.join("; ")))
    }

    fn with_impact(
        &self,
        impact: PlausibilityImpact,
    ) -> impl Iterator<Item = &PlausibilityFinding> + '_ {
        self.0
            .iter()
            .filter(move |finding| finding.impact() == impact)
    }
}

impl fmt::Display for PlausibilityFindings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, finding) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str("; ")?;
            }
            write!(formatter, "{finding}")?;
        }
        Ok(())
    }
}

/// Runs on the extracted identity, before the URL-hash fallback replaces a blank one.
fn identity_checks(source_listing_id: &str) -> Vec<PlausibilityCheck> {
    if source_listing_id.trim().is_empty() {
        vec![PlausibilityCheck::Blank]
    } else if SourceListingId::try_from(source_listing_id).is_err() {
        vec![PlausibilityCheck::InvalidIdentifier]
    } else if is_word_list(source_listing_id) {
        vec![PlausibilityCheck::WordList]
    } else {
        Vec::new()
    }
}

/// Whether `value` is a list of names rather than one identifier: at least
/// [`MIN_WORD_LIST_WORDS`] whitespace-separated words, each shaped like a CSS class name.
/// A whole class attribute reads like this, as Vauclair's `<body class>`
/// ("portfolio-template-default single single-portfolio postid-13892 …") does: it holds
/// the post id but changes with the page template and the product's tags. So does a
/// phrase such as "Price on application". An identifier is one code, number or URL, and
/// a label in front of it brings punctuation or a number ("Stock Number: 38088",
/// "Ref. 12432", "SKU #42/Blue"), so it never reads as such a list.
fn is_word_list(value: &str) -> bool {
    let words: Vec<&str> = value.split_whitespace().collect();
    words.len() >= MIN_WORD_LIST_WORDS && words.iter().all(|word| is_class_name_like(word))
}

/// An ASCII name as themes and plugins write CSS classes: a letter or `_`, optionally
/// after one `-`, then letters, digits, `-`, `_` and inner dots (`astra-4.13.3`).
fn is_class_name_like(word: &str) -> bool {
    let name = word.strip_prefix('-').unwrap_or(word);
    name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && !name.ends_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn title_checks(title: &str) -> Vec<PlausibilityCheck> {
    if title.trim().is_empty() {
        return vec![PlausibilityCheck::Blank];
    }
    markup_check(title).into_iter().collect()
}

fn description_checks(fragments: &[String]) -> Vec<PlausibilityCheck> {
    fragments
        .iter()
        .find_map(|fragment| markup_check(fragment))
        .into_iter()
        .collect()
}

fn price_checks(price: &str) -> Vec<PlausibilityCheck> {
    if price.trim().is_empty() {
        return vec![PlausibilityCheck::Blank];
    }
    [
        length_check(price),
        markup_check(price),
        has_only_zero_digits(price).then_some(PlausibilityCheck::AllZeroDigits),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn state_checks(state: &str) -> Vec<PlausibilityCheck> {
    if state.trim().is_empty() {
        return vec![PlausibilityCheck::Blank];
    }
    [length_check(state), markup_check(state)]
        .into_iter()
        .flatten()
        .collect()
}

/// Compares the primary candidate of each ordered image candidate group, so the same
/// image extracted twice repeats even when its fallback candidates differ.
fn image_checks(images: &[String]) -> Vec<PlausibilityCheck> {
    let mut seen = HashSet::new();
    let repeats_a_value = images
        .iter()
        .filter_map(|group| split_image_candidate_group(group).into_iter().next())
        .map(str::trim)
        .filter(|image| !image.is_empty())
        .any(|image| !seen.insert(image));
    repeats_a_value
        .then_some(PlausibilityCheck::DuplicateValues)
        .into_iter()
        .collect()
}

fn length_check(value: &str) -> Option<PlausibilityCheck> {
    (collapsed_char_count(value) > MAX_LABEL_CHARS).then_some(PlausibilityCheck::TooLong)
}

fn markup_check(value: &str) -> Option<PlausibilityCheck> {
    contains_markup_tokens(value).then_some(PlausibilityCheck::MarkupTokens)
}

/// Characters of `value` with each whitespace run counted as one space and outer
/// whitespace ignored, so markup indentation does not count towards a label's length.
fn collapsed_char_count(value: &str) -> usize {
    let (words, chars) = value
        .split_whitespace()
        .fold((0_usize, 0_usize), |(words, chars), word| {
            (words + 1, chars + word.chars().count())
        });
    chars + words.saturating_sub(1)
}

fn has_only_zero_digits(value: &str) -> bool {
    let mut digits = value.chars().filter(char::is_ascii_digit).peekable();
    digits.peek().is_some() && digits.all(|digit| digit == '0')
}

/// Whether an extracted value still carries CSS rules or script source, as a selector
/// produces when it targets a container holding an inline `<style>` or `<script>` block.
///
/// Description prose is checked too, and text extraction joins text nodes without
/// separators, so each pattern needs script syntax around its keyword: prose may say
/// "a dual function (desk and table)" or end a line with "window." before `<br>`.
pub(crate) fn contains_markup_tokens(value: &str) -> bool {
    contains_css_declaration_block(value)
        || contains_function_source(value)
        || value.contains("=>")
        || contains_member_access(value, "document.")
        || contains_member_access(value, "window.")
}

/// `{ ... }` holding a `property: value` declaration, so `.x{display:none}` counts while
/// prose braces such as `{rare}` do not.
fn contains_css_declaration_block(value: &str) -> bool {
    let mut rest = value;
    while let Some(open) = rest.find('{') {
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('}') else {
            return false;
        };
        if after_open[..close].contains(':') {
            return true;
        }
        rest = &after_open[close + 1..];
    }
    false
}

/// `function`, an optional name and a parameter list followed by a body, as in
/// `function(){` or `function track(event) {`; prose such as "its original function
/// (as a desk)" has no body brace after the parentheses.
fn contains_function_source(value: &str) -> bool {
    value.match_indices("function").any(|(index, keyword)| {
        let rest = skip_identifier(value[index + keyword.len()..].trim_start()).trim_start();
        let Some(parameters) = rest.strip_prefix('(') else {
            return false;
        };
        parameters
            .split_once(')')
            .is_some_and(|(_, body)| body.trim_start().starts_with('{'))
    })
}

/// `token` followed by a member name and then by script syntax (`.`, `(`, `[`, `;` or
/// an assignment), as in `window.dataLayer.push` or `document.cookie;`. Prose ends a
/// sentence there instead: `stained glass window. Circa 1880`, or `window.Height 40 cm`
/// where a `<br>` separated the lines.
fn contains_member_access(value: &str, token: &str) -> bool {
    value.match_indices(token).any(|(index, _)| {
        let rest = &value[index + token.len()..];
        let after_member = skip_identifier(rest);
        after_member.len() < rest.len()
            && (after_member.starts_with(['.', '(', '[', ';'])
                || after_member.trim_start().starts_with('='))
    })
}

/// `value` after a leading script identifier, or `value` itself when it starts with none.
fn skip_identifier(value: &str) -> &str {
    if !value.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_' || c == '$') {
        return value;
    }
    value.trim_start_matches(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

#[cfg(test)]
mod tests {
    use super::PlausibilityCheck::{
        AllZeroDigits, Blank, DuplicateValues, InvalidIdentifier, MarkupTokens, TooLong, WordList,
    };
    use super::PlausibilityField::{
        Description, Images, Price, PriceEstimateMax, SourceListingId, State, Title,
    };
    use super::*;
    use crate::scraper::css_selector::rule::IMAGE_CANDIDATE_SEPARATOR;

    fn plausible() -> RawExtractedProduct {
        RawExtractedProduct {
            source_listing_id: "SKU-42".to_owned(),
            title: "Biedermeier Chair".to_owned(),
            description: vec!["Walnut veneer, circa 1830.".to_owned()],
            price: Some("£1,250".to_owned()),
            price_estimate_min: None,
            price_estimate_max: None,
            state: "In stock".to_owned(),
            images: vec!["/a.jpg".to_owned(), "/b.jpg".to_owned()],
            raw_attributes: Default::default(),
        }
    }

    fn finding(field: PlausibilityField, check: PlausibilityCheck) -> PlausibilityFinding {
        PlausibilityFinding::new(field, check)
    }

    mod plausible_extraction {
        use super::*;

        #[test]
        fn should_report_nothing_when_every_field_is_plausible() {
            assert!(PlausibilityFindings::of(&plausible()).is_empty());
        }

        #[test]
        fn should_accept_a_free_text_state_and_a_price_on_application_label() {
            let mut raw = plausible();
            raw.state = "Viewing by appointment only.".to_owned();
            raw.price = Some(
                "POA – Please \n\n   contact us\n\nif you are interested in this product."
                    .to_owned(),
            );

            assert!(PlausibilityFindings::of(&raw).is_empty());
        }

        #[test]
        fn should_accept_an_absent_price_and_a_price_with_a_non_zero_digit() {
            let mut raw = plausible();
            raw.price = None;
            raw.price_estimate_min = Some("Regular price £0.00 Sale price £450".to_owned());

            assert!(PlausibilityFindings::of(&raw).is_empty());
        }

        #[test]
        fn should_accept_a_state_and_price_of_exactly_label_length() {
            let mut raw = plausible();
            raw.state = format!("  {}  ", "s".repeat(MAX_LABEL_CHARS));
            raw.price = Some(format!("£1{}", " 0".repeat((MAX_LABEL_CHARS - 2) / 2)));

            assert_eq!(collapsed_char_count(&raw.state), MAX_LABEL_CHARS);
            assert_eq!(
                raw.price.as_deref().map(collapsed_char_count),
                Some(MAX_LABEL_CHARS)
            );
            assert!(PlausibilityFindings::of(&raw).is_empty());
        }

        /// Text extraction joins text nodes without separators, so a `<br>` or a
        /// minified paragraph break leaves `window.Height`; prose also says
        /// "function (". Neither is script source.
        #[test]
        fn should_accept_description_prose_that_mentions_window_document_or_function() {
            let mut raw = plausible();
            raw.description = vec![
                "Leaded glass from a bay window.Height 40 cm".to_owned(),
                "Sold with the original purchase document.Provenance: private collection"
                    .to_owned(),
                "Serves a dual function (library steps and chair). Original glass {rare}"
                    .to_owned(),
                "Fits a window.Dimensions: 30 x 20 cm".to_owned(),
            ];

            assert!(PlausibilityFindings::of(&raw).is_empty());
        }

        /// Identities captured from real shops and fixtures: codes, numbers, URLs and
        /// labelled references, none a list of class-like words.
        #[rstest::rstest]
        #[case::post_id("13892")]
        #[case::leading_zero("04590")]
        #[case::prefixed_post_id("post-51040")]
        #[case::ecwid_id("p807376735")]
        #[case::dated_code("2025-0219-03")]
        #[case::range("22234-22238")]
        #[case::marhamchurch_reference("#Marh3682")]
        #[case::canonical_url("https://galerie-vauclair.fr/portfolio-item/atelier-grenet/")]
        #[case::path("/products/demetre-chiparus-venetian-lady-9685")]
        #[case::labelled_with_nbsp("Artikel-Nr:\u{a0}02092")]
        #[case::labelled_stock_number("Stock Number: 38088")]
        #[case::labelled_reference("Ref. 12432")]
        #[case::label_and_code("Ref A12")]
        #[case::dash_separated_label("Stock - A123")]
        #[case::lot_number("Lot 12 A")]
        #[case::spaced_sku("SKU  #42/Blue")]
        #[case::composite_label("Style Japonism, Chinoiserie / Ref.12432")]
        fn should_accept_identifiers_and_labelled_references(#[case] identity: &str) {
            let mut raw = plausible();
            raw.source_listing_id = identity.to_owned();

            assert!(PlausibilityFindings::of(&raw).is_empty());
        }
    }

    mod rejecting_findings {
        use super::*;

        #[test]
        fn should_reject_a_blank_title_or_state() {
            let mut raw = plausible();
            raw.title = " \n".to_owned();
            raw.state = "\u{a0}".to_owned();

            let findings = PlausibilityFindings::of(&raw);

            assert_eq!(
                findings.as_slice(),
                [finding(Title, Blank), finding(State, Blank)]
            );
            assert!(findings.rejects_candidate());
            assert_eq!(
                findings.rejection_reason().as_deref(),
                Some("implausible extraction: `title` is blank; `state` is blank")
            );
        }

        #[test]
        fn should_reject_a_state_longer_than_a_label() {
            let mut raw = plausible();
            raw.state = "word ".repeat(30);

            let findings = PlausibilityFindings::of(&raw);

            assert_eq!(findings.as_slice(), [finding(State, TooLong)]);
            assert!(findings.rejects_candidate());
        }

        #[test]
        fn should_reject_a_state_one_character_longer_than_a_label() {
            let mut raw = plausible();
            raw.state = "s".repeat(MAX_LABEL_CHARS + 1);

            assert_eq!(
                PlausibilityFindings::of(&raw).as_slice(),
                [finding(State, TooLong)]
            );
        }

        #[test]
        fn should_reject_script_source_in_a_description() {
            for script in [
                "function(){ track() }",
                "function trackView (event) { return 1 }",
                "items.forEach(item => send(item))",
                "window.dataLayer = window.dataLayer || []",
                "window.Shopify.theme",
                "document.getElementById('price')",
                "document.cookie;",
            ] {
                let mut raw = plausible();
                raw.description = vec![format!("Walnut veneer. {script}")];

                assert_eq!(
                    PlausibilityFindings::of(&raw).as_slice(),
                    [finding(Description, MarkupTokens)],
                    "{script}"
                );
            }
        }

        #[test]
        fn should_reject_inline_css_or_script_in_state_price_or_description() {
            let mut raw = plausible();
            raw.state = "Sold .badge { color: red; }".to_owned();
            raw.price = Some("POA .x{display:none}".to_owned());
            raw.description = vec!["Oak".to_owned(), "window.dataLayer.push({})".to_owned()];

            let findings = PlausibilityFindings::of(&raw);

            assert_eq!(
                findings.as_slice(),
                [
                    finding(Description, MarkupTokens),
                    finding(Price, MarkupTokens),
                    finding(State, MarkupTokens),
                ]
            );
            assert!(findings.rejects_candidate());
        }

        #[test]
        fn should_reject_an_identity_that_cannot_be_captured() {
            let mut raw = plausible();
            raw.source_listing_id = "x".repeat(513);

            let findings = PlausibilityFindings::of(&raw);

            assert_eq!(
                findings.as_slice(),
                [finding(SourceListingId, InvalidIdentifier)]
            );
            assert!(findings.rejects_candidate());
        }
    }

    mod rank_lowering_findings {
        use super::*;

        #[test]
        fn should_lower_rank_for_a_zero_price_and_a_blank_estimate() {
            let mut raw = plausible();
            raw.price = Some("£0.00".to_owned());
            raw.price_estimate_max = Some(" ".to_owned());

            let findings = PlausibilityFindings::of(&raw);

            assert_eq!(
                findings.as_slice(),
                [
                    finding(Price, AllZeroDigits),
                    finding(PriceEstimateMax, Blank)
                ]
            );
            assert!(!findings.rejects_candidate());
            assert_eq!(findings.rank_penalty(), 2);
        }

        #[test]
        fn should_lower_rank_for_a_price_longer_than_a_label() {
            let mut raw = plausible();
            raw.price = Some("£1,250 ".repeat(20));

            let findings = PlausibilityFindings::of(&raw);

            assert_eq!(findings.as_slice(), [finding(Price, TooLong)]);
            assert_eq!(findings.rank_penalty(), 1);
        }

        #[test]
        fn should_lower_rank_when_every_image_appears_twice() {
            let mut raw = plausible();
            raw.images = vec![
                "/a.jpg".to_owned(),
                "/b.jpg".to_owned(),
                "/a.jpg".to_owned(),
                "/b.jpg ".to_owned(),
            ];

            let findings = PlausibilityFindings::of(&raw);

            assert_eq!(findings.as_slice(), [finding(Images, DuplicateValues)]);
            assert_eq!(findings.rank_penalty(), 1);
        }

        #[test]
        fn should_lower_rank_when_an_image_repeats_with_other_fallback_candidates() {
            let mut raw = plausible();
            raw.images = vec![
                format!("/a.jpg{IMAGE_CANDIDATE_SEPARATOR}/a-300x300.jpg"),
                format!("/a.jpg{IMAGE_CANDIDATE_SEPARATOR}/a-600x600.jpg"),
                format!("/b.jpg{IMAGE_CANDIDATE_SEPARATOR}/a-300x300.jpg"),
            ];

            assert_eq!(
                PlausibilityFindings::of(&raw).as_slice(),
                [finding(Images, DuplicateValues)]
            );
        }
    }

    mod report_only_findings {
        use super::*;

        #[test]
        fn should_report_a_blank_identity_before_the_url_hash_fallback() {
            let mut raw = plausible();
            raw.source_listing_id = "  ".to_owned();

            let findings = PlausibilityFindings::of(&raw);

            assert_eq!(findings.as_slice(), [finding(SourceListingId, Blank)]);
            assert!(!findings.rejects_candidate());
            assert_eq!(findings.rank_penalty(), 0);
            assert_eq!(findings.rejection_reason(), None);
        }

        /// A whole class attribute differs per product through its post id and tags, so
        /// the cross-page checks pass it; its shape gives it away.
        #[rstest::rstest]
        #[case::vauclair_body_class(
            "portfolio-template-default single single-portfolio postid-13803 stretched rtl_columns av-curtain-numeric helvetica neue-websave helvetica_neue  cookies-not-set post-type-portfolio post_tag-angleterre post_tag-dragon post_tag-tabouret portfolio_entries-ceramiques portfolio_entries-mobilier portfolio_entries-nouveautes avia-responsive-images-support av-recaptcha-enabled av-google-badge-hide"
        )]
        #[case::short_body_class(
            "wp-singular products-template-default single single-products postid-51040 wp-theme-collinge-antiques"
        )]
        #[case::versioned_plugin_class(
            "wp-singular product-template-default single single-product postid-8721 wp-theme-salient woocommerce js-comp-ver-8.7.3 vc_responsive"
        )]
        #[case::three_classes("single single-product postid-8721")]
        #[case::phrase("Price on application")]
        fn should_report_an_identity_that_lists_words(#[case] identity: &str) {
            let mut raw = plausible();
            raw.source_listing_id = identity.to_owned();

            let findings = PlausibilityFindings::of(&raw);

            assert_eq!(findings.as_slice(), [finding(SourceListingId, WordList)]);
            assert!(!findings.rejects_candidate());
            assert_eq!(findings.rank_penalty(), 0);
        }
    }

    mod serialization {
        use super::*;

        #[test]
        fn should_serialize_findings_with_schema_field_names_and_stable_check_codes() {
            let mut raw = plausible();
            raw.source_listing_id = "single single-product postid-8721".to_owned();
            raw.price_estimate_min = Some("0".to_owned());

            assert_eq!(
                serde_json::to_value(PlausibilityFindings::of(&raw))
                    .unwrap_or_else(|error| panic!("findings must serialize: {error}")),
                serde_json::json!([
                    { "field": "source_listing_id", "check": "WORD_LIST" },
                    { "field": "price_estimate_min", "check": "ALL_ZERO_DIGITS" }
                ])
            );
        }
    }
}
