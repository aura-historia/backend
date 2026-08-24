use crate::{
    enhanced_match_reason::EnhancedMatchReason, search_filter_state::SearchFilterState,
    user_search_filter_id::UserSearchFilterId, user_search_filter_name::UserSearchFilterName,
};
use domain_primitives::{change_outcome::ChangeOutcome, event_id::EventId};
use fxrate_core::FxRateId;
use product_listing_core::product_id::ProductId;
use product_listing_core::product_search::ProductSearch;
use user_core::user_id::UserId;
pub mod enhanced_match_reason;
pub mod search_filter_state;
pub mod user_search_filter_id;
pub mod user_search_filter_name;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilter {
    user_search_filter_id: UserSearchFilterId,
    user_id: UserId,
    name: UserSearchFilterName,
    notifications: bool,
    state: SearchFilterState,
    search: ProductSearch,
    embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewSearchFilter {
    pub user_search_filter_id: UserSearchFilterId,
    pub user_id: UserId,
    pub name: UserSearchFilterName,
    pub notifications: bool,
    pub state: SearchFilterState,
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
        state: SearchFilterState,
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

    pub fn rename(&mut self, name: UserSearchFilterName) -> ChangeOutcome {
        if self.name == name {
            return ChangeOutcome::Unchanged;
        }
        self.name = name;
        ChangeOutcome::Changed
    }

    pub fn change_notifications(&mut self, notifications: bool) -> ChangeOutcome {
        if self.notifications == notifications {
            return ChangeOutcome::Unchanged;
        }
        self.notifications = notifications;
        ChangeOutcome::Changed
    }

    pub fn change_state(&mut self, state: SearchFilterState) -> ChangeOutcome {
        if self.state == state {
            return ChangeOutcome::Unchanged;
        }
        self.state = state;
        ChangeOutcome::Changed
    }

    pub fn replace_search(
        &mut self,
        search: ProductSearch,
        embedding: Option<Vec<f32>>,
    ) -> ChangeOutcome {
        if self.search == search && self.embedding == embedding {
            return ChangeOutcome::Unchanged;
        }
        self.search = search;
        self.embedding = embedding;
        ChangeOutcome::Changed
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
    pub fn state(&self) -> SearchFilterState {
        self.state
    }
    pub fn search(&self) -> &ProductSearch {
        &self.search
    }
    pub fn embedding(&self) -> Option<&Vec<f32>> {
        self.embedding.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceMatchValuation {
    pub basis: product_listing_core::product::ProductPriceValuationBasis,
    pub fx_rate_id: FxRateId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilterProductMatch {
    pub user_id: UserId,
    pub user_search_filter_id: UserSearchFilterId,
    pub user_search_filter_name: Option<UserSearchFilterName>,
    pub product_id: ProductId,
    pub origin_event_id: EventId,
    /// Immutable valuation used only when this filter had a price condition.
    pub price_match_valuation: Option<PriceMatchValuation>,
    pub enhanced_match_reason: Option<EnhancedMatchReason>,
    pub feedback: Option<bool>,
}

impl SearchFilterProductMatch {
    pub fn change_feedback(&mut self, feedback: Option<bool>) -> ChangeOutcome {
        if self.feedback == feedback {
            return ChangeOutcome::Unchanged;
        }
        self.feedback = feedback;
        ChangeOutcome::Changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use localization::Language;
    use money::Currency;

    fn sample_filter() -> SearchFilter {
        SearchFilter::create(NewSearchFilter {
            user_search_filter_id: UserSearchFilterId::new(),
            user_id: UserId::new(),
            name: UserSearchFilterName::from("daily"),
            notifications: true,
            state: SearchFilterState::Active,
            search: ProductSearch::new(Language::En, Currency::Eur),
            embedding: None,
        })
    }

    #[test]
    fn should_create_search_filter() {
        let filter = sample_filter();
        assert!(filter.notifications());
        assert_eq!(SearchFilterState::Active, filter.state());
        assert_eq!(Language::En, filter.search().language);
    }

    #[test]
    fn should_rename_search_filter() {
        let mut filter = sample_filter();
        assert_eq!(
            ChangeOutcome::Changed,
            filter.rename(UserSearchFilterName::from("weekly"))
        );
        assert_eq!("weekly", filter.name().as_ref());
    }

    #[test]
    fn should_not_change_search_filter_when_name_is_unchanged() {
        let mut filter = sample_filter();
        assert_eq!(
            ChangeOutcome::Unchanged,
            filter.rename(UserSearchFilterName::from("daily"))
        );
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
        assert_eq!(
            ChangeOutcome::Changed,
            filter.replace_search(
                ProductSearch::new(Language::De, Currency::Usd),
                Some(vec![1.0, 2.0]),
            )
        );
        assert_eq!(Language::De, filter.search().language);
        assert_eq!(Some(&vec![1.0, 2.0]), filter.embedding());
    }

    #[test]
    fn should_not_change_match_feedback_when_value_is_unchanged() {
        let mut product_match = SearchFilterProductMatch {
            user_id: UserId::new(),
            user_search_filter_id: UserSearchFilterId::new(),
            user_search_filter_name: None,
            product_id: ProductId::new(),
            origin_event_id: EventId::new(),
            price_match_valuation: None,
            enhanced_match_reason: None,
            feedback: Some(true),
        };

        assert_eq!(
            ChangeOutcome::Unchanged,
            product_match.change_feedback(Some(true))
        );
    }
}
