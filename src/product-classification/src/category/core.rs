use common::{
    category_key::{CategoryId, CategoryKey},
    language::domain::Language,
    localized::Localized,
    string_newtype,
};
use std::collections::HashMap;
use time::OffsetDateTime;

string_newtype!(CategoryMetaName);
string_newtype!(CategoryMetaDescription);
string_newtype!(CategoryMetaKeyword);
string_newtype!(CategoryName);
string_newtype!(CategoryDescription);

#[derive(Debug, Clone, PartialEq)]
pub struct Category {
    pub category_id: CategoryId,
    pub category_key: CategoryKey,
    pub meta_name: CategoryMetaName,
    pub meta_description: CategoryMetaDescription,
    pub meta_keywords: Vec<CategoryMetaKeyword>,
    pub embedding: Vec<f32>,
    pub display_name: HashMap<Language, CategoryName>,
    pub display_description: HashMap<Language, CategoryDescription>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl Category {
    pub fn embedding_text(&self) -> String {
        format!(
            "{} [SEP] {} [SEP] {}",
            self.meta_name,
            self.meta_description,
            self.meta_keywords
                .iter()
                .map(|k| k.to_string())
                .collect::<Vec<String>>()
                .join(", ")
        )
    }

    pub fn localized(self, preferred_languages: &[Language]) -> LocalizedCategory {
        LocalizedCategory {
            category_id: self.category_id.clone(),
            category_key: self.category_key.clone(),
            display_name: Language::resolve(preferred_languages, self.display_name),
            display_description: Language::resolve(preferred_languages, self.display_description),
            created: self.created,
            updated: self.updated,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedCategory {
    pub category_id: CategoryId,
    pub category_key: CategoryKey,
    pub display_name: Option<Localized<Language, CategoryName>>,
    pub display_description: Option<Localized<Language, CategoryDescription>>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[cfg(feature = "test-data")]
pub mod faker {
    use super::*;
    use fake::{Dummy, Faker, Rng, rand::seq::IndexedRandom};
    use serde::{Deserialize, Serialize};
    use strum::EnumCount;

    static CATEGORIES_DATA: &str = include_str!(concat!(
        env!("CARGO_WORKSPACE_DIR"),
        "src/product-classification/data/categories.json"
    ));

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct CategoryTestPayload {
        category_id: String,
        category_key: String,
        meta_name: String,
        meta_description: String,
        meta_keywords: Vec<String>,
        display_name_de: String,
        display_name_en: String,
        display_name_fr: String,
        display_name_es: String,
        display_description_de: String,
        display_description_en: String,
        display_description_fr: String,
        display_description_es: String,
    }

    impl From<CategoryTestPayload> for Category {
        fn from(payload: CategoryTestPayload) -> Self {
            let mut display_name = HashMap::with_capacity(Language::COUNT);
            display_name.insert(Language::De, CategoryName(payload.display_name_de));
            display_name.insert(Language::En, CategoryName(payload.display_name_en));
            display_name.insert(Language::Fr, CategoryName(payload.display_name_fr));
            display_name.insert(Language::Es, CategoryName(payload.display_name_es));
            let mut display_description = HashMap::with_capacity(Language::COUNT);
            display_description.insert(
                Language::De,
                CategoryDescription(payload.display_description_de),
            );
            display_description.insert(
                Language::En,
                CategoryDescription(payload.display_description_en),
            );
            display_description.insert(
                Language::Fr,
                CategoryDescription(payload.display_description_fr),
            );
            display_description.insert(
                Language::Es,
                CategoryDescription(payload.display_description_es),
            );
            Category {
                category_id: payload.category_id.into(),
                category_key: payload.category_key.into(),
                meta_name: payload.meta_name.into(),
                meta_description: payload.meta_description.into(),
                meta_keywords: payload
                    .meta_keywords
                    .into_iter()
                    .map(CategoryMetaKeyword)
                    .collect(),
                embedding: fake::vec![f32; 1024],
                display_name,
                display_description,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    impl Category {
        pub fn load_categories() -> Vec<Self> {
            serde_json::from_str::<Vec<CategoryTestPayload>>(CATEGORIES_DATA)
                .expect("shouldn't fail parsing categories data")
                .into_iter()
                .map(Category::from)
                .collect()
        }
    }

    impl Dummy<Faker> for Category {
        fn dummy_with_rng<R: Rng + ?Sized>(_config: &Faker, rng: &mut R) -> Self {
            let categories = Category::load_categories();
            categories
                .choose(rng)
                .expect("shouldn't fail picking random category")
                .clone()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::category::core::Category;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_category() {
            Faker.fake::<Category>();
        }
    }
}
