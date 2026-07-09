use crate::product_lifecycle::domain::ProductLifecycle;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize, Hash, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductLifecycleRecord {
    #[default]
    Active,
    Deleted,
}

impl From<ProductLifecycle> for ProductLifecycleRecord {
    fn from(domain: ProductLifecycle) -> Self {
        match domain {
            ProductLifecycle::Active => ProductLifecycleRecord::Active,
            ProductLifecycle::Deleted => ProductLifecycleRecord::Deleted,
        }
    }
}

impl From<ProductLifecycleRecord> for ProductLifecycle {
    fn from(record: ProductLifecycleRecord) -> Self {
        match record {
            ProductLifecycleRecord::Active => ProductLifecycle::Active,
            ProductLifecycleRecord::Deleted => ProductLifecycle::Deleted,
        }
    }
}
