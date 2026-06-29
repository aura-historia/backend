use common::resource_state::domain::ResourceState;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UserSearchFilterSearch {
    pub state: Option<ResourceState>,
    pub has_enhanced_search_description: Option<bool>,
}
