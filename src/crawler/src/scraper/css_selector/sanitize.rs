//! A product schema's `sanitize` texts: per schema field, shop boilerplate, such as a shipping,
//! payment or invoice notice, that the shop repeats in that field on every listing and that says
//! nothing about the item. The schema author lists them; extraction removes each from the field's
//! values. See `docs/crawler/extraction-and-review.md#sanitize`.
//!
//! Adding a field: add a [`SanitizedField`] variant and pass the field's extracted values through
//! [`Sanitize::apply`] where the schema extracts them. Validation, storage, review and the agents'
//! schema take the new key from the enum.

use crate::scraper::css_selector::rule::ExtractionError;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::OnceLock;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

/// Most texts one field may list.
pub(crate) const MAX_SANITIZE_TEXTS: usize = 20;
/// Shortest text, in characters other than whitespace: a shorter one, such as a lone word or
/// punctuation, would also cut item text.
pub(crate) const MIN_SANITIZE_TEXT_CHARS: usize = 8;
/// Longest text, in characters.
pub(crate) const MAX_SANITIZE_TEXT_CHARS: usize = 1_000;
/// Most characters of all texts of one schema together, so a candidate of a few schemas stays
/// well inside the review MCP's 48 KiB answers, which never shorten a candidate.
pub(crate) const MAX_SANITIZE_SCHEMA_CHARS: usize = 4_000;

/// A schema field whose extracted values `sanitize` cleans, named as the schema names it.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
    EnumIter,
)]
#[serde(rename_all = "snake_case")]
pub enum SanitizedField {
    Description,
}

impl SanitizedField {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Description => "description",
        }
    }
}

/// Per field, the shop boilerplate texts removed from that field's extracted values. Only
/// fields that can be sanitized are keys, so a schema never stores a text that nothing removes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct Sanitize {
    texts: BTreeMap<SanitizedField, Vec<String>>,
    /// Each field's matcher, compiled on first use and kept as long as the schema, so a scrape
    /// pass compiles it once rather than once per page.
    #[serde(skip)]
    #[schemars(skip)]
    matchers: OnceLock<BTreeMap<SanitizedField, Regex>>,
}

impl PartialEq for Sanitize {
    fn eq(&self, other: &Self) -> bool {
        self.texts == other.texts
    }
}

impl Eq for Sanitize {}

impl<const N: usize> From<[(SanitizedField, Vec<String>); N]> for Sanitize {
    fn from(texts: [(SanitizedField, Vec<String>); N]) -> Self {
        Self {
            texts: BTreeMap::from(texts),
            matchers: OnceLock::new(),
        }
    }
}

/// Why a schema's `sanitize` texts are unusable: the field, the text's start (empty when the
/// field's list as a whole is) and the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InvalidSanitizeText {
    pub(crate) field: SanitizedField,
    pub(crate) text: String,
    pub(crate) reason: &'static str,
}

impl Sanitize {
    /// No field lists a text; such a schema is stored without the key.
    pub fn is_empty(&self) -> bool {
        self.texts.values().all(Vec::is_empty)
    }

    pub fn texts(&self, field: SanitizedField) -> &[String] {
        self.texts.get(&field).map_or(&[], Vec::as_slice)
    }

    /// The first unusable text, in field order.
    pub(crate) fn invalid(&self) -> Option<InvalidSanitizeText> {
        let mut schema_chars = 0;
        SanitizedField::iter().find_map(|field| {
            let texts = self.texts(field);
            schema_chars += texts.iter().map(|text| text.chars().count()).sum::<usize>();
            let invalid = |text: String, reason| InvalidSanitizeText {
                field,
                text,
                reason,
            };
            if schema_chars > MAX_SANITIZE_SCHEMA_CHARS {
                return Some(invalid(
                    String::new(),
                    "the schema's texts hold more than 4000 characters together",
                ));
            }
            invalid_texts(texts).map(|(text, reason)| invalid(text, reason))
        })
    }

    /// Removes every occurrence of every text of `field` from its extracted values, the longest
    /// text first where texts overlap. A text matches with any run of whitespace, line breaks
    /// included, where it has one, and is otherwise exact. A value left without a letter or digit
    /// is dropped. Fails safe: when nothing but sanitize texts would remain, the values are
    /// returned unchanged, so sanitizing never clears a field.
    pub(crate) fn apply(&self, field: SanitizedField, values: Vec<String>) -> Vec<String> {
        let Some(matcher) = self.matchers.get_or_init(|| self.compile()).get(&field) else {
            return values;
        };
        let sanitized = values
            .iter()
            .filter_map(|value| {
                let removed = remove(value, matcher);
                let changed = removed != *value;
                (!removed.trim().is_empty()
                    && (!changed || removed.chars().any(char::is_alphanumeric)))
                .then_some(removed)
            })
            .collect::<Vec<_>>();
        if sanitized.is_empty() {
            return values;
        }
        sanitized
    }

    /// One alternation per field, longest text first, so a text that contains another is
    /// removed whole whatever order the author listed them in. A field whose texts do not
    /// compile, which validation refuses before a schema is stored, gets none.
    fn compile(&self) -> BTreeMap<SanitizedField, Regex> {
        self.texts
            .iter()
            .filter_map(|(field, texts)| {
                let mut patterns = texts
                    .iter()
                    .filter_map(|text| pattern(text))
                    .collect::<Vec<_>>();
                if patterns.is_empty() {
                    return None;
                }
                patterns.sort_by_key(|pattern| std::cmp::Reverse(pattern.len()));
                let alternation = patterns
                    .iter()
                    .map(|pattern| format!("(?:{pattern})"))
                    .collect::<Vec<_>>()
                    .join("|");
                Regex::new(&alternation)
                    .ok()
                    .map(|matcher| (*field, matcher))
            })
            .collect()
    }
}

impl From<InvalidSanitizeText> for ExtractionError {
    fn from(invalid: InvalidSanitizeText) -> Self {
        Self::InvalidSanitizeText {
            field: invalid.field.as_str().to_owned(),
            text: invalid.text,
            reason: invalid.reason.to_owned(),
        }
    }
}

#[cfg(feature = "test-data")]
impl fake::Dummy<fake::Faker> for Sanitize {
    fn dummy_with_rng<R: fake::RngExt + ?Sized>(_: &fake::Faker, _: &mut R) -> Self {
        Self::default()
    }
}

fn invalid_texts(texts: &[String]) -> Option<(String, &'static str)> {
    if texts.len() > MAX_SANITIZE_TEXTS {
        return Some((String::new(), "the field lists more than 20 texts"));
    }
    for (index, text) in texts.iter().enumerate() {
        let reason =
            if text.chars().filter(|c| !c.is_whitespace()).count() < MIN_SANITIZE_TEXT_CHARS {
                Some("a text needs at least 8 characters other than whitespace")
            } else if text.chars().count() > MAX_SANITIZE_TEXT_CHARS {
                Some("a text holds at most 1000 characters")
            } else if texts[..index]
                .iter()
                .any(|earlier| whitespace_runs(earlier) == whitespace_runs(text))
            {
                Some("the text is listed twice")
            } else if pattern(text).is_none_or(|pattern| Regex::new(&pattern).is_err()) {
                Some("the text cannot be matched")
            } else {
                None
            };
        if let Some(reason) = reason {
            return Some((text.chars().take(80).collect(), reason));
        }
    }
    None
}

fn whitespace_runs(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

/// A text's whitespace-tolerant literal pattern; `None` for a blank text, which validation
/// refuses before a schema is stored.
fn pattern(text: &str) -> Option<String> {
    let words = text
        .split_whitespace()
        .map(regex::escape)
        .collect::<Vec<_>>();
    (!words.is_empty()).then(|| words.join(r"\s+"))
}

/// Whitespace within a line: everything but a line break.
fn is_inline_space(character: char) -> bool {
    character.is_whitespace() && character != '\n' && character != '\r'
}

/// Removes every match and closes the gap it leaves. A match alone on its line takes the line
/// break after it; within a line, the whitespace around a match collapses to one space. Other
/// line breaks stay, and a value without a match is returned unchanged.
fn remove(value: &str, matcher: &Regex) -> String {
    if !matcher.is_match(value) {
        return value.to_owned();
    }
    let mut kept = String::with_capacity(value.len());
    let mut rest = 0;
    for found in matcher.find_iter(value) {
        let before = &value[rest..found.start()];
        let before_trimmed = before.trim_end_matches(is_inline_space);
        kept.push_str(before_trimmed);
        let after = &value[found.end()..];
        let after_trimmed = after.trim_start_matches(is_inline_space);
        rest = value.len() - after_trimmed.len();
        if kept.is_empty() || kept.ends_with('\n') {
            if let Some(next_line) = after_trimmed
                .strip_prefix("\r\n")
                .or_else(|| after_trimmed.strip_prefix('\n'))
            {
                rest = value.len() - next_line.len();
            }
        } else if !after_trimmed.is_empty()
            && !after_trimmed.starts_with(['\n', '\r'])
            && !kept.ends_with(' ')
            && (before_trimmed.len() < before.len() || after_trimmed.len() < after.len())
        {
            kept.push(' ');
        }
    }
    kept.push_str(&value[rest..]);
    kept.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn texts(values: &[&str]) -> Vec<String> {
        values.iter().map(|&value| value.to_owned()).collect()
    }

    fn description(values: &[&str]) -> Sanitize {
        Sanitize::from([(SanitizedField::Description, texts(values))])
    }

    fn sanitize(fragments: &[&str], sanitize: &[&str]) -> Vec<String> {
        description(sanitize).apply(SanitizedField::Description, texts(fragments))
    }

    fn invalid(values: &[&str]) -> Option<&'static str> {
        description(values).invalid().map(|invalid| invalid.reason)
    }

    #[test]
    fn should_remove_a_sanitize_text_inside_a_fragment() {
        assert_eq!(
            vec!["Libro antiguo, encuadernación en piel. Buen estado.".to_owned()],
            sanitize(
                &[
                    "Libro antiguo, encuadernación en piel. El envío gratuito no tiene número de seguimiento. Buen estado."
                ],
                &["El envío gratuito no tiene número de seguimiento."],
            )
        );
    }

    #[test]
    fn should_drop_a_fragment_that_is_only_a_sanitize_text() {
        assert_eq!(
            vec!["Biedermeier Kommode, Kirschbaum furniert.".to_owned()],
            sanitize(
                &[
                    "Biedermeier Kommode, Kirschbaum furniert.",
                    "Eine Rechnung mit ausgewiesener MwSt. liegt bei.",
                ],
                &["Eine Rechnung mit ausgewiesener MwSt. liegt bei."],
            )
        );
    }

    #[test]
    fn should_match_across_other_whitespace_and_line_breaks() {
        assert_eq!(
            vec!["Vase aus Meissen.\nSehr guter Zustand.".to_owned()],
            sanitize(
                &[
                    "Vase aus Meissen.\nVersand  innerhalb\nDeutschlands versichert.\nSehr guter Zustand."
                ],
                &["Versand innerhalb Deutschlands versichert."],
            )
        );
    }

    #[test]
    fn should_remove_japanese_shipping_note() {
        assert_eq!(
            vec!["伊万里焼の大皿です。状態は良好です。".to_owned()],
            sanitize(
                &["伊万里焼の大皿です。送料無料、即日発送いたします。状態は良好です。"],
                &["送料無料、即日発送いたします。"]
            )
        );
    }

    #[test]
    fn should_keep_item_facts_the_author_did_not_list() {
        let notice = "Versand innerhalb Deutschlands als Großbrief.";
        for state in ["Gelaufen.", "Nicht gelaufen mit der Post."] {
            assert_eq!(
                vec![format!("Ansichtskarte Berlin. {state}")],
                sanitize(
                    &[&format!("Ansichtskarte Berlin. {state} {notice}")],
                    &[notice]
                )
            );
        }
    }

    #[test]
    fn should_match_case_exactly() {
        assert_eq!(
            vec!["Free shipping worldwide on this vase.".to_owned()],
            sanitize(
                &["Free shipping worldwide on this vase."],
                &["free shipping worldwide"]
            )
        );
    }

    #[test]
    fn should_keep_the_description_when_only_sanitize_texts_remain() {
        assert_eq!(
            texts(&["Save on postage: combine your purchases."]),
            sanitize(
                &["Save on postage: combine your purchases."],
                &["Save on postage: combine your purchases."],
            )
        );
    }

    #[test]
    fn should_leave_fragments_unchanged_without_a_match() {
        let fragments = ["Zeile eins   \nZeile zwei"];
        assert_eq!(
            texts(&fragments),
            sanitize(&fragments, &["Versand weltweit möglich."])
        );
    }

    #[rstest]
    #[case(&["Versand"], "a text needs at least 8 characters other than whitespace")]
    #[case(&["  \n "], "a text needs at least 8 characters other than whitespace")]
    #[case(&["Versand weltweit.", "Versand  weltweit."], "the text is listed twice")]
    fn should_refuse_unusable_texts(#[case] values: &[&str], #[case] reason: &str) {
        assert_eq!(Some(reason), invalid(values));
    }

    #[test]
    fn should_refuse_overlong_and_too_many_texts() {
        let overlong = "x".repeat(MAX_SANITIZE_TEXT_CHARS + 1);
        assert!(invalid(&[&overlong]).is_some());
        let many = (0..=MAX_SANITIZE_TEXTS)
            .map(|index| format!("Versandhinweis Nummer {index}"))
            .collect::<Vec<_>>();
        let many = many.iter().map(String::as_str).collect::<Vec<_>>();
        assert!(invalid(&many).is_some());
        assert_eq!(None, invalid(&many[..MAX_SANITIZE_TEXTS]));
    }

    #[test]
    fn should_name_the_field_of_an_unusable_text() {
        assert_eq!(
            Some(InvalidSanitizeText {
                field: SanitizedField::Description,
                text: "Versand".to_owned(),
                reason: "a text needs at least 8 characters other than whitespace",
            }),
            description(&["Versand weltweit möglich.", "Versand"]).invalid()
        );
    }

    #[test]
    fn should_store_texts_by_field_and_refuse_a_field_nothing_sanitizes() {
        let sanitize = description(&["Versand weltweit möglich."]);
        let stored = serde_json::to_value(&sanitize).unwrap();
        assert_eq!(
            serde_json::json!({"description": ["Versand weltweit möglich."]}),
            stored
        );
        assert_eq!(sanitize, serde_json::from_value(stored).unwrap());
        assert!(
            serde_json::from_value::<Sanitize>(serde_json::json!({"price": ["EUR 12"]})).is_err()
        );
        assert!(description(&[]).is_empty());
    }

    #[test]
    fn should_leave_fields_without_texts_unchanged() {
        assert_eq!(
            texts(&["Versand weltweit möglich."]),
            Sanitize::default().apply(
                SanitizedField::Description,
                texts(&["Versand weltweit möglich."])
            )
        );
    }

    #[test]
    fn should_remove_the_longer_of_two_overlapping_texts_whatever_their_order() {
        let short = "Versand innerhalb Deutschlands.";
        let long = "Versand innerhalb Deutschlands. Rechnung mit MwSt. liegt bei.";
        let value = format!("Kommode. {long}");
        for order in [[short, long], [long, short]] {
            assert_eq!(vec!["Kommode.".to_owned()], sanitize(&[&value], &order));
        }
    }

    #[test]
    fn should_close_the_gap_of_repeated_texts_and_non_breaking_spaces() {
        let notice = "Versand weltweit möglich.";
        assert_eq!(
            vec!["Vase. Guter Zustand.".to_owned()],
            sanitize(
                &[&format!("Vase. {notice} {notice} Guter Zustand.")],
                &[notice]
            )
        );
        assert_eq!(
            vec!["Vase. Guter Zustand.".to_owned()],
            sanitize(
                &[&format!("Vase.\u{a0}{notice}\u{a0}Guter Zustand.")],
                &[notice]
            )
        );
    }

    #[test]
    fn should_drop_a_line_by_line_value_with_crlf_and_a_bullet_left_alone() {
        let notice = "Versand weltweit möglich.";
        assert_eq!(
            vec!["Vase.\r\nGuter Zustand.".to_owned()],
            sanitize(
                &[
                    &format!("Vase.\r\n{notice}\r\nGuter Zustand."),
                    &format!("• {notice}")
                ],
                &[notice]
            )
        );
    }

    #[test]
    fn should_refuse_texts_of_one_schema_beyond_their_total_size() {
        let texts = (0..5)
            .map(|index| format!("{index} {}", "x".repeat(MAX_SANITIZE_TEXT_CHARS - 2)))
            .collect::<Vec<_>>();
        let texts = texts.iter().map(String::as_str).collect::<Vec<_>>();
        assert_eq!(
            Some("the schema's texts hold more than 4000 characters together"),
            invalid(&texts)
        );
        assert_eq!(None, invalid(&texts[..4]));
    }
}
