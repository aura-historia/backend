use crate::category::core::Category;
use common::category_key::{CategoryKey, CategorySlugId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategoryRecord {
    pub pk: String,
    pub sk: String,

    pub category_key: CategoryKey,
    pub category_slug_id: CategorySlugId,
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

pub fn mk_sk(category_key: &CategoryKey) -> String {
    format!("category#{category_key}")
}

impl From<Category> for CategoryRecord {
    fn from(category: Category) -> Self {
        Self {
            pk: mk_pk().to_string(),
            sk: mk_sk(&category.category_key),
            category_key: category.category_key,
            category_slug_id: category.category_slug_id,
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
            category_key: record.category_key,
            category_slug_id: record.category_slug_id,
            meta_name: record.meta_name.into(),
            meta_description: record.meta_description.into(),
            meta_keywords: record.meta_keywords.into_iter().map(Into::into).collect(),
            embedding: record.embedding,
            created: record.created,
            updated: record.updated,
        }
    }
}
