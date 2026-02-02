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
        }
    }

    pub fn format_human_readable(&self) -> &'static str {
        match self {
            Language::De => "German",
            Language::En => "English",
            Language::Fr => "French",
            Language::Es => "Spanish",
        }
    }
}

impl From<LanguageRecord> for Language {
    fn from(record: LanguageRecord) -> Self {
        match record {
            LanguageRecord::De => Language::De,
            LanguageRecord::En => Language::En,
            LanguageRecord::Fr => Language::Fr,
            LanguageRecord::Es => Language::Es,
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
}
