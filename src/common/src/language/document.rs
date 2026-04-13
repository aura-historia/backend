use crate::language::record::LanguageRecord;
use crate::language::{domain::Language, record::TextRecord};
use crate::localized::Localized;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LanguageDocument {
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

impl From<Language> for LanguageDocument {
    fn from(domain: Language) -> Self {
        match domain {
            Language::De => LanguageDocument::De,
            Language::En => LanguageDocument::En,
            Language::Fr => LanguageDocument::Fr,
            Language::Es => LanguageDocument::Es,
            Language::It => LanguageDocument::It,
            Language::Zh => LanguageDocument::Zh,
            Language::Pt => LanguageDocument::Pt,
            Language::Pl => LanguageDocument::Pl,
            Language::Tr => LanguageDocument::Tr,
            Language::Nl => LanguageDocument::Nl,
            Language::Cs => LanguageDocument::Cs,
            Language::Ja => LanguageDocument::Ja,
            Language::Ru => LanguageDocument::Ru,
            Language::Ar => LanguageDocument::Ar,
        }
    }
}

impl From<LanguageRecord> for LanguageDocument {
    fn from(record: LanguageRecord) -> Self {
        match record {
            LanguageRecord::De => LanguageDocument::De,
            LanguageRecord::En => LanguageDocument::En,
            LanguageRecord::Fr => LanguageDocument::Fr,
            LanguageRecord::Es => LanguageDocument::Es,
            LanguageRecord::It => LanguageDocument::It,
            LanguageRecord::Zh => LanguageDocument::Zh,
            LanguageRecord::Pt => LanguageDocument::Pt,
            LanguageRecord::Pl => LanguageDocument::Pl,
            LanguageRecord::Tr => LanguageDocument::Tr,
            LanguageRecord::Nl => LanguageDocument::Nl,
            LanguageRecord::Cs => LanguageDocument::Cs,
            LanguageRecord::Ja => LanguageDocument::Ja,
            LanguageRecord::Ru => LanguageDocument::Ru,
            LanguageRecord::Ar => LanguageDocument::Ar,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Eq, PartialEq, Debug)]
pub struct TextDocument {
    pub text: String,
    pub language: LanguageDocument,
}

impl TextDocument {
    pub fn new(text: impl Into<String>, language: LanguageDocument) -> TextDocument {
        TextDocument {
            text: text.into(),
            language,
        }
    }
}

impl<T: Into<String>> From<Localized<Language, T>> for TextDocument {
    fn from(value: Localized<Language, T>) -> Self {
        TextDocument {
            text: value.payload.into(),
            language: value.localization.into(),
        }
    }
}

impl From<TextRecord> for TextDocument {
    fn from(record: TextRecord) -> Self {
        TextDocument {
            text: record.text,
            language: record.language.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LanguageDocument;
    use rstest::rstest;

    #[rstest]
    #[case(LanguageDocument::De, "\"DE\"")]
    #[case(LanguageDocument::En, "\"EN\"")]
    #[case(LanguageDocument::Fr, "\"FR\"")]
    #[case(LanguageDocument::Es, "\"ES\"")]
    #[case(LanguageDocument::It, "\"IT\"")]
    #[case(LanguageDocument::Zh, "\"ZH\"")]
    #[case(LanguageDocument::Pt, "\"PT\"")]
    #[case(LanguageDocument::Pl, "\"PL\"")]
    #[case(LanguageDocument::Tr, "\"TR\"")]
    #[case(LanguageDocument::Nl, "\"NL\"")]
    #[case(LanguageDocument::Cs, "\"CS\"")]
    #[case(LanguageDocument::Ja, "\"JA\"")]
    #[case(LanguageDocument::Ru, "\"RU\"")]
    #[case(LanguageDocument::Ar, "\"AR\"")]
    #[trace]
    fn should_serialize_language_in_screaming_snake_case(
        #[case] language: LanguageDocument,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&language).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"DE\"", LanguageDocument::De)]
    #[case("\"EN\"", LanguageDocument::En)]
    #[case("\"FR\"", LanguageDocument::Fr)]
    #[case("\"ES\"", LanguageDocument::Es)]
    #[case("\"IT\"", LanguageDocument::It)]
    #[case("\"ZH\"", LanguageDocument::Zh)]
    #[case("\"PT\"", LanguageDocument::Pt)]
    #[case("\"PL\"", LanguageDocument::Pl)]
    #[case("\"TR\"", LanguageDocument::Tr)]
    #[case("\"NL\"", LanguageDocument::Nl)]
    #[case("\"CS\"", LanguageDocument::Cs)]
    #[case("\"JA\"", LanguageDocument::Ja)]
    #[case("\"RU\"", LanguageDocument::Ru)]
    #[case("\"AR\"", LanguageDocument::Ar)]
    #[trace]
    fn should_deserialize_language_in_screaming_snake_case(
        #[case] language: &str,
        #[case] expected: LanguageDocument,
    ) {
        let actual = serde_json::from_str::<LanguageDocument>(language).unwrap();
        assert_eq!(actual, expected);
    }
}
