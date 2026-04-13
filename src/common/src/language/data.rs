use crate::{language::domain::Language, localized::Localized};
use serde::{Deserialize, Serialize};

// ISO 639-1
#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum LanguageData {
    #[serde(
        alias = "de-DE",
        alias = "de-AT",
        alias = "de-CH",
        alias = "de-LU",
        alias = "de-LI"
    )]
    De,

    #[serde(
        alias = "en-US",
        alias = "en-GB",
        alias = "en-AU",
        alias = "en-CA",
        alias = "en-NZ",
        alias = "en_IE"
    )]
    #[default]
    En,

    #[serde(
        alias = "fr-FR",
        alias = "fr-CA",
        alias = "fr-BE",
        alias = "fr-CH",
        alias = "fr-LU"
    )]
    Fr,

    #[serde(
        alias = "es-ES",
        alias = "es-MX",
        alias = "es-AR",
        alias = "es-CO",
        alias = "es-CL",
        alias = "es-PE",
        alias = "es-VE"
    )]
    Es,

    #[serde(alias = "it-IT", alias = "it-CH")]
    It,

    #[serde(alias = "zh-CN", alias = "zh-Hans")]
    Zh,

    #[serde(alias = "pt-PT", alias = "pt-BR")]
    Pt,

    #[serde(alias = "pl-PL")]
    Pl,

    #[serde(alias = "tr-TR")]
    Tr,

    #[serde(alias = "nl-NL", alias = "nl-BE")]
    Nl,

    #[serde(alias = "cs-CZ")]
    Cs,

    #[serde(alias = "ja-JP")]
    Ja,

    #[serde(alias = "ru-RU")]
    Ru,

    #[serde(alias = "ar-SA", alias = "ar-EG", alias = "ar-AE")]
    Ar,
}

impl LanguageData {
    pub fn as_str(&self) -> &'static str {
        match self {
            LanguageData::De => "de",
            LanguageData::En => "en",
            LanguageData::Fr => "fr",
            LanguageData::Es => "es",
            LanguageData::It => "it",
            LanguageData::Zh => "zh",
            LanguageData::Pt => "pt",
            LanguageData::Pl => "pl",
            LanguageData::Tr => "tr",
            LanguageData::Nl => "nl",
            LanguageData::Cs => "cs",
            LanguageData::Ja => "ja",
            LanguageData::Ru => "ru",
            LanguageData::Ar => "ar",
        }
    }
}

impl From<Language> for LanguageData {
    fn from(domain: Language) -> Self {
        match domain {
            Language::De => LanguageData::De,
            Language::En => LanguageData::En,
            Language::Fr => LanguageData::Fr,
            Language::Es => LanguageData::Es,
            Language::It => LanguageData::It,
            Language::Zh => LanguageData::Zh,
            Language::Pt => LanguageData::Pt,
            Language::Pl => LanguageData::Pl,
            Language::Tr => LanguageData::Tr,
            Language::Nl => LanguageData::Nl,
            Language::Cs => LanguageData::Cs,
            Language::Ja => LanguageData::Ja,
            Language::Ru => LanguageData::Ru,
            Language::Ar => LanguageData::Ar,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Eq, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LocalizedTextData {
    pub text: String,
    pub language: LanguageData,
}

impl LocalizedTextData {
    pub fn new(text: impl Into<String>, language: LanguageData) -> Self {
        LocalizedTextData {
            text: text.into(),
            language,
        }
    }
}

impl<T: Into<String>> From<Localized<Language, T>> for LocalizedTextData {
    fn from(value: Localized<Language, T>) -> Self {
        LocalizedTextData {
            text: value.payload.into(),
            language: value.localization.into(),
        }
    }
}

#[cfg(feature = "api")]
pub mod api {
    use crate::{
        api::{error::ApiError, error_code::BAD_QUERY_PARAMETER_VALUE},
        language::data::LanguageData,
    };
    use aws_lambda_events::query_map::QueryMap;

    pub fn extract_language_query(query: &QueryMap) -> Result<LanguageData, ApiError> {
        let language = query
            .first("language")
            .filter(|str| !str.is_empty())
            .map(|language| serde_json::from_str::<LanguageData>(&format!(r#""{language}""#)))
            .map(|language_res| {
                language_res.map_err(|err| {
                    let msg = err.to_string();
                    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE, Box::new(err))
                        .with_query_field("language")
                        .with_detail(msg)
                })
            })
            .transpose()?
            .unwrap_or_default();

        Ok(language)
    }

    #[cfg(test)]
    mod tests {
        use rstest;

        use crate::language::data::LanguageData;
        use crate::language::data::api::extract_language_query;
        use aws_lambda_events::query_map::QueryMap;
        use std::collections::HashMap;

        #[rstest::rstest]
        #[case("de", LanguageData::De)]
        #[case("de-DE", LanguageData::De)]
        #[case("en", LanguageData::En)]
        #[case("en-GB", LanguageData::En)]
        #[case("en-US", LanguageData::En)]
        #[case("fr", LanguageData::Fr)]
        #[case("es", LanguageData::Es)]
        #[case("it", LanguageData::It)]
        #[case("zh", LanguageData::Zh)]
        #[case("zh-CN", LanguageData::Zh)]
        #[case("pt", LanguageData::Pt)]
        #[case("pt-BR", LanguageData::Pt)]
        #[case("pl", LanguageData::Pl)]
        #[case("tr", LanguageData::Tr)]
        #[case("nl", LanguageData::Nl)]
        #[case("cs", LanguageData::Cs)]
        #[case("ja", LanguageData::Ja)]
        #[case("ru", LanguageData::Ru)]
        #[case("ar", LanguageData::Ar)]
        #[trace]
        fn should_extract_language_query(
            #[case] query_value: String,
            #[case] expected: LanguageData,
        ) {
            let query = QueryMap::from(HashMap::from_iter([("language".to_string(), query_value)]));

            let actual = extract_language_query(&query).unwrap();

            assert_eq!(expected, actual);
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::language::data::LocalizedTextData;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for LocalizedTextData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            LocalizedTextData {
                text: fake::faker::lorem::en::Sentence(5..20).fake_with_rng(rng),
                language: config.fake_with_rng(rng),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::language::data::LocalizedTextData;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_localized_text_data() {
            let _ = Faker.fake::<LocalizedTextData>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LanguageData;
    use rstest::rstest;

    #[rstest]
    #[case(LanguageData::De, "\"de\"")]
    #[case(LanguageData::En, "\"en\"")]
    #[case(LanguageData::Fr, "\"fr\"")]
    #[case(LanguageData::Es, "\"es\"")]
    #[case(LanguageData::It, "\"it\"")]
    #[case(LanguageData::Zh, "\"zh\"")]
    #[case(LanguageData::Pt, "\"pt\"")]
    #[case(LanguageData::Pl, "\"pl\"")]
    #[case(LanguageData::Tr, "\"tr\"")]
    #[case(LanguageData::Nl, "\"nl\"")]
    #[case(LanguageData::Cs, "\"cs\"")]
    #[case(LanguageData::Ja, "\"ja\"")]
    #[case(LanguageData::Ru, "\"ru\"")]
    #[case(LanguageData::Ar, "\"ar\"")]
    #[trace]
    fn should_serialize_language_according_to_iso_639_1(
        #[case] language: LanguageData,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&language).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"de\"", LanguageData::De)]
    #[case("\"en\"", LanguageData::En)]
    #[case("\"fr\"", LanguageData::Fr)]
    #[case("\"es\"", LanguageData::Es)]
    #[case("\"it\"", LanguageData::It)]
    #[case("\"zh\"", LanguageData::Zh)]
    #[case("\"pt\"", LanguageData::Pt)]
    #[case("\"pl\"", LanguageData::Pl)]
    #[case("\"tr\"", LanguageData::Tr)]
    #[case("\"nl\"", LanguageData::Nl)]
    #[case("\"cs\"", LanguageData::Cs)]
    #[case("\"ja\"", LanguageData::Ja)]
    #[case("\"ru\"", LanguageData::Ru)]
    #[case("\"ar\"", LanguageData::Ar)]
    #[trace]
    fn should_deserialize_language_according_to_iso_639_1(
        #[case] language: &str,
        #[case] expected: LanguageData,
    ) {
        let actual = serde_json::from_str::<LanguageData>(language).unwrap();
        assert_eq!(actual, expected);
    }
}
