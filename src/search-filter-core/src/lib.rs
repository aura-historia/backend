use common::enhanced_match_reason::EnhancedMatchReason;
use common::event_id::EventId;
use common::product_id::ProductId;
use common::resource_state::domain::ResourceState;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
pub use product_core::product_search::{EnhancedSearchDescription, ProductSearch};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilter {
    user_search_filter_id: UserSearchFilterId,
    user_id: UserId,
    name: UserSearchFilterName,
    notifications: bool,
    state: ResourceState,
    search: ProductSearch,
    embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewSearchFilter {
    pub user_search_filter_id: UserSearchFilterId,
    pub user_id: UserId,
    pub name: UserSearchFilterName,
    pub notifications: bool,
    pub state: ResourceState,
    pub search: ProductSearch,
    pub embedding: Option<Vec<f32>>,
}

impl SearchFilter {
    pub fn create(new: NewSearchFilter) -> Self {
        Self {
            user_search_filter_id: new.user_search_filter_id,
            user_id: new.user_id,
            name: new.name,
            notifications: new.notifications,
            state: new.state,
            search: new.search,
            embedding: new.embedding,
        }
    }

    pub fn rehydrate(
        user_search_filter_id: UserSearchFilterId,
        user_id: UserId,
        name: UserSearchFilterName,
        notifications: bool,
        state: ResourceState,
        search: ProductSearch,
        embedding: Option<Vec<f32>>,
    ) -> Self {
        Self {
            user_search_filter_id,
            user_id,
            name,
            notifications,
            state,
            search,
            embedding,
        }
    }

    pub fn rename(&mut self, name: UserSearchFilterName) {
        self.name = name;
    }
    pub fn change_notifications(&mut self, notifications: bool) {
        self.notifications = notifications;
    }
    pub fn change_state(&mut self, state: ResourceState) {
        self.state = state;
    }
    pub fn replace_search(&mut self, search: ProductSearch, embedding: Option<Vec<f32>>) {
        self.search = search;
        self.embedding = embedding;
    }

    pub fn id(&self) -> UserSearchFilterId {
        self.user_search_filter_id
    }
    pub fn user_id(&self) -> UserId {
        self.user_id
    }
    pub fn name(&self) -> &UserSearchFilterName {
        &self.name
    }
    pub fn notifications(&self) -> bool {
        self.notifications
    }
    pub fn state(&self) -> ResourceState {
        self.state
    }
    pub fn search(&self) -> &ProductSearch {
        &self.search
    }
    pub fn embedding(&self) -> Option<&Vec<f32>> {
        self.embedding.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilterProductMatch {
    pub user_id: UserId,
    pub user_search_filter_id: UserSearchFilterId,
    pub user_search_filter_name: Option<UserSearchFilterName>,
    pub product_id: ProductId,
    pub origin_event_id: EventId,
    pub enhanced_match_reason: Option<EnhancedMatchReason>,
    pub feedback: Option<bool>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{currency::domain::Currency, language::domain::Language};

    fn sample_filter() -> SearchFilter {
        SearchFilter::create(NewSearchFilter {
            user_search_filter_id: UserSearchFilterId::new(),
            user_id: UserId::new(),
            name: UserSearchFilterName::from("daily"),
            notifications: true,
            state: ResourceState::Active,
            search: ProductSearch::new(Language::En, Currency::Eur),
            embedding: None,
        })
    }

    #[test]
    fn should_create_search_filter() {
        let filter = sample_filter();
        assert!(filter.notifications());
        assert_eq!(ResourceState::Active, filter.state());
        assert_eq!(Language::En, filter.search().language);
    }

    #[test]
    fn should_rename_search_filter() {
        let mut filter = sample_filter();
        filter.rename(UserSearchFilterName::from("weekly"));
        assert_eq!("weekly", filter.name().as_ref());
    }

    #[test]
    fn should_change_notifications() {
        let mut filter = sample_filter();
        filter.change_notifications(false);
        assert!(!filter.notifications());
    }

    #[test]
    fn should_replace_search_and_embedding() {
        let mut filter = sample_filter();
        filter.replace_search(
            ProductSearch::new(Language::De, Currency::Usd),
            Some(vec![1.0, 2.0]),
        );
        assert_eq!(Language::De, filter.search().language);
        assert_eq!(Some(&vec![1.0, 2.0]), filter.embedding());
    }
}
