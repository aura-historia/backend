use crate::core::product_event::lifecycle::ProductLifecycleEventPayload;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductLifecycleEventTypeRecord {
    LifecycleDeleted,
}

impl ProductLifecycleEventTypeRecord {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProductLifecycleEventTypeRecord::LifecycleDeleted => "LIFECYCLE_DELETED",
        }
    }
}

impl From<&ProductLifecycleEventPayload> for ProductLifecycleEventTypeRecord {
    fn from(event: &ProductLifecycleEventPayload) -> Self {
        match event {
            ProductLifecycleEventPayload::Deleted(_) => {
                ProductLifecycleEventTypeRecord::LifecycleDeleted
            }
        }
    }
}
