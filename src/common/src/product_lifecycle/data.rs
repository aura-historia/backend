use crate::product_lifecycle::domain::ProductLifecycle;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductLifecycleData {
    #[default]
    Active,
    Deleted,
}

impl From<ProductLifecycle> for ProductLifecycleData {
    fn from(domain: ProductLifecycle) -> Self {
        match domain {
            ProductLifecycle::Active => ProductLifecycleData::Active,
            ProductLifecycle::Deleted => ProductLifecycleData::Deleted,
        }
    }
}

impl From<ProductLifecycleData> for ProductLifecycle {
    fn from(data: ProductLifecycleData) -> Self {
        match data {
            ProductLifecycleData::Active => ProductLifecycle::Active,
            ProductLifecycleData::Deleted => ProductLifecycle::Deleted,
        }
    }
}
