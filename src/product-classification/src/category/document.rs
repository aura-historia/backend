use crate::category::core::Category;
use common::category_key::{CategoryKey, CategorySlugId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategoryDocument {
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

impl CategoryDocument {
    pub fn _id(&self) -> &CategoryKey {
        &self.category_key
    }
}

impl From<CategoryDocument> for Category {
    fn from(document: CategoryDocument) -> Self {
        Self {
            category_key: document.category_key,
            category_slug_id: document.category_slug_id,
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
