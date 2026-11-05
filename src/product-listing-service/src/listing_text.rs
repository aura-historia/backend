use localization::{Language, Localized};

/// A listing text as the source published it, paired with the text to show for a request.
///
/// `localized` is chosen from the translations and the original in the order requested
/// language, `en`, `de`, then any other language, so it can be the original itself. It is
/// always present, which lets clients show it without a fallback of their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingText<T> {
    pub original: Option<Localized<Language, T>>,
    pub localized: Localized<Language, T>,
}

impl<T: Clone> ListingText<T> {
    /// Pairs the original with the selected text, or `None` when there is neither.
    ///
    /// The original stands in when no selected text is available, so a listing with an
    /// original always has a text to show.
    pub fn select(
        original: Option<Localized<Language, T>>,
        selected: Option<Localized<Language, T>>,
    ) -> Option<Self> {
        let localized = selected.or_else(|| original.clone())?;
        Some(Self {
            original,
            localized,
        })
    }
}

impl<T> ListingText<T> {
    /// A text whose original is not part of the read model, such as a search summary.
    pub fn localized(localized: Localized<Language, T>) -> Self {
        Self {
            original: None,
            localized,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(language: Language, text: &str) -> Localized<Language, String> {
        Localized::new(language, text.to_owned())
    }

    #[test]
    fn should_pair_the_original_with_the_selected_text() {
        let selected = ListingText::select(
            Some(text(Language::En, "Cabinet")),
            Some(text(Language::De, "Schrank")),
        );

        assert_eq!(
            Some(ListingText {
                original: Some(text(Language::En, "Cabinet")),
                localized: text(Language::De, "Schrank"),
            }),
            selected
        );
    }

    #[test]
    fn should_localize_to_the_original_when_no_text_is_selected() {
        let selected = ListingText::select(Some(text(Language::En, "Cabinet")), None);

        assert_eq!(
            Some(ListingText {
                original: Some(text(Language::En, "Cabinet")),
                localized: text(Language::En, "Cabinet"),
            }),
            selected
        );
    }

    #[test]
    fn should_keep_a_selected_translation_without_an_original() {
        let selected = ListingText::select(None, Some(text(Language::De, "Schrank")));

        assert_eq!(
            Some(ListingText::localized(text(Language::De, "Schrank"))),
            selected
        );
    }

    #[test]
    fn should_omit_the_text_when_there_is_neither_original_nor_selection() {
        assert_eq!(None, ListingText::<String>::select(None, None));
    }
}
