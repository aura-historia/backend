use super::{error::NormalizationError, language::detect_language};
use common::{language::domain::Language, localized::Localized, shops_product_id::ShopsProductId};
use product::core::{description::Description, title::Title};

pub(super) fn normalize_shops_product_id(raw: &str) -> Result<ShopsProductId, NormalizationError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(NormalizationError::ShopsProductIdEmpty);
    }
    Ok(ShopsProductId::from(trimmed))
}

pub(super) fn normalize_title(raw: &str) -> Result<Title, NormalizationError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(NormalizationError::TitleEmpty);
    }
    Ok(Title::from(trimmed))
}

pub(super) fn normalize_title_localized(
    raw: &str,
) -> Result<Localized<Language, Title>, NormalizationError> {
    let title = normalize_title(raw)?;
    let language = detect_language(title.as_ref()).ok_or_else(|| {
        NormalizationError::TitleUnknownLanguage {
            text: title.as_ref().chars().take(100).collect(),
        }
    })?;
    Ok(Localized::new(language, title))
}

/// Joins non-blank fragments with `\n\n` and detects the language of the
/// resulting text. Returns `None` when all fragments are blank.
pub(super) fn normalize_description(
    fragments: Vec<String>,
) -> Result<Option<Localized<Language, Description>>, NormalizationError> {
    let cleaned: Vec<String> = fragments
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();

    if cleaned.is_empty() {
        return Ok(None);
    }

    let joined = cleaned.join("\n\n");
    let description = Description::from(joined.as_str());
    let language = detect_language(description.as_ref()).ok_or_else(|| {
        NormalizationError::DescriptionUnknownLanguage {
            text: description.as_ref().chars().take(100).collect(),
        }
    })?;

    Ok(Some(Localized::new(language, description)))
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::{
        normalize_description, normalize_shops_product_id, normalize_title,
        normalize_title_localized,
    };
    use crate::normalization::error::NormalizationError;

    // -----------------------------------------------------------------------
    // shops_product_id
    // -----------------------------------------------------------------------

    #[test]
    fn should_normalize_shops_product_id_when_plain_string_provided() {
        let result = normalize_shops_product_id("PROD-001").unwrap();
        assert_eq!(result.to_string(), "PROD-001");
    }

    #[test]
    fn should_trim_whitespace_when_normalizing_shops_product_id() {
        let result = normalize_shops_product_id("  PROD-001  ").unwrap();
        assert_eq!(result.to_string(), "PROD-001");
    }

    #[test]
    fn should_return_error_when_shops_product_id_is_empty() {
        let err = normalize_shops_product_id("").unwrap_err();
        assert!(matches!(err, NormalizationError::ShopsProductIdEmpty));
    }

    #[test]
    fn should_return_error_when_shops_product_id_is_only_whitespace() {
        let err = normalize_shops_product_id("   ").unwrap_err();
        assert!(matches!(err, NormalizationError::ShopsProductIdEmpty));
    }

    // -----------------------------------------------------------------------
    // title (raw — no language detection)
    // -----------------------------------------------------------------------

    #[test]
    fn should_normalize_title_when_plain_string_provided() {
        // normalize_title only trims and capitalises — no language detection.
        let title = normalize_title("Antique Vase").unwrap();
        assert_eq!(title.as_ref(), "Antique Vase");
    }

    #[test]
    fn should_capitalize_first_letter_when_title_starts_lowercase() {
        let title = normalize_title("antique vase").unwrap();
        assert_eq!(&title.as_ref()[..1], "A");
    }

    #[test]
    fn should_trim_whitespace_when_normalizing_title() {
        let title = normalize_title("  Antique Vase  ").unwrap();
        assert_eq!(title.as_ref(), "Antique Vase");
    }

    #[test]
    fn should_return_error_when_title_is_empty() {
        let err = normalize_title("").unwrap_err();
        assert!(matches!(err, NormalizationError::TitleEmpty));
    }

    #[test]
    fn should_return_error_when_title_is_only_whitespace() {
        let err = normalize_title("   ").unwrap_err();
        assert!(matches!(err, NormalizationError::TitleEmpty));
    }

    // -----------------------------------------------------------------------
    // title (localized — includes language detection)
    // -----------------------------------------------------------------------

    #[test]
    fn should_detect_language_for_title_when_english_text() {
        use common::language::domain::Language;
        let localized = normalize_title_localized("This is an antique vase from England").unwrap();
        assert_eq!(localized.localization, Language::En);
    }

    #[test]
    fn should_return_error_when_title_language_cannot_be_detected() {
        // A single character cannot be language-detected reliably.
        let err = normalize_title_localized("X").unwrap_err();
        assert!(
            matches!(err, NormalizationError::TitleUnknownLanguage { .. }),
            "expected TitleUnknownLanguage, got: {:?}",
            err
        );
    }

    // -----------------------------------------------------------------------
    // description
    // -----------------------------------------------------------------------

    #[test]
    fn should_return_none_when_description_fragments_are_empty() {
        let result = normalize_description(vec![]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn should_return_none_when_all_fragments_are_blank() {
        let result = normalize_description(vec!["  ".into(), "\t".into()]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn should_join_fragments_with_double_newline_when_multiple_fragments() {
        let result = normalize_description(vec![
            "This antique piece comes from a private English collection.".into(),
            "It was acquired during the early twentieth century by the original owner.".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            result.payload.as_ref(),
            "This antique piece comes from a private English collection.\n\nIt was acquired during the early twentieth century by the original owner."
        );
    }

    #[test]
    fn should_trim_each_fragment_when_fragments_have_surrounding_whitespace() {
        let result = normalize_description(vec![
            "  This antique piece comes from a private English collection.  ".into(),
            "  It was acquired during the early twentieth century by the owner.  ".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            result.payload.as_ref(),
            "This antique piece comes from a private English collection.\n\nIt was acquired during the early twentieth century by the owner."
        );
    }

    #[test]
    fn should_skip_blank_fragments_when_some_fragments_are_blank() {
        let result = normalize_description(vec![
            "This antique piece comes from a private English collection.".into(),
            "  ".into(),
            "It was acquired during the early twentieth century by the original owner.".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            result.payload.as_ref(),
            "This antique piece comes from a private English collection.\n\nIt was acquired during the early twentieth century by the original owner."
        );
    }

    #[test]
    fn should_return_single_paragraph_when_only_one_non_blank_fragment() {
        let result = normalize_description(vec![
            "This antique piece comes from a private English collection and dates to around nineteen twenty.".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            result.payload.as_ref(),
            "This antique piece comes from a private English collection and dates to around nineteen twenty."
        );
    }

    #[rstest]
    #[case(vec!["X".into()])]
    fn should_return_error_when_description_language_cannot_be_detected(
        #[case] fragments: Vec<String>,
    ) {
        let err = normalize_description(fragments).unwrap_err();
        assert!(
            matches!(err, NormalizationError::DescriptionUnknownLanguage { .. }),
            "expected DescriptionUnknownLanguage, got: {:?}",
            err
        );
    }
}
