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
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

/// Most texts one field may list.
pub(crate) const MAX_SANITIZE_TEXTS: usize = 50;
/// Shortest text, in characters other than whitespace: a shorter one, such as a lone word or
/// punctuation, would also cut item text.
pub(crate) const MIN_SANITIZE_TEXT_CHARS: usize = 8;
/// Longest text, in characters.
pub(crate) const MAX_SANITIZE_TEXT_CHARS: usize = 2_000;

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

/// Per field, the shop boilerplate texts removed from that field's extracted values. A field
/// key outside [`SanitizedField`] does not decode, so a schema never stores a text that nothing
/// applies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct Sanitize(BTreeMap<SanitizedField, Vec<String>>);

impl<const N: usize> From<[(SanitizedField, Vec<String>); N]> for Sanitize {
    fn from(texts: [(SanitizedField, Vec<String>); N]) -> Self {
        Self(BTreeMap::from(texts))
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
        self.0.values().all(Vec::is_empty)
    }

    pub fn texts(&self, field: SanitizedField) -> &[String] {
        self.0.get(&field).map_or(&[], Vec::as_slice)
    }

    /// The first unusable text, in field order.
    pub(crate) fn invalid(&self) -> Option<InvalidSanitizeText> {
        SanitizedField::iter().find_map(|field| {
            invalid_texts(self.texts(field)).map(|(text, reason)| InvalidSanitizeText {
                field,
                text,
                reason,
            })
        })
    }

    /// Removes every occurrence of every text of `field` from its extracted values. A text
    /// matches with any run of whitespace, line breaks included, where it has one, and is
    /// otherwise exact. Values left blank are dropped. Fails safe: when nothing but sanitize
    /// texts would remain, the values are returned unchanged, so sanitizing never clears a
    /// field.
    pub(crate) fn apply(&self, field: SanitizedField, values: Vec<String>) -> Vec<String> {
        let patterns = self
            .texts(field)
            .iter()
            .filter_map(|text| pattern(text))
            .collect::<Vec<_>>();
        if patterns.is_empty() {
            return values;
        }
        let sanitized = values
            .iter()
            .map(|value| {
                patterns
                    .iter()
                    .fold(value.clone(), |value, pattern| remove(&value, pattern))
            })
            .filter(|value| !value.trim().is_empty())
            .collect::<Vec<_>>();
        if sanitized.is_empty() {
            return values;
        }
        sanitized
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
        return Some((String::new(), "the field lists more than 50 texts"));
    }
    for (index, text) in texts.iter().enumerate() {
        let reason =
            if text.chars().filter(|c| !c.is_whitespace()).count() < MIN_SANITIZE_TEXT_CHARS {
                Some("a text needs at least 8 characters other than whitespace")
            } else if text.chars().count() > MAX_SANITIZE_TEXT_CHARS {
                Some("a text holds at most 2000 characters")
            } else if texts[..index]
                .iter()
                .any(|earlier| whitespace_runs(earlier) == whitespace_runs(text))
            {
                Some("the text is listed twice")
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
fn pattern(text: &str) -> Option<Regex> {
    let words = text
        .split_whitespace()
        .map(regex::escape)
        .collect::<Vec<_>>();
    if words.is_empty() {
        return None;
    }
    Regex::new(&words.join(r"\s+")).ok()
}

/// Removes every match and closes the gap it leaves. A match alone on its line takes the line
/// break after it; within a line, the spaces around a match collapse to one. Other line breaks
/// stay, and a fragment without a match is returned unchanged.
fn remove(fragment: &str, pattern: &Regex) -> String {
    if !pattern.is_match(fragment) {
        return fragment.to_owned();
    }
    let mut kept = String::with_capacity(fragment.len());
    let mut rest = 0;
    for found in pattern.find_iter(fragment) {
        let before = &fragment[rest..found.start()];
        let before_trimmed = before.trim_end_matches([' ', '\t']);
        kept.push_str(before_trimmed);
        let after = &fragment[found.end()..];
        let after_trimmed = after.trim_start_matches([' ', '\t']);
        rest = fragment.len() - after_trimmed.len();
        if kept.is_empty() || kept.ends_with('\n') {
            if let Some(next_line) = after_trimmed
                .strip_prefix("\r\n")
                .or_else(|| after_trimmed.strip_prefix('\n'))
            {
                rest = fragment.len() - next_line.len();
            }
        } else if !after_trimmed.is_empty()
            && !after_trimmed.starts_with(['\n', '\r'])
            && (before_trimmed.len() < before.len() || after_trimmed.len() < after.len())
        {
            kept.push(' ');
        }
    }
    kept.push_str(&fragment[rest..]);
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
}
