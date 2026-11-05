use crate::{
    error::NormalizationError,
    language::{detect_language, detect_language_with_minimum_confidence},
};
use localization::{Language, Localized};
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_core::{description::Description, title::Title};
use sha2::{Digest, Sha256};
use url::Url;

/// Strict variant used only in tests — returns an error when `raw` is blank
/// rather than falling back to the URL.
pub fn normalize_source_listing_id(raw: &str) -> Result<SourceListingId, NormalizationError> {
    SourceListingId::try_from(raw).map_err(|error| match error {
        product_listing_core::source_listing_id::InvalidSourceListingId::Blank => {
            NormalizationError::SourceListingIdEmpty
        }
        error => NormalizationError::SourceListingIdInvalid(error),
    })
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Returns the normalized extracted ID when present; otherwise falls back to
/// a SHA-256 hash of the full URL string.
pub fn normalize_source_listing_id_with_url_sha_fallback(
    raw: &str,
    url: &Url,
) -> Result<SourceListingId, NormalizationError> {
    if raw.trim().is_empty() {
        SourceListingId::try_from(sha256_hex(url.as_str()))
            .map_err(NormalizationError::SourceListingIdInvalid)
    } else {
        SourceListingId::try_from(raw).map_err(NormalizationError::SourceListingIdInvalid)
    }
}

/// Returns the normalized title, or [`NormalizationError::TitleEmpty`] if no
/// title text remains, as for a blank or pictograph-only title.
pub fn normalize_title(raw: &str) -> Result<Title, NormalizationError> {
    let title = Title::from(raw);
    if title.as_ref().is_empty() {
        return Err(NormalizationError::TitleEmpty);
    }
    Ok(title)
}

/// Returns a localized title with language detection applied to its text alone.
///
/// Errors with [`NormalizationError::TitleEmpty`] if no title text remains,
/// or [`NormalizationError::TitleUnknownLanguage`] if the language
/// cannot be detected.
#[allow(dead_code)]
pub fn normalize_title_localized(
    raw: &str,
) -> Result<Localized<Language, Title>, NormalizationError> {
    localize_normalized_title(normalize_title(raw)?, None, None)
}

/// Share of the detector's confidence a text needs to decide a title's language. Antique titles
/// mostly consist of proper names, style periods and loanwords, which mislead detection below
/// this share; above it, a title in another language than its description keeps its own.
const DECISIVE_CONFIDENCE: f64 = 0.75;

/// Joins the trimmed, non-blank description fragments as paragraphs, or returns `None` when
/// every fragment is blank.
pub(crate) fn join_description_fragments(fragments: &[String]) -> Option<String> {
    let paragraphs: Vec<&str> = fragments
        .iter()
        .map(|fragment| fragment.trim())
        .filter(|fragment| !fragment.is_empty())
        .collect();
    (!paragraphs.is_empty()).then(|| paragraphs.join("\n\n"))
}

/// Localizes a title to its original language.
///
/// The first decisive detection wins: of the title alone, then of the title followed by its
/// `description`. Without one, the source's `fallback_language` breaks the tie before the
/// detector's best guess for that text.
///
/// Errors with [`NormalizationError::TitleUnknownLanguage`] when none of them yields a
/// language.
pub fn localize_normalized_title(
    title: Title,
    description: Option<&str>,
    fallback_language: Option<Language>,
) -> Result<Localized<Language, Title>, NormalizationError> {
    let text = title.as_ref();
    let text_in_context = description.map(|description| format!("{text}\n\n{description}"));
    let language = detect_language_with_minimum_confidence(text, DECISIVE_CONFIDENCE)
        .or_else(|| {
            text_in_context.as_deref().and_then(|text_in_context| {
                detect_language_with_minimum_confidence(text_in_context, DECISIVE_CONFIDENCE)
            })
        })
        .or(fallback_language)
        .or_else(|| detect_language(text_in_context.as_deref().unwrap_or(text)))
        .ok_or_else(|| NormalizationError::TitleUnknownLanguage {
            text: text.chars().take(100).collect(),
        })?;

    Ok(Localized::new(language, title))
}

/// Joins non-blank fragments with `\n\n`, detects the language of the
/// resulting text, and returns a localized description.
///
/// Returns `Ok(None)` when all fragments are blank.  Returns
/// [`NormalizationError::DescriptionUnknownLanguage`] when language detection
/// fails on the joined text and no fallback language is available.
pub fn normalize_description(
    fragments: Vec<String>,
    fallback_language: Option<Language>,
) -> Result<Option<Localized<Language, Description>>, NormalizationError> {
    let Some(joined) = join_description_fragments(&fragments) else {
        return Ok(None);
    };
    let description = Description::from(joined);
    let language = detect_language(description.as_ref())
        .or(fallback_language)
        .ok_or_else(|| NormalizationError::DescriptionUnknownLanguage {
            text: description.as_ref().chars().take(100).collect(),
        })?;

    Ok(Some(Localized::new(language, description)))
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use url::Url;

    use localization::Language;

    use super::{
        localize_normalized_title, normalize_description, normalize_source_listing_id,
        normalize_source_listing_id_with_url_sha_fallback, normalize_title,
        normalize_title_localized,
    };
    use crate::error::NormalizationError;

    // -----------------------------------------------------------------------
    // source_listing_id
    // -----------------------------------------------------------------------

    #[test]
    fn should_preserve_case_when_source_listing_id_is_provided() {
        let result = normalize_source_listing_id("PROD-001").unwrap();
        assert_eq!(result.to_string(), "PROD-001");
    }

    #[test]
    fn should_trim_outer_whitespace_when_normalizing_source_listing_id() {
        let result = normalize_source_listing_id("  PROD-001  ").unwrap();
        assert_eq!(result.to_string(), "PROD-001");
    }

    #[test]
    fn should_return_error_when_source_listing_id_is_empty() {
        let err = normalize_source_listing_id("").unwrap_err();
        assert!(matches!(err, NormalizationError::SourceListingIdEmpty));
    }

    #[test]
    fn should_return_error_when_source_listing_id_is_only_whitespace() {
        let err = normalize_source_listing_id("   ").unwrap_err();
        assert!(matches!(err, NormalizationError::SourceListingIdEmpty));
    }

    // -----------------------------------------------------------------------
    // source_listing_id — URL SHA fallback
    // -----------------------------------------------------------------------

    #[test]
    fn should_use_extracted_id_when_source_listing_id_with_sha_fallback_is_non_empty() {
        let url = Url::parse("https://example.com/products/123").unwrap();
        let result = normalize_source_listing_id_with_url_sha_fallback("PROD-001", &url)
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));
        assert_eq!(result.to_string(), "PROD-001");
    }

    #[test]
    fn should_trim_and_use_extracted_id_when_source_listing_id_with_sha_fallback_has_whitespace() {
        let url = Url::parse("https://example.com/products/123").unwrap();
        let result = normalize_source_listing_id_with_url_sha_fallback("  PROD-001  ", &url)
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));
        assert_eq!(result.to_string(), "PROD-001");
    }

    #[test]
    fn should_fall_back_to_sha256_of_url_when_source_listing_id_with_sha_fallback_is_empty() {
        let url = Url::parse("https://example.com/products/123").unwrap();
        let result = normalize_source_listing_id_with_url_sha_fallback("", &url)
            .unwrap_or_else(|error| panic!("valid fallback source listing ID: {error}"));
        assert_eq!(
            result.to_string(),
            "28c714c9f68ec26408de2fcdb45ef93e77920c3ef602cb85d57f9cd8fe5ea651"
        );
    }

    #[test]
    fn should_fall_back_to_sha256_of_url_when_source_listing_id_with_sha_fallback_is_only_whitespace()
     {
        let url = Url::parse("https://example.com/products/123").unwrap();
        let result = normalize_source_listing_id_with_url_sha_fallback("   ", &url)
            .unwrap_or_else(|error| panic!("valid fallback source listing ID: {error}"));
        assert_eq!(
            result.to_string(),
            "28c714c9f68ec26408de2fcdb45ef93e77920c3ef602cb85d57f9cd8fe5ea651"
        );
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

    #[test]
    fn should_return_error_when_title_is_only_pictographs() {
        let err = normalize_title(" 🌿✨ ★ ").unwrap_err();
        assert!(matches!(err, NormalizationError::TitleEmpty));
    }

    #[test]
    fn should_detect_language_of_normalized_decorative_title() {
        let localized = normalize_title_localized(
            "𝐄𝐢𝐧 𝐌ö𝐛𝐞𝐥, 𝐝𝐚𝐬 𝐑𝐮𝐡𝐞 𝐢𝐧 𝐃𝐞𝐢𝐧 𝐙𝐮𝐡𝐚𝐮𝐬𝐞 𝐛𝐫𝐢𝐧𝐠𝐭 🌿✨Anrichte Biedermeier Kirschbaum | 3360",
        )
        .unwrap();
        assert_eq!(localized.localization, Language::De);
        assert_eq!(
            localized.payload.as_ref(),
            "Ein Möbel, das Ruhe in Dein Zuhause bringt Anrichte Biedermeier Kirschbaum | 3360"
        );
    }

    // -----------------------------------------------------------------------
    // title (localized — includes language detection)
    // -----------------------------------------------------------------------

    #[test]
    fn should_detect_language_for_title_when_english_text() {
        let localized = normalize_title_localized("This is an antique vase from England").unwrap();
        assert_eq!(localized.localization, Language::En);
    }

    #[test]
    fn should_return_error_when_title_language_cannot_be_detected() {
        // Pure digits have no language signal — lingua returns None.
        let err = normalize_title_localized("12345").unwrap_err();
        assert!(
            matches!(err, NormalizationError::TitleUnknownLanguage { .. }),
            "expected TitleUnknownLanguage, got: {:?}",
            err
        );
    }

    // -----------------------------------------------------------------------
    // title language in the context of description and fallback language
    // -----------------------------------------------------------------------

    fn localize(
        title: &str,
        description: Option<&str>,
        fallback_language: Option<Language>,
    ) -> Language {
        let title = normalize_title(title).unwrap();
        localize_normalized_title(title, description, fallback_language)
            .unwrap()
            .localization
    }

    /// Titles of proper names, style periods and loanwords that title-only detection mislabels
    /// (as `nl`, `fr`, `fr`, `pl`, `it`, `fr`) take their language from the description context.
    #[rstest]
    #[case(
        "Dutch Louis XV barometer",
        "A Dutch Louis XV barometer with a carved and gilded oak case, the dial signed by the maker. Circa 1770, in good working condition.",
        Language::En
    )]
    #[case(
        "French Louis XVI oeil de boeuf cartel",
        "A French gilt bronze oeil de boeuf cartel clock with an enamel dial and a striking movement, late eighteenth century.",
        Language::En
    )]
    #[case(
        "French Louis XV cartel d’alcove, Jean Baptiste Baillon",
        "This small alcove clock was made in Paris by Jean Baptiste Baillon, one of the most important clockmakers of his time.",
        Language::En
    )]
    #[case(
        "Krosno Józefina kandelaar, 41 cm, wit opaline glas",
        "Mooie kandelaar van het Poolse merk Krosno, gemaakt van wit opaline glas. In goede staat, zonder schade.",
        Language::Nl
    )]
    #[case(
        "Stenen putto op bol",
        "Een stenen tuinbeeld van een putto die op een bol staat. Mooi verweerd en in goede staat.",
        Language::Nl
    )]
    #[case(
        "La Saintongeoise",
        "A rare French faience plate from the Saintonge region.",
        Language::En
    )]
    #[case("Vase", "A restored ceramic vase.", Language::En)]
    fn should_detect_title_language_with_description_when_title_is_not_decisive(
        #[case] title: &str,
        #[case] description: &str,
        #[case] expected: Language,
    ) {
        assert_eq!(localize(title, Some(description), None), expected);
    }

    /// A genuinely mixed listing keeps its decisive title language.
    #[rstest]
    #[case(
        "Antique Victorian mahogany writing desk with leather top",
        "Prachtig antiek Victoriaans mahonie schrijfbureau met een leren blad. Het bureau heeft drie laden en staat op gedraaide poten.",
        Language::En
    )]
    #[case(
        "Biedermeier Sekretär Nussbaum furniert",
        "A Biedermeier secretary veneered in walnut, Vienna, circa 1825, with a fall front and three drawers.",
        Language::De
    )]
    fn should_keep_decisive_title_language_when_description_differs(
        #[case] title: &str,
        #[case] description: &str,
        #[case] expected: Language,
    ) {
        assert_eq!(localize(title, Some(description), None), expected);
    }

    #[test]
    fn should_use_fallback_language_when_title_without_description_is_not_decisive() {
        let title = "Fine Antique Chinese Kangxi Famille Verte Porcelain Octagonal Platter";

        assert_eq!(localize(title, None, Some(Language::En)), Language::En);
    }

    #[test]
    fn should_use_fallback_language_when_title_with_description_is_not_decisive() {
        assert_eq!(
            localize(
                "Vintage Poster",
                Some("23-1/2\"18-1/4\""),
                Some(Language::De)
            ),
            Language::De
        );
    }

    #[test]
    fn should_return_error_when_neither_title_nor_description_has_a_language() {
        let title = normalize_title("12345").unwrap();
        let err = localize_normalized_title(title, Some("23-1/2\"18-1/4\""), None).unwrap_err();
        assert!(matches!(
            err,
            NormalizationError::TitleUnknownLanguage { .. }
        ));
    }

    #[test]
    fn should_keep_decisive_title_language_over_fallback_language() {
        assert_eq!(
            localize(
                "Biedermeier Sekretär Nussbaum furniert",
                None,
                Some(Language::En)
            ),
            Language::De
        );
    }

    #[test]
    fn should_use_indecisive_title_language_without_description_or_fallback_language() {
        assert_eq!(localize("Vintage Poster", None, None), Language::En);
    }

    // -----------------------------------------------------------------------
    // description
    // -----------------------------------------------------------------------

    #[test]
    fn should_return_none_when_description_fragments_are_empty() {
        let result = normalize_description(vec![], None).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn should_return_none_when_all_fragments_are_blank() {
        let result = normalize_description(vec!["  ".into(), "\t".into()], None).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn should_join_fragments_with_double_newline_when_multiple_fragments() {
        let result = normalize_description(
            vec![
                "This antique piece comes from a private English collection.".into(),
                "It was acquired during the early twentieth century by the original owner.".into(),
            ],
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            result.payload.as_ref(),
            "This antique piece comes from a private English collection.\n\nIt was acquired during the early twentieth century by the original owner."
        );
    }

    #[test]
    fn should_trim_each_fragment_when_fragments_have_surrounding_whitespace() {
        let result = normalize_description(
            vec![
                "  This antique piece comes from a private English collection.  ".into(),
                "  It was acquired during the early twentieth century by the owner.  ".into(),
            ],
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            result.payload.as_ref(),
            "This antique piece comes from a private English collection.\n\nIt was acquired during the early twentieth century by the owner."
        );
    }

    #[test]
    fn should_skip_blank_fragments_when_some_fragments_are_blank() {
        let result = normalize_description(
            vec![
                "This antique piece comes from a private English collection.".into(),
                "  ".into(),
                "It was acquired during the early twentieth century by the original owner.".into(),
            ],
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            result.payload.as_ref(),
            "This antique piece comes from a private English collection.\n\nIt was acquired during the early twentieth century by the original owner."
        );
    }

    #[test]
    fn should_return_single_paragraph_when_only_one_non_blank_fragment() {
        let result = normalize_description(
            vec![
                "This antique piece comes from a private English collection and dates to around nineteen twenty.".into(),
            ],
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            result.payload.as_ref(),
            "This antique piece comes from a private English collection and dates to around nineteen twenty."
        );
    }

    #[rstest]
    #[case(vec!["12345".into()])]
    fn should_return_error_when_description_language_cannot_be_detected(
        #[case] fragments: Vec<String>,
    ) {
        let err = normalize_description(fragments, None).unwrap_err();
        assert!(
            matches!(err, NormalizationError::DescriptionUnknownLanguage { .. }),
            "expected DescriptionUnknownLanguage, got: {:?}",
            err
        );
    }

    #[test]
    fn should_use_fallback_language_when_description_language_cannot_be_detected() {
        let result = normalize_description(vec!["23-1/2\"18-1/4\"".into()], Some(Language::En))
            .unwrap()
            .unwrap();

        assert_eq!(result.localization, Language::En);
        assert_eq!(result.payload.as_ref(), "23-1/2\"18-1/4\"");
    }
}
