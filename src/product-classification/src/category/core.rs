use common::{
    category_key::{CategoryId, CategoryKey},
    string_newtype,
};
use time::OffsetDateTime;

string_newtype!(CategoryMetaName);
string_newtype!(CategoryMetaDescription);
string_newtype!(CategoryMetaKeyword);

#[derive(Debug, Clone, PartialEq)]
pub struct Category {
    pub category_id: CategoryId,
    pub category_key: CategoryKey,
    pub meta_name: CategoryMetaName,
    pub meta_description: CategoryMetaDescription,
    pub meta_keywords: Vec<CategoryMetaKeyword>,
    pub embedding: Vec<f32>,
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
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Faker, Rng, rand::seq::IndexedRandom};
    use serde::{Deserialize, Serialize};

    static CATEGORES_DATA: &str = include_str!(concat!(
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
    }

    impl From<CategoryTestPayload> for Category {
        fn from(payload: CategoryTestPayload) -> Self {
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
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    impl Dummy<Faker> for Category {
        fn dummy_with_rng<R: Rng + ?Sized>(_config: &Faker, rng: &mut R) -> Self {
            let categories = serde_json::from_str::<Vec<CategoryTestPayload>>(CATEGORES_DATA)
                .expect("shouldn't fail parsing categories data")
                .into_iter()
                .map(Category::from)
                .collect::<Vec<_>>();
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
