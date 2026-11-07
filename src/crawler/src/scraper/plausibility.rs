//! Plausibility checks on raw extracted values. They inspect the strings a schema
//! extracted, from one page here, across the review pages in `cross_page` and across the
//! schemas of one page in `identity_agreement`, and never
//! convert them into typed values: price parsing, currency, availability and language
//! belong to the ProductListing normalization worker, which reports what it cannot
//! normalize.

use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use crate::scraper::css_selector::raw_attribute_registry::is_registered_key;
use crate::scraper::css_selector::rule::split_image_candidate_group;
use product_listing_core::source_listing_id::SourceListingId;
use regex::regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashSet;
use std::fmt;

mod cross_page;
mod identity_agreement;

pub use cross_page::{CrossPageCheck, CrossPageFinding, CrossPageFindings, PageExtraction};
pub use identity_agreement::{
    IdentityAgreementCheck, IdentityAgreementFinding, IdentityAgreementFindings,
};

/// Longest plausible `state`, price or estimate, in characters after collapsing
/// whitespace runs. These fields hold labels such as "In stock", "£1,250" or
/// "POA – please contact us"; a longer value is a selector that captured a container.
const MAX_LABEL_CHARS: usize = 100;

/// Fewest words of a `source_listing_id` that lists names rather than identifying the
/// product; a label and its code, such as "Ref A12", stay below it.
const MIN_WORD_LIST_WORDS: usize = 3;

/// A raw extracted field, named as in the product schema.
///
/// A stored name reads back through [`PlausibilityField::from_stored`]: a core field name or a
/// registered raw attribute key, and anything else is rejected rather than taken for a raw
/// attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlausibilityField {
    SourceListingId,
    Title,
    Description,
    Price,
    PriceEstimateMin,
    PriceEstimateMax,
    State,
    Images,
    /// A raw attribute, named by its registered key as the schema matrix names its rule, such
    /// as `rawMaterial`. Registered keys are camelCase `raw` names, so a stored finding never
    /// reads back as one of the snake_case fields above.
    RawAttribute(String),
}

impl PlausibilityField {
    pub fn as_str(&self) -> &str {
        match self {
            Self::SourceListingId => "source_listing_id",
            Self::Title => "title",
            Self::Description => "description",
            Self::Price => "price",
            Self::PriceEstimateMin => "price_estimate_min",
            Self::PriceEstimateMax => "price_estimate_max",
            Self::State => "state",
            Self::Images => "images",
            Self::RawAttribute(key) => key,
        }
    }

    /// The field a stored name denotes: a core field or a registered raw attribute key.
    pub fn from_stored(name: &str) -> Result<Self, UnknownPlausibilityField> {
        Ok(match name {
            "source_listing_id" => Self::SourceListingId,
            "title" => Self::Title,
            "description" => Self::Description,
            "price" => Self::Price,
            "price_estimate_min" => Self::PriceEstimateMin,
            "price_estimate_max" => Self::PriceEstimateMax,
            "state" => Self::State,
            "images" => Self::Images,
            key if is_registered_key(key) => Self::RawAttribute(key.to_owned()),
            _ => return Err(UnknownPlausibilityField(name.to_owned())),
        })
    }
}

/// A stored plausibility field name that is neither a core field nor a registered raw
/// attribute key.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is neither a plausibility field nor a registered raw attribute key")]
pub struct UnknownPlausibilityField(String);

impl Serialize for PlausibilityField {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PlausibilityField {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Self::from_stored(&name).map_err(serde::de::Error::custom)
    }
}

/// The check a raw extracted value failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PlausibilityCheck {
    /// Present but empty after trimming; for a `description` or raw attribute, each of its
    /// values. For `source_listing_id` the capture falls back to the URL hash, so the
    /// identity changes once a later extraction succeeds.
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
    /// An amount with a currency, where a label of availability or a cart or enquiry action is
    /// expected: a `state` rule that reads a price element.
    PriceText,
}

impl PlausibilityCheck {
    /// Every check, so tests can pin how the review console names each code.
    #[cfg(test)]
    pub(crate) const ALL: [Self; 8] = [
        Self::Blank,
        Self::TooLong,
        Self::AllZeroDigits,
        Self::MarkupTokens,
        Self::DuplicateValues,
        Self::InvalidIdentifier,
        Self::WordList,
        Self::PriceText,
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
            Self::PriceText => "is a price, not an availability label",
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlausibilityFinding {
    pub field: PlausibilityField,
    pub check: PlausibilityCheck,
}

impl PlausibilityFinding {
    const fn new(field: PlausibilityField, check: PlausibilityCheck) -> Self {
        Self { field, check }
    }

    pub fn impact(&self) -> PlausibilityImpact {
        use PlausibilityCheck::{InvalidIdentifier, MarkupTokens};
        use PlausibilityField::{
            Description, Images, Price, PriceEstimateMax, PriceEstimateMin, RawAttribute,
            SourceListingId, State, Title,
        };
        match (&self.field, self.check) {
            (_, MarkupTokens) | (Title | State, _) | (SourceListingId, InvalidIdentifier) => {
                PlausibilityImpact::RejectsCandidate
            }
            (Price | PriceEstimateMin | PriceEstimateMax | Images, _) => {
                PlausibilityImpact::LowersRank
            }
            (SourceListingId | Description | RawAttribute(_), _) => PlausibilityImpact::ReportOnly,
        }
    }

    /// Whether the finding keeps a schema out of approval, deterministic and a review agent's.
    /// A repeated image does not: capture keeps each image URL once, so a gallery that repeats
    /// its featured photo loses nothing, and no selector can tell that repeat from a gallery
    /// image. It still lowers the candidate's rank and shows in the schema matrix.
    pub fn blocks_approval(&self) -> bool {
        !matches!(
            (&self.field, self.check),
            (
                PlausibilityField::Images,
                PlausibilityCheck::DuplicateValues
            )
        )
    }

    /// A blank `title` or `state`: the page leaves a required value empty.
    fn is_missing_required_value(&self) -> bool {
        matches!(
            (&self.field, self.check),
            (
                PlausibilityField::Title | PlausibilityField::State,
                PlausibilityCheck::Blank
            )
        )
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
        let mut report = |field: PlausibilityField, checks: Vec<PlausibilityCheck>| {
            findings.extend(
                checks
                    .into_iter()
                    .map(|check| PlausibilityFinding::new(field.clone(), check)),
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
        for (key, values) in &raw.raw_attributes {
            report(
                PlausibilityField::RawAttribute(key.clone()),
                raw_attribute_checks(values),
            );
        }
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

    /// Whether a finding keeps the schema out of approval, as
    /// [`PlausibilityFinding::blocks_approval`] decides.
    pub fn blocks_approval(&self) -> bool {
        self.0.iter().any(|finding| finding.blocks_approval())
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

    /// The rejecting findings when each one is a blank `title` or `state`: the extraction
    /// fails only because the page leaves a required value empty where the schema found
    /// its element. `None` when nothing rejects the extraction or something else rejects
    /// it too.
    pub(crate) fn missing_required_values(&self) -> Option<Self> {
        let rejecting = self
            .with_impact(PlausibilityImpact::RejectsCandidate)
            .cloned()
            .collect::<Vec<_>>();
        (!rejecting.is_empty()
            && rejecting
                .iter()
                .all(|finding| finding.is_missing_required_value()))
        .then_some(Self(rejecting))
    }

    /// Whether a blank `title` is the only finding that rejects the extraction: every other
    /// required value, the state among them, is there.
    pub(crate) fn rejects_only_for_blank_title(&self) -> bool {
        self.missing_required_values().is_some_and(|missing| {
            missing.as_slice()
                == [PlausibilityFinding::new(
                    PlausibilityField::Title,
                    PlausibilityCheck::Blank,
                )]
        })
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
    if is_blank_list(fragments) {
        return vec![PlausibilityCheck::Blank];
    }
    fragments
        .iter()
        .find_map(|fragment| markup_check(fragment))
        .into_iter()
        .collect()
}

fn raw_attribute_checks(values: &[String]) -> Vec<PlausibilityCheck> {
    if is_blank_list(values) {
        vec![PlausibilityCheck::Blank]
    } else {
        Vec::new()
    }
}

/// Whether a list field is present and each of its values blank, as a `first` attribute
/// rule yields it when every element it matched is blank. Extraction drops a blank item of
/// an `all` rule, so a list never mixes blank and other values.
fn is_blank_list(values: &[String]) -> bool {
    !values.is_empty() && values.iter().all(|value| value.trim().is_empty())
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
    [
        length_check(state),
        markup_check(state),
        price_text_check(state),
    ]
    .into_iter()
        .flatten()
        .collect()
}

/// A `state` that names an amount with its currency, such as "£ 1,800 plus VAT" or "Price £113".
/// Availability labels and cart or enquiry actions carry no currency marker, and a bare quantity
/// such as "2 in stock" or "Only 1 left" has none either.
fn price_text_check(state: &str) -> Option<PlausibilityCheck> {
    let symbol_before = regex!(r"[£€$¥₹₩₽₺₪]\s*\d");
    let symbol_after = regex!(r"\d\s*[£€$¥₹₩₽₺₪]");
    let code_before = regex!(
        r"(?i)\b(?:eur|gbp|usd|chf|jpy|cad|aud|nzd|sek|nok|dkk|pln|czk|huf|ron)\s*\d"
    );
    let code_or_word_after = regex!(
        r"(?i)\d\s*(?:eur|gbp|usd|chf|jpy|cad|aud|nzd|sek|nok|dkk|pln|czk|huf|ron|euros?|pounds?|dollars?)\b"
    );
    let labelled = regex!(r"(?i)\b(?:price|prijs|preis|prix|precio|prezzo)\b\W{0,3}\d");
    (symbol_before.is_match(state)
        || symbol_after.is_match(state)
        || code_before.is_match(state)
        || code_or_word_after.is_match(state)
        || labelled.is_match(state))
    .then_some(PlausibilityCheck::PriceText)
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
/// Description prose is checked too, so each pattern needs script syntax around its
/// keyword: prose says "a dual function (desk and table)" or ends a sentence with
/// "window.". Text extraction starts a new line after it at `<br>` and block boundaries,
/// but inline markup (`window.<b>Height</b>`) or a missing space still runs the next
/// word into it.
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
///
/// The regex engine matches in linear time, so page text that repeats
/// `functionfunction…` cannot hold the scraper the way a name scan from every
/// occurrence would.
fn contains_function_source(value: &str) -> bool {
    regex!(r"function\s*(?:[A-Za-z_$][A-Za-z0-9_$]*)?\s*\([^)]*\)\s*\{").is_match(value)
}

/// `token` followed by a member name and then by script syntax (`.`, `(`, `[`, `;` or
/// an assignment), as in `window.dataLayer.push` or `document.cookie;`. Prose ends a
/// sentence there instead: `stained glass window. Circa 1880`, `window.` and a new line,
/// or `window.Height 40 cm` where inline markup or a missing space joined the words.
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
        AllZeroDigits, Blank, DuplicateValues, InvalidIdentifier, MarkupTokens, PriceText, TooLong,
        WordList,
    };
    use super::PlausibilityField::{
        Description, Images, Price, PriceEstimateMax, RawAttribute, SourceListingId, State, Title,
    };
    use super::*;
    use crate::scraper::css_selector::raw_attribute_registry::registered_definitions;
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

        /// Text extraction starts a new line after "window." or "document." at a `<br>`
        /// or a paragraph break, while inline markup or a missing space leaves
        /// `window.Dimensions`; prose also says "function (". None of it is script source.
        #[test]
        fn should_accept_description_prose_that_mentions_window_document_or_function() {
            let mut raw = plausible();
            raw.description = vec![
                "Leaded glass from a bay window.\nHeight 40 cm".to_owned(),
                "Sold with the original purchase document.\nProvenance: private collection"
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

    mod missing_required_values {
        use super::*;

        #[test]
        fn should_name_the_blank_title_when_it_alone_rejects_the_extraction() {
            let mut raw = plausible();
            raw.title = String::new();
            raw.images = vec!["/a.jpg".to_owned(), "/a.jpg".to_owned()];

            let missing = PlausibilityFindings::of(&raw).missing_required_values();

            assert_eq!(
                missing.as_ref().map(PlausibilityFindings::as_slice),
                Some([finding(Title, Blank)].as_slice())
            );
        }

        #[test]
        fn should_name_every_blank_required_value() {
            let mut raw = plausible();
            raw.title = " ".to_owned();
            raw.state = String::new();

            let missing = PlausibilityFindings::of(&raw).missing_required_values();

            assert_eq!(
                missing.map(|findings| findings.to_string()),
                Some("`title` is blank; `state` is blank".to_owned())
            );
        }

        #[test]
        fn should_name_nothing_when_another_finding_also_rejects_the_extraction() {
            let mut raw = plausible();
            raw.title = String::new();
            raw.state = "Sold .badge { color: red; }".to_owned();

            assert_eq!(
                PlausibilityFindings::of(&raw).missing_required_values(),
                None
            );
        }

        #[test]
        fn should_tell_a_blank_title_alone_from_other_missing_values() {
            let mut blank_title = plausible();
            blank_title.title = String::new();
            let mut blank_title_and_state = blank_title.clone();
            blank_title_and_state.state = String::new();
            let mut blank_state = plausible();
            blank_state.state = String::new();

            assert!(PlausibilityFindings::of(&blank_title).rejects_only_for_blank_title());
            assert!(
                !PlausibilityFindings::of(&blank_title_and_state).rejects_only_for_blank_title()
            );
            assert!(!PlausibilityFindings::of(&blank_state).rejects_only_for_blank_title());
            assert!(!PlausibilityFindings::of(&plausible()).rejects_only_for_blank_title());
        }

        #[test]
        fn should_name_nothing_when_nothing_rejects_the_extraction() {
            let mut raw = plausible();
            raw.source_listing_id = String::new();

            assert_eq!(
                PlausibilityFindings::of(&raw).missing_required_values(),
                None
            );
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
        fn should_reject_a_state_that_reads_a_price() {
            for price in [
                "£ 1,800 plus VAT",
                "€ 75",
                "Price £113",
                "$4,800",
                "1.250,00 EUR",
                "GBP 265",
                "75 euro",
                "Prijs: 95",
            ] {
                let mut raw = plausible();
                raw.state = price.to_owned();

                let findings = PlausibilityFindings::of(&raw);

                assert_eq!(findings.as_slice(), [finding(State, PriceText)], "{price}");
                assert!(findings.rejects_candidate(), "{price}");
            }
        }

        #[test]
        fn should_accept_availability_labels_quantities_and_cart_or_enquiry_actions_as_state() {
            for state in [
                "Sold",
                "POA",
                "Price on application",
                "Reserved",
                "In stock",
                "Only 1 left",
                "2 in stock",
                "Verkocht",
                "https://schema.org/InStock",
                "Add to cart",
                "Buy this piece",
                "Enquire by Email",
                "I'd like to buy this",
                "Enquire or Make an Offer",
            ] {
                let mut raw = plausible();
                raw.state = state.to_owned();

                assert!(PlausibilityFindings::of(&raw).is_empty(), "{state}");
            }
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
                "var track = function\n(event)\n{ send(event) }",
                "async function $load_2 ( ) {}",
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

        /// Scanning a function name from every `function` occurrence takes minutes on
        /// this text; the check must stay linear in the length of the page's text.
        #[test]
        fn should_check_a_description_that_repeats_function_in_linear_time() {
            let repeated = "function".repeat(32 * 1024);
            let mut raw = plausible();
            raw.description = vec![repeated.clone()];

            assert!(PlausibilityFindings::of(&raw).is_empty());

            raw.description = vec![format!("{repeated} track(event) {{ send(event) }}")];

            assert_eq!(
                PlausibilityFindings::of(&raw).as_slice(),
                [finding(Description, MarkupTokens)]
            );
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

        /// A `first` attribute rule whose every match is blank yields its blank, which the
        /// check reports for these optional fields without affecting selection.
        #[test]
        fn should_report_a_blank_description_and_raw_attribute() {
            let mut raw = plausible();
            raw.description = vec!["\u{a0}".to_owned()];
            raw.raw_attributes = [
                ("rawMaterial".to_owned(), vec!["Oak".to_owned()]),
                ("rawYear".to_owned(), vec![String::new()]),
            ]
            .into();

            let findings = PlausibilityFindings::of(&raw);

            assert_eq!(
                findings.as_slice(),
                [
                    finding(Description, Blank),
                    finding(RawAttribute("rawYear".to_owned()), Blank)
                ]
            );
            assert!(!findings.rejects_candidate());
            assert_eq!(findings.rank_penalty(), 0);
            assert!(findings.blocks_approval());
            assert_eq!(
                findings.to_string(),
                "`description` is blank; `rawYear` is blank"
            );
        }

        #[test]
        fn should_report_nothing_for_a_description_and_raw_attributes_left_empty() {
            let mut raw = plausible();
            raw.description = Vec::new();
            raw.raw_attributes = [("rawMaterial".to_owned(), vec!["Oak".to_owned()])].into();

            assert!(PlausibilityFindings::of(&raw).is_empty());
        }
    }

    mod approval {
        use super::*;

        #[test]
        fn should_keep_approval_open_when_only_an_image_repeats() {
            let mut raw = plausible();
            raw.images = vec![
                "/a.jpg".to_owned(),
                "/b.jpg".to_owned(),
                "/a.jpg".to_owned(),
            ];

            let findings = PlausibilityFindings::of(&raw);

            assert_eq!(findings.as_slice(), [finding(Images, DuplicateValues)]);
            assert!(!findings.blocks_approval());
        }

        #[test]
        fn should_block_approval_with_any_other_finding_beside_a_repeated_image() {
            let mut raw = plausible();
            raw.images = vec!["/a.jpg".to_owned(), "/a.jpg".to_owned()];
            raw.price = Some("£0.00".to_owned());

            assert!(PlausibilityFindings::of(&raw).blocks_approval());
        }
    }

    mod serialization {
        use super::*;

        /// Findings stored before the field name was checked on read: every core field and
        /// the raw attribute keys the schema matrix named. They must keep reading.
        #[rstest::rstest]
        #[case::identity("source_listing_id", SourceListingId)]
        #[case::title("title", Title)]
        #[case::description("description", Description)]
        #[case::price("price", Price)]
        #[case::estimate_min("price_estimate_min", PlausibilityField::PriceEstimateMin)]
        #[case::estimate_max("price_estimate_max", PriceEstimateMax)]
        #[case::state("state", State)]
        #[case::images("images", Images)]
        #[case::material("rawMaterial", RawAttribute("rawMaterial".to_owned()))]
        #[case::year("rawYear", RawAttribute("rawYear".to_owned()))]
        fn should_keep_reading_a_stored_finding(
            #[case] name: &str,
            #[case] field: PlausibilityField,
        ) {
            let stored = serde_json::json!({ "field": name, "check": "BLANK" });

            assert_eq!(
                serde_json::from_value::<PlausibilityFinding>(stored)
                    .unwrap_or_else(|error| panic!("{name} must deserialize: {error}")),
                finding(field, Blank)
            );
        }

        #[rstest::rstest]
        #[case::unknown_key("rawUnknown")]
        #[case::wrong_case("rawmaterial")]
        #[case::camel_core_field("sourceListingId")]
        #[case::empty("")]
        fn should_reject_a_stored_finding_with_an_unknown_field_name(#[case] name: &str) {
            let stored = serde_json::json!({ "field": name, "check": "BLANK" });

            let error = serde_json::from_value::<PlausibilityFinding>(stored)
                .expect_err("an unregistered name must not read as a raw attribute");

            assert!(
                error.to_string().contains("registered raw attribute key"),
                "{error}"
            );
        }

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

        /// The schema matrix, stored with its review, names a raw attribute's finding by the
        /// key its rule has in the matrix, so the review console shows it next to that rule.
        /// Every registered key reads back as that raw attribute, not as a core field.
        #[test]
        fn should_name_a_raw_attribute_by_its_key_and_read_every_field_back() {
            let core_fields = [
                SourceListingId,
                Title,
                Description,
                Price,
                PlausibilityField::PriceEstimateMin,
                PriceEstimateMax,
                State,
                Images,
            ];
            let raw_attributes = registered_definitions()
                .map(|(_, definition)| RawAttribute(definition.key.to_owned()));

            for field in core_fields.into_iter().chain(raw_attributes) {
                let finding = finding(field.clone(), Blank);
                let stored = serde_json::to_value(&finding)
                    .unwrap_or_else(|error| panic!("{field:?} must serialize: {error}"));

                assert_eq!(
                    stored,
                    serde_json::json!({ "field": field.as_str(), "check": "BLANK" })
                );
                assert_eq!(
                    serde_json::from_value::<PlausibilityFinding>(stored)
                        .unwrap_or_else(|error| panic!("{field:?} must deserialize: {error}")),
                    finding
                );
            }
        }
    }
}
