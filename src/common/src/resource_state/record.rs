use serde::{Deserialize, Serialize};

use crate::resource_state::domain::ResourceState;

#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResourceStateRecord {
    #[default]
    Active,
    InactiveByUser,
    InactiveByRestrictedPlan,
}

impl ResourceStateRecord {
    pub fn is_active(&self) -> bool {
        matches!(self, ResourceStateRecord::Active)
    }

    pub fn is_inactive(&self) -> bool {
        !self.is_active()
    }
}

impl From<ResourceState> for ResourceStateRecord {
    fn from(state: ResourceState) -> Self {
        match state {
            ResourceState::Active => ResourceStateRecord::Active,
            ResourceState::InactiveByUser => ResourceStateRecord::InactiveByUser,
            ResourceState::InactiveByRestrictedPlan => {
                ResourceStateRecord::InactiveByRestrictedPlan
            }
        }
    }
}

impl From<ResourceStateRecord> for ResourceState {
    fn from(state: ResourceStateRecord) -> Self {
        match state {
            ResourceStateRecord::Active => ResourceState::Active,
            ResourceStateRecord::InactiveByUser => ResourceState::InactiveByUser,
            ResourceStateRecord::InactiveByRestrictedPlan => {
                ResourceState::InactiveByRestrictedPlan
            }
        }
    }
}
