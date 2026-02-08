use crate::category::core::Category;
use common::category_key::{CategoryId, CategoryKey};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryDocument {
    pub category_id: CategoryId,
    pub category_key: CategoryKey,
    pub meta_name: String,
    pub meta_description: String,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub meta_keywords: Vec<String>,
    pub embedding: Vec<f32>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl CategoryDocument {
    pub fn _id(&self) -> &CategoryKey {
        &self.category_key
    }
}

impl From<CategoryDocument> for Category {
    fn from(document: CategoryDocument) -> Self {
        Self {
            category_id: document.category_id,
            category_key: document.category_key,
            meta_name: document.meta_name.into(),
            meta_description: document.meta_description.into(),
            meta_keywords: document.meta_keywords.into_iter().map(Into::into).collect(),
            embedding: document.embedding,
            created: document.created,
            updated: document.updated,
        }
    }
}

impl From<Category> for CategoryDocument {
    fn from(category: Category) -> Self {
        Self {
            category_id: category.category_id,
            category_key: category.category_key,
            meta_name: category.meta_name.into(),
            meta_description: category.meta_description.into(),
            meta_keywords: category.meta_keywords.into_iter().map(Into::into).collect(),
            embedding: category.embedding,
            created: category.created,
            updated: category.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for CategoryDocument {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<Category, R>(rng).into()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::category::document::CategoryDocument;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_category_document() {
            Faker.fake::<CategoryDocument>();
        }
    }
}
