use crate::Localized;
use std::collections::HashMap;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(
    Copy,
    Clone,
    Eq,
    PartialEq,
    Debug,
    Hash,
    Default,
    strum_macros::EnumIter,
    strum_macros::EnumCount,
)]
pub enum Language {
    De,
    #[default]
    En,
    Fr,
    Es,
    It,
    Zh,
    Pt,
    Pl,
    Tr,
    Nl,
    Cs,
    Ja,
    Ru,
    Ar,
}

impl Language {
    pub fn resolve<T>(
        preferred: &[Language],
        available: HashMap<Language, T>,
    ) -> Option<Localized<Language, T>> {
        let mut available = available;
        preferred
            .iter()
            .find_map(|language| {
                available
                    .remove(language)
                    .map(|payload| Localized::new(*language, payload))
            })
            .or_else(|| {
                available
                    .remove(&Language::En)
                    .map(|payload| Localized::new(Language::En, payload))
            })
            .or_else(|| {
                available
                    .remove(&Language::De)
                    .map(|payload| Localized::new(Language::De, payload))
            })
            .or_else(|| {
                available
                    .into_iter()
                    .next()
                    .map(|(language, payload)| Localized::new(language, payload))
            })
    }

    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;

        Self::iter().find(|language| language.as_str() == value)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Language::De => "de",
            Language::En => "en",
            Language::Fr => "fr",
            Language::Es => "es",
            Language::It => "it",
            Language::Zh => "zh",
            Language::Pt => "pt",
            Language::Pl => "pl",
            Language::Tr => "tr",
            Language::Nl => "nl",
            Language::Cs => "cs",
            Language::Ja => "ja",
            Language::Ru => "ru",
            Language::Ar => "ar",
        }
    }

    pub fn format_human_readable(self) -> &'static str {
        match self {
            Language::De => "German",
            Language::En => "English",
            Language::Fr => "French",
            Language::Es => "Spanish",
            Language::It => "Italian",
            Language::Zh => "Chinese (Simplified)",
            Language::Pt => "Portuguese",
            Language::Pl => "Polish",
            Language::Tr => "Turkish",
            Language::Nl => "Dutch",
            Language::Cs => "Czech",
            Language::Ja => "Japanese",
            Language::Ru => "Russian",
            Language::Ar => "Arabic",
        }
    }

    pub fn is_translation_target(self) -> bool {
        matches!(
            self,
            Language::De | Language::En | Language::Fr | Language::Es | Language::It
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_keep_iso_codes_and_resolution_fallbacks() {
        assert_eq!(Language::En.as_str(), "en");
        assert_eq!(Language::Zh.as_str(), "zh");
        assert_eq!(Some(Language::En), Language::from_code("en"));
        assert_eq!(None, Language::from_code("en-US"));

        let resolved = Language::resolve(
            &[Language::Es],
            HashMap::from([(Language::Fr, "bonjour"), (Language::En, "hello")]),
        );

        assert_eq!(resolved, Some(Localized::new(Language::En, "hello")));
    }
}
