use strum_macros::{EnumCount, EnumIter};

use crate::language::data::LanguageData;
use crate::language::document::LanguageDocument;
use crate::language::record::LanguageRecord;
use crate::localized::Localized;
use std::collections::HashMap;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Default, EnumIter, EnumCount)]
pub enum Language {
    De,
    #[default]
    En,
    Fr,
    Es,
    It,
    /// Chinese (Simplified) — ingestion-only, no translation target
    Zh,
    /// Portuguese — ingestion-only, no translation target
    Pt,
    /// Polish — ingestion-only, no translation target
    Pl,
    /// Turkish — ingestion-only, no translation target
    Tr,
    /// Dutch — ingestion-only, no translation target
    Nl,
    /// Czech — ingestion-only, no translation target
    Cs,
    /// Japanese — ingestion-only, no translation target
    Ja,
    /// Russian — ingestion-only, no translation target
    Ru,
    /// Arabic — ingestion-only, no translation target
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
            .find_map(|lang| available.remove(lang).map(|t| Localized::new(*lang, t)))
            .or_else(|| {
                available
                    .remove(&Language::En)
                    .map(|t| Localized::new(Language::En, t))
            })
            .or_else(|| {
                available
                    .remove(&Language::De)
                    .map(|t| Localized::new(Language::De, t))
            })
            .or_else(|| {
                available
                    .into_iter()
                    .next()
                    .map(|(lang, t)| Localized::new(lang, t))
            })
    }

    pub fn as_str(&self) -> &'static str {
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

    pub fn format_human_readable(&self) -> &'static str {
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

    /// Returns `true` for the five fully-supported languages that are also used
    /// as translation targets.  The remaining languages are ingestion-only: we
    /// translate *from* them but never *into* them.
    pub fn is_translation_target(&self) -> bool {
        matches!(
            self,
            Language::De | Language::En | Language::Fr | Language::Es | Language::It
        )
    }
}

impl From<LanguageRecord> for Language {
    fn from(record: LanguageRecord) -> Self {
        match record {
            LanguageRecord::De => Language::De,
            LanguageRecord::En => Language::En,
            LanguageRecord::Fr => Language::Fr,
            LanguageRecord::Es => Language::Es,
            LanguageRecord::It => Language::It,
            LanguageRecord::Zh => Language::Zh,
            LanguageRecord::Pt => Language::Pt,
            LanguageRecord::Pl => Language::Pl,
            LanguageRecord::Tr => Language::Tr,
            LanguageRecord::Nl => Language::Nl,
            LanguageRecord::Cs => Language::Cs,
            LanguageRecord::Ja => Language::Ja,
            LanguageRecord::Ru => Language::Ru,
            LanguageRecord::Ar => Language::Ar,
        }
    }
}

impl From<LanguageDocument> for Language {
    fn from(document: LanguageDocument) -> Self {
        match document {
            LanguageDocument::De => Language::De,
            LanguageDocument::En => Language::En,
            LanguageDocument::Fr => Language::Fr,
            LanguageDocument::Es => Language::Es,
            LanguageDocument::It => Language::It,
            LanguageDocument::Zh => Language::Zh,
            LanguageDocument::Pt => Language::Pt,
            LanguageDocument::Pl => Language::Pl,
            LanguageDocument::Tr => Language::Tr,
            LanguageDocument::Nl => Language::Nl,
            LanguageDocument::Cs => Language::Cs,
            LanguageDocument::Ja => Language::Ja,
            LanguageDocument::Ru => Language::Ru,
            LanguageDocument::Ar => Language::Ar,
        }
    }
}

impl From<LanguageData> for Language {
    fn from(data: LanguageData) -> Self {
        match data {
            LanguageData::De => Language::De,
            LanguageData::En => Language::En,
            LanguageData::Fr => Language::Fr,
            LanguageData::Es => Language::Es,
            LanguageData::It => Language::It,
            LanguageData::Zh => Language::Zh,
            LanguageData::Pt => Language::Pt,
            LanguageData::Pl => Language::Pl,
            LanguageData::Tr => Language::Tr,
            LanguageData::Nl => Language::Nl,
            LanguageData::Cs => Language::Cs,
            LanguageData::Ja => Language::Ja,
            LanguageData::Ru => Language::Ru,
            LanguageData::Ar => Language::Ar,
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest;

    use crate::language::domain::Language;
    use std::collections::HashMap;

    #[rstest::rstest]
    #[case::empty_defaults_english(&[], Some("English text".into()))]
    #[case::takes_preferred_from_singleton(&[Language::En], Some("English text".into()))]
    #[case::takes_preferred_from_many1(&[Language::Es, Language::Fr, Language::En], Some("Spanish text".into()))]
    #[case::takes_preferred_from_many2(&[Language::Fr, Language::De, Language::En, Language::Es], Some("French text".into()))]
    #[trace]
    fn should_respect_language_priority_when_contains_all_for_resolve(
        #[case] preferred: &[Language],
        #[case] expected: Option<String>,
    ) {
        let available = HashMap::from([
            (Language::De, "German text".to_owned()),
            (Language::En, "English text".to_owned()),
            (Language::Fr, "French text".to_owned()),
            (Language::Es, "Spanish text".to_owned()),
        ]);

        let actual = Language::resolve(preferred, available).map(|localized| localized.payload);

        assert_eq!(expected, actual);
    }

    #[rstest::rstest]
    #[case::empty_defaults_english(&[], Some("English text".into()))]
    #[case::takes_preferred_from_singleton(&[Language::En], Some("English text".into()))]
    #[case::takes_preferred_from_many(&[Language::Es, Language::Fr, Language::En], Some("French text".into()))]
    #[trace]
    fn should_respect_language_priority_when_contains_some_for_resolve(
        #[case] languages: &[Language],
        #[case] expected: Option<String>,
    ) {
        let domain = HashMap::from([
            (Language::En, "English text".to_owned()),
            (Language::Fr, "French text".to_owned()),
        ]);

        let actual = Language::resolve(languages, domain).map(|localized| localized.payload);

        assert_eq!(expected, actual);
    }

    #[rstest::rstest]
    #[case::empty_defaults_english(&[], Some("French text".into()))]
    #[case::takes_preferred_from_singleton(&[Language::En], Some("French text".into()))]
    #[case::takes_preferred_from_many(&[Language::Es, Language::En], Some("French text".into()))]
    #[trace]
    fn should_resort_to_next_best_when_contains_no_match_nor_defaults_for_resolve(
        #[case] languages: &[Language],
        #[case] expected: Option<String>,
    ) {
        let domain = HashMap::from([(Language::Fr, "French text".to_owned())]);

        let actual = Language::resolve(languages, domain).map(|localized| localized.payload);

        assert_eq!(expected, actual);
    }

    #[rstest::rstest]
    #[case(Language::De, true)]
    #[case(Language::En, true)]
    #[case(Language::Fr, true)]
    #[case(Language::Es, true)]
    #[case(Language::It, true)]
    #[case(Language::Zh, false)]
    #[case(Language::Pt, false)]
    #[case(Language::Pl, false)]
    #[case(Language::Tr, false)]
    #[case(Language::Nl, false)]
    #[case(Language::Cs, false)]
    #[case(Language::Ja, false)]
    #[case(Language::Ru, false)]
    #[case(Language::Ar, false)]
    #[trace]
    fn should_return_correct_translation_target_status_for_is_translation_target(
        #[case] language: Language,
        #[case] expected: bool,
    ) {
        assert_eq!(language.is_translation_target(), expected);
    }
}
