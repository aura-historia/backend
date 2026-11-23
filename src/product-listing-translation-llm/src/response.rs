//! Per-title validation of a group's answer. Each title is checked on its own, so one bad entry
//! fails only its title.

use crate::prompt::title_key;
use application::error::static_error;
use indexmap::IndexMap;
use localization::Language;
use product_listing_core::title::Title;
use product_listing_service::ports::ProductListingTitleTranslationError;
use serde::Deserialize;

/// A translation more than this many times longer than its source, plus [`LENGTH_SLACK_CHARS`],
/// is not a translation of it. Dense CJK titles expand about fourfold into Latin scripts.
const MAX_LENGTH_RATIO: usize = 8;
/// A translation less than an eighth of its source's length lost most of it.
const MIN_LENGTH_RATIO: usize = 8;
const LENGTH_SLACK_CHARS: usize = 32;

/// The answer to one group. Entries stay untyped until each title is validated alone.
#[derive(Debug, Deserialize)]
pub(crate) struct GroupResponse {
    translations: serde_json::Map<String, serde_json::Value>,
}

impl GroupResponse {
    /// Fails closed unless the entry of `position` holds a plausible, non-empty translation of
    /// `source` for every target.
    pub(crate) fn translated_title(
        &self,
        position: u32,
        source: &Title,
        target_languages: &[Language],
    ) -> Result<IndexMap<Language, Title>, ProductListingTitleTranslationError> {
        let entry = self
            .translations
            .get(&title_key(position))
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| invalid("product title translation response omitted a title"))?;
        let source_chars = source.as_ref().chars().count();
        let mut translated = IndexMap::with_capacity(target_languages.len());
        for language in target_languages {
            let raw = entry
                .get(language.as_str())
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    invalid("product title translation response omitted a target language")
                })?;
            if !plausible_length(source_chars, raw.trim().chars().count()) {
                return Err(invalid(
                    "product title translation length is implausible for its source",
                ));
            }
            let title = Title::from(raw);
            if title.as_ref().is_empty() {
                return Err(invalid(
                    "product title translation response contains an empty title",
                ));
            }
            translated.insert(*language, title);
        }
        Ok(translated)
    }
}

fn plausible_length(source_chars: usize, translated_chars: usize) -> bool {
    translated_chars > 0
        && translated_chars * MIN_LENGTH_RATIO >= source_chars
        && translated_chars <= source_chars * MAX_LENGTH_RATIO + LENGTH_SLACK_CHARS
}

fn invalid(message: &'static str) -> ProductListingTitleTranslationError {
    ProductListingTitleTranslationError::InvalidResponse {
        source: static_error(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(translations: serde_json::Value) -> GroupResponse {
        serde_json::from_value(serde_json::json!({ "translations": translations })).unwrap()
    }

    const TARGETS: [Language; 2] = [Language::En, Language::Fr];

    #[test]
    fn should_validate_each_title_on_its_own() {
        let response = response(serde_json::json!({
            "1": {"en": "Antique oak chair", "fr": "Chaise ancienne en chêne"},
            "2": {"en": "Antique table"},
            "3": {"en": " ", "fr": "Table"},
            "4": "not an object",
            "5": {"en": "Antique oak chair", "fr": "x".repeat(400)},
            "6": {"en": "a", "fr": "b"},
        }));
        let source = Title::from("Antiker Eichenstuhl");

        let first = response.translated_title(1, &source, &TARGETS).unwrap();
        assert_eq!(
            vec!["Antique oak chair", "Chaise ancienne en chêne"],
            first.values().map(AsRef::as_ref).collect::<Vec<_>>()
        );
        for position in 2..=7 {
            assert!(
                matches!(
                    response.translated_title(position, &source, &TARGETS),
                    Err(ProductListingTitleTranslationError::InvalidResponse { .. })
                ),
                "title {position} should fail"
            );
        }
    }

    #[test]
    fn should_accept_the_expansion_of_a_dense_title() {
        let source = Title::from("宋代青瓷碗");
        let response = response(serde_json::json!({
            "1": {"en": "Song dynasty celadon bowl", "fr": "Bol en céladon de la dynastie Song"},
        }));

        assert!(response.translated_title(1, &source, &TARGETS).is_ok());
    }
}
