use crate::category::core::Category;
use common::category_key::{CategoryId, CategoryKey};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategoryRecord {
    pub pk: String,
    pub sk: String,

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

pub fn mk_pk() -> &'static str {
    "global#categories"
}

pub fn mk_sk(category_id: &CategoryId) -> String {
    format!("category#{category_id}")
}

impl From<Category> for CategoryRecord {
    fn from(category: Category) -> Self {
        Self {
            pk: mk_pk().to_string(),
            sk: mk_sk(&category.category_id),
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

impl From<CategoryRecord> for Category {
    fn from(record: CategoryRecord) -> Self {
        Self {
            category_id: record.category_id,
            category_key: record.category_key,
            meta_name: record.meta_name.into(),
            meta_description: record.meta_description.into(),
            meta_keywords: record.meta_keywords.into_iter().map(Into::into).collect(),
            embedding: record.embedding,
            created: record.created,
            updated: record.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for CategoryRecord {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<Category, R>(rng).into()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::category::record::CategoryRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_category_record() {
            Faker.fake::<CategoryRecord>();
        }
    }
}
