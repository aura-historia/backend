use crate::product_lifecycle::domain::ProductLifecycle;
use crate::product_lifecycle::record::ProductLifecycleRecord;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(
    Copy,
    Clone,
    Eq,
    PartialEq,
    Debug,
    Hash,
    Serialize,
    Deserialize,
    strum_macros::EnumCount,
    Default,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductLifecycleDocument {
    #[default]
    Active,
    Deleted,
}

impl From<ProductLifecycleRecord> for ProductLifecycleDocument {
    fn from(record: ProductLifecycleRecord) -> Self {
        match record {
            ProductLifecycleRecord::Active => ProductLifecycleDocument::Active,
            ProductLifecycleRecord::Deleted => ProductLifecycleDocument::Deleted,
        }
    }
}

impl From<ProductLifecycle> for ProductLifecycleDocument {
    fn from(value: ProductLifecycle) -> Self {
        match value {
            ProductLifecycle::Active => ProductLifecycleDocument::Active,
            ProductLifecycle::Deleted => ProductLifecycleDocument::Deleted,
        }
    }
}

impl From<ProductLifecycleDocument> for ProductLifecycle {
    fn from(value: ProductLifecycleDocument) -> Self {
        match value {
            ProductLifecycleDocument::Active => ProductLifecycle::Active,
            ProductLifecycleDocument::Deleted => ProductLifecycle::Deleted,
        }
    }
}

impl ProductLifecycleDocument {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProductLifecycleDocument::Active => "ACTIVE",
            ProductLifecycleDocument::Deleted => "DELETED",
        }
    }
}
