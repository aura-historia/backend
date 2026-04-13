use crate::language::domain::Language;
use crate::localized::Localized;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LanguageRecord {
    De,
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

impl LanguageRecord {
    pub fn as_str(&self) -> &'static str {
        match self {
            LanguageRecord::De => "de",
            LanguageRecord::En => "en",
            LanguageRecord::Fr => "fr",
            LanguageRecord::Es => "es",
            LanguageRecord::It => "it",
            LanguageRecord::Zh => "zh",
            LanguageRecord::Pt => "pt",
            LanguageRecord::Pl => "pl",
            LanguageRecord::Tr => "tr",
            LanguageRecord::Nl => "nl",
            LanguageRecord::Cs => "cs",
            LanguageRecord::Ja => "ja",
            LanguageRecord::Ru => "ru",
            LanguageRecord::Ar => "ar",
        }
    }
}

impl From<Language> for LanguageRecord {
    fn from(domain: Language) -> Self {
        match domain {
            Language::De => LanguageRecord::De,
            Language::En => LanguageRecord::En,
            Language::Fr => LanguageRecord::Fr,
            Language::Es => LanguageRecord::Es,
            Language::It => LanguageRecord::It,
            Language::Zh => LanguageRecord::Zh,
            Language::Pt => LanguageRecord::Pt,
            Language::Pl => LanguageRecord::Pl,
            Language::Tr => LanguageRecord::Tr,
            Language::Nl => LanguageRecord::Nl,
            Language::Cs => LanguageRecord::Cs,
            Language::Ja => LanguageRecord::Ja,
            Language::Ru => LanguageRecord::Ru,
            Language::Ar => LanguageRecord::Ar,
        }
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Serialize, Deserialize, Clone, Eq, PartialEq, Debug)]
pub struct TextRecord {
    pub text: String,
    pub language: LanguageRecord,
}

impl TextRecord {
    pub fn new(text: impl Into<String>, language: LanguageRecord) -> TextRecord {
        TextRecord {
            text: text.into(),
            language,
        }
    }
}

impl<T: Into<String>> From<Localized<Language, T>> for TextRecord {
    fn from(value: Localized<Language, T>) -> Self {
        TextRecord {
            text: value.payload.into(),
            language: value.localization.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LanguageRecord;
    use rstest::rstest;

    #[rstest]
    #[case(LanguageRecord::De, "\"DE\"")]
    #[case(LanguageRecord::En, "\"EN\"")]
    #[case(LanguageRecord::Fr, "\"FR\"")]
    #[case(LanguageRecord::Es, "\"ES\"")]
    #[case(LanguageRecord::It, "\"IT\"")]
    #[case(LanguageRecord::Zh, "\"ZH\"")]
    #[case(LanguageRecord::Pt, "\"PT\"")]
    #[case(LanguageRecord::Pl, "\"PL\"")]
    #[case(LanguageRecord::Tr, "\"TR\"")]
    #[case(LanguageRecord::Nl, "\"NL\"")]
    #[case(LanguageRecord::Cs, "\"CS\"")]
    #[case(LanguageRecord::Ja, "\"JA\"")]
    #[case(LanguageRecord::Ru, "\"RU\"")]
    #[case(LanguageRecord::Ar, "\"AR\"")]
    #[trace]
    fn should_serialize_language_in_screaming_snake_case(
        #[case] language: LanguageRecord,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&language).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"DE\"", LanguageRecord::De)]
    #[case("\"EN\"", LanguageRecord::En)]
    #[case("\"FR\"", LanguageRecord::Fr)]
    #[case("\"ES\"", LanguageRecord::Es)]
    #[case("\"IT\"", LanguageRecord::It)]
    #[case("\"ZH\"", LanguageRecord::Zh)]
    #[case("\"PT\"", LanguageRecord::Pt)]
    #[case("\"PL\"", LanguageRecord::Pl)]
    #[case("\"TR\"", LanguageRecord::Tr)]
    #[case("\"NL\"", LanguageRecord::Nl)]
    #[case("\"CS\"", LanguageRecord::Cs)]
    #[case("\"JA\"", LanguageRecord::Ja)]
    #[case("\"RU\"", LanguageRecord::Ru)]
    #[case("\"AR\"", LanguageRecord::Ar)]
    #[trace]
    fn should_deserialize_language_in_screaming_snake_case(
        #[case] language: &str,
        #[case] expected: LanguageRecord,
    ) {
        let actual = serde_json::from_str::<LanguageRecord>(language).unwrap();
        assert_eq!(actual, expected);
    }
}
