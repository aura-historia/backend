use crate::resource_state::domain::ResourceState;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResourceStateDocument {
    #[default]
    Active,
    InactiveByUser,
    InactiveByRestrictedPlan,
}

impl ResourceStateDocument {
    pub fn is_active(&self) -> bool {
        matches!(self, ResourceStateDocument::Active)
    }

    pub fn is_inactive(&self) -> bool {
        !self.is_active()
    }
}

impl From<ResourceState> for ResourceStateDocument {
    fn from(state: ResourceState) -> Self {
        match state {
            ResourceState::Active => ResourceStateDocument::Active,
            ResourceState::InactiveByUser => ResourceStateDocument::InactiveByUser,
            ResourceState::InactiveByRestrictedPlan => {
                ResourceStateDocument::InactiveByRestrictedPlan
            }
        }
    }
}

impl From<ResourceStateDocument> for ResourceState {
    fn from(state: ResourceStateDocument) -> Self {
        match state {
            ResourceStateDocument::Active => ResourceState::Active,
            ResourceStateDocument::InactiveByUser => ResourceState::InactiveByUser,
            ResourceStateDocument::InactiveByRestrictedPlan => {
                ResourceState::InactiveByRestrictedPlan
            }
        }
    }
}
