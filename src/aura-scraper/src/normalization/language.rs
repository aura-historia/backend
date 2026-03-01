use common::language::domain::Language;

/// Detects the language of a text snippet.
///
/// Returns `None` if the language cannot be identified as one of the supported
/// languages (DE, EN, FR, ES, IT).
pub(super) fn detect_language(text: &str) -> Option<Language> {
    whatlang::detect_lang(text).and_then(|lang| match lang {
        whatlang::Lang::Deu => Some(Language::De),
        whatlang::Lang::Eng => Some(Language::En),
        whatlang::Lang::Fra => Some(Language::Fr),
        whatlang::Lang::Spa => Some(Language::Es),
        whatlang::Lang::Ita => Some(Language::It),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use common::language::domain::Language;
    use rstest::rstest;

    use super::detect_language;

    #[rstest]
    #[case(
        "This antique piece comes from a private English collection and dates to the early twentieth century.",
        Some(Language::En)
    )]
    #[case(
        "Dieses antike Stück stammt aus einer privaten deutschen Sammlung und stammt aus dem frühen zwanzigsten Jahrhundert.",
        Some(Language::De)
    )]
    #[case(
        "Cette pièce antique provient d'une collection privée française et date du début du vingtième siècle.",
        Some(Language::Fr)
    )]
    #[case(
        "Esta pieza antigua proviene de una colección privada española y data de principios del siglo veinte.",
        Some(Language::Es)
    )]
    #[case(
        "Questo pezzo antico proviene da una collezione privata italiana e risale all'inizio del ventesimo secolo.",
        Some(Language::It)
    )]
    fn should_detect_language_when_sufficient_text_provided(
        #[case] text: &str,
        #[case] expected: Option<Language>,
    ) {
        assert_eq!(detect_language(text), expected);
    }

    #[rstest]
    #[case("X")]
    #[case("")]
    fn should_return_none_when_text_is_too_short_to_detect(#[case] text: &str) {
        assert_eq!(detect_language(text), None);
    }

    #[test]
    fn should_return_none_when_language_is_not_supported() {
        // Japanese — not in our supported set
        assert_eq!(
            detect_language("日本語のテキストはサポートされていない言語の例です。"),
            None
        );
    }
}
