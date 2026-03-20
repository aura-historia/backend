use common::language::{data::LanguageData, domain::Language};
use common::query::text_query::TextQuery;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeriodSearch {
    pub language: Language,
    pub name_query: Option<TextQuery<0>>,
}

impl PeriodSearch {
    pub fn is_empty(&self) -> bool {
        self.name_query.is_none()
    }
}

impl From<PeriodSearchData> for PeriodSearch {
    fn from(data: PeriodSearchData) -> Self {
        PeriodSearch {
            language: data.language.into(),
            name_query: data.name_query,
        }
    }
}

impl From<&PeriodSearchData> for PeriodSearch {
    fn from(data: &PeriodSearchData) -> Self {
        PeriodSearch {
            language: data.language.into(),
            name_query: data.name_query.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeriodSearchData {
    #[serde(default)]
    pub language: LanguageData,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_query: Option<TextQuery<0>>,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for PeriodSearchData {
        fn dummy_with_rng<R: RngExt + ?Sized>(_config: &Faker, _rng: &mut R) -> Self {
            PeriodSearchData {
                language: Faker.fake(),
                name_query: Faker.fake(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_period_search_data() {
            let _ = Faker.fake::<PeriodSearchData>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn should_deserialize_when_name_query_present() {
        let json = json!({
            "language": "en",
            "nameQuery": "Renaissance",
        });
        let expected = PeriodSearchData {
            language: LanguageData::En,
            name_query: Some("Renaissance".try_into().unwrap()),
        };

        let actual: PeriodSearchData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_deserialize_when_empty() {
        let json = json!({
            "language": "de",
        });
        let expected = PeriodSearchData {
            name_query: None,
            language: LanguageData::De,
        };

        let actual: PeriodSearchData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_deserialize_when_empty_with_default_language() {
        let json = json!({});
        let expected = PeriodSearchData {
            name_query: None,
            language: LanguageData::En,
        };

        let actual: PeriodSearchData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_be_empty_when_no_name_query() {
        let search = PeriodSearch {
            language: Language::En,
            name_query: None,
        };
        assert!(search.is_empty());
    }

    #[test]
    fn should_not_be_empty_when_name_query_present() {
        let search = PeriodSearch {
            language: Language::En,
            name_query: Some("Renaissance".try_into().unwrap()),
        };
        assert!(!search.is_empty());
    }

    #[test]
    fn should_convert_from_data_to_domain() {
        let data = PeriodSearchData {
            language: LanguageData::En,
            name_query: Some("Renaissance".try_into().unwrap()),
        };
        let domain = PeriodSearch {
            language: Language::En,
            name_query: data.name_query,
        };
        assert_eq!(domain.name_query, Some("Renaissance".try_into().unwrap()));
        assert_eq!(domain.language, Language::En);
    }
}
