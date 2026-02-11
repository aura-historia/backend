use common::language::domain::Language;
use common::query::text_query::TextQuery;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategorySearch {
    pub language: Language,
    pub name_query: Option<TextQuery<0>>,
}

impl CategorySearch {
    pub fn is_empty(&self) -> bool {
        self.name_query.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategorySearchData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_query: Option<TextQuery<0>>,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for CategorySearchData {
        fn dummy_with_rng<R: Rng + ?Sized>(_config: &Faker, _rng: &mut R) -> Self {
            CategorySearchData {
                name_query: Faker.fake(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_category_search_data() {
            let _ = Faker.fake::<CategorySearchData>();
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
            "nameQuery": "Furniture",
        });
        let expected = CategorySearchData {
            name_query: Some("Furniture".try_into().unwrap()),
        };

        let actual: CategorySearchData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_deserialize_when_empty() {
        let json = json!({});
        let expected = CategorySearchData { name_query: None };

        let actual: CategorySearchData = serde_json::from_value(json).unwrap();

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_be_empty_when_no_name_query() {
        let search = CategorySearch {
            language: Language::En,
            name_query: None,
        };
        assert!(search.is_empty());
    }

    #[test]
    fn should_not_be_empty_when_name_query_present() {
        let search = CategorySearch {
            language: Language::En,
            name_query: Some("Furniture".try_into().unwrap()),
        };
        assert!(!search.is_empty());
    }

    #[test]
    fn should_convert_from_data_to_domain() {
        let data = CategorySearchData {
            name_query: Some("Furniture".try_into().unwrap()),
        };
        let domain = CategorySearch {
            language: Language::En,
            name_query: data.name_query,
        };
        assert_eq!(domain.name_query, Some("Furniture".try_into().unwrap()));
        assert_eq!(domain.language, Language::En);
    }
}
