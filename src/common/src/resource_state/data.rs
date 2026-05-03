use crate::resource_state::domain::ResourceState;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResourceStateData {
    #[default]
    Active,
    InactiveByUser,
    InactiveByRestrictedPlan,
}

impl ResourceStateData {
    pub fn is_active(&self) -> bool {
        matches!(self, ResourceStateData::Active)
    }

    pub fn is_inactive(&self) -> bool {
        !self.is_active()
    }
}

impl From<ResourceState> for ResourceStateData {
    fn from(state: ResourceState) -> Self {
        match state {
            ResourceState::Active => ResourceStateData::Active,
            ResourceState::InactiveByUser => ResourceStateData::InactiveByUser,
            ResourceState::InactiveByRestrictedPlan => ResourceStateData::InactiveByRestrictedPlan,
        }
    }
}

impl From<ResourceStateData> for ResourceState {
    fn from(state: ResourceStateData) -> Self {
        match state {
            ResourceStateData::Active => ResourceState::Active,
            ResourceStateData::InactiveByUser => ResourceState::InactiveByUser,
            ResourceStateData::InactiveByRestrictedPlan => ResourceState::InactiveByRestrictedPlan,
        }
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PatchResourceStateData {
    Active,
    InactiveByUser,
}

impl From<PatchResourceStateData> for ResourceState {
    fn from(state: PatchResourceStateData) -> Self {
        match state {
            PatchResourceStateData::Active => ResourceState::Active,
            PatchResourceStateData::InactiveByUser => ResourceState::InactiveByUser,
        }
    }
}
