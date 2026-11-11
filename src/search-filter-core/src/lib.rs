use crate::{
    search_filter_state::SearchFilterState, user_search_filter_id::UserSearchFilterId,
    user_search_filter_name::UserSearchFilterName,
};
use domain_primitives::{change_outcome::ChangeOutcome, event_id::EventId};
use fxrate_core::FxRateId;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_search::ProductListingSearch;
use user_core::user_id::UserId;
mod matching_evidence_policy;
pub mod search_filter_state;
mod search_intent_translation;
pub use matching_evidence_policy::MatchingEvidencePolicy;
pub use search_intent_translation::{
    EnglishSearchIntent, SearchIntentFingerprint, SearchIntentTranslation,
    SearchIntentTranslationError,
};
pub mod user_search_filter_id;
pub mod user_search_filter_name;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilter {
    user_search_filter_id: UserSearchFilterId,
    user_id: UserId,
    name: UserSearchFilterName,
    notifications: bool,
    state: SearchFilterState,
    search: ProductListingSearch,
    embedding: Option<Vec<f32>>,
    matching_evidence_policy: MatchingEvidencePolicy,
    /// Internal matching state; always current for `search` when present.
    intent_translation: Option<SearchIntentTranslation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewSearchFilter {
    pub user_search_filter_id: UserSearchFilterId,
    pub user_id: UserId,
    pub name: UserSearchFilterName,
    pub notifications: bool,
    pub state: SearchFilterState,
    pub search: ProductListingSearch,
    pub embedding: Option<Vec<f32>>,
    pub matching_evidence_policy: MatchingEvidencePolicy,
    pub intent_translation: Option<SearchIntentTranslation>,
}

/// A translation made from different text than the filter's current search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("search intent translation does not match the filter's current search")]
pub struct StaleSearchIntentTranslation;

impl SearchFilter {
    pub fn create(new: NewSearchFilter) -> Self {
        // Only a translation usable for the current search is kept; the next write clears others.
        let intent_translation = new.intent_translation.filter(|translation| {
            translation.is_current_for(&new.search) && translation.english_for(&new.search).is_ok()
        });
        Self {
            user_search_filter_id: new.user_search_filter_id,
            user_id: new.user_id,
            name: new.name,
            notifications: new.notifications,
            state: new.state,
            search: new.search,
            embedding: new.embedding,
            matching_evidence_policy: new.matching_evidence_policy,
            intent_translation,
        }
    }

    pub fn rehydrate(state: NewSearchFilter) -> Self {
        Self::create(state)
    }

    pub fn rename(&mut self, name: UserSearchFilterName) -> ChangeOutcome {
        if self.name == name {
            return ChangeOutcome::Unchanged;
        }
        self.name = name;
        ChangeOutcome::Changed
    }

    pub fn resolve_matching_evidence(&mut self, policy: MatchingEvidencePolicy) -> ChangeOutcome {
        if self.matching_evidence_policy == policy {
            return ChangeOutcome::Unchanged;
        }
        self.matching_evidence_policy = policy;
        ChangeOutcome::Changed
    }

    pub fn matching_evidence_policy(&self) -> MatchingEvidencePolicy {
        self.matching_evidence_policy
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

    /// Replacing the semantic text discards the translation made from the old text; changing
    /// only other criteria keeps it.
    pub fn replace_search(
        &mut self,
        search: ProductListingSearch,
        embedding: Option<Vec<f32>>,
    ) -> ChangeOutcome {
        if self.search == search && self.embedding == embedding {
            return ChangeOutcome::Unchanged;
        }
        self.search = search;
        self.embedding = embedding;
        if self
            .intent_translation
            .as_ref()
            .is_some_and(|translation| !translation.is_current_for(&self.search))
        {
            self.intent_translation = None;
        }
        ChangeOutcome::Changed
    }

    pub fn record_intent_translation(
        &mut self,
        translation: SearchIntentTranslation,
    ) -> Result<ChangeOutcome, StaleSearchIntentTranslation> {
        if !translation.is_current_for(&self.search) {
            return Err(StaleSearchIntentTranslation);
        }
        if self.intent_translation.as_ref() == Some(&translation) {
            return Ok(ChangeOutcome::Unchanged);
        }
        self.intent_translation = Some(translation);
        Ok(ChangeOutcome::Changed)
    }

    pub fn intent_translation(&self) -> Option<&SearchIntentTranslation> {
        self.intent_translation.as_ref()
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
    pub fn search(&self) -> &ProductListingSearch {
        &self.search
    }
    pub fn embedding(&self) -> Option<&Vec<f32>> {
        self.embedding.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceMatchValuation {
    pub basis: product_listing_core::product_listing::ProductListingPriceValuationBasis,
    pub fx_rate_id: FxRateId,
}

/// Immutable explanation of an explicitly FX-aware shipping maximum at evaluation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShippingMatchValuation {
    pub fx_rate_id: FxRateId,
    pub captured_at: time::OffsetDateTime,
    pub maximum_per_item: money::Price,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilterProductListingMatch {
    pub user_id: UserId,
    pub user_search_filter_id: UserSearchFilterId,
    pub user_search_filter_name: Option<UserSearchFilterName>,
    pub product_listing_id: ProductListingId,
    pub origin_event_id: EventId,
    /// Immutable valuation used only when this filter had a price condition.
    pub price_match_valuation: Option<PriceMatchValuation>,
    pub shipping_match_valuation: Option<ShippingMatchValuation>,
    pub feedback: Option<bool>,
}

impl SearchFilterProductListingMatch {
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
            search: ProductListingSearch::new(Language::En, Currency::Eur),
            embedding: None,
            matching_evidence_policy: MatchingEvidencePolicy::EnhancedTextAndFourImages,
            intent_translation: None,
        })
    }

    fn translated_filter() -> SearchFilter {
        let search = ProductListingSearch::new(Language::De, Currency::Eur)
            .with_product_listing_query("Manschettenknöpfe".try_into().unwrap());
        let english = EnglishSearchIntent::new(vec!["cufflinks".try_into().unwrap()], None);
        let mut filter = sample_filter();
        filter.replace_search(search.clone(), None);
        filter
            .record_intent_translation(SearchIntentTranslation::english(&search, english).unwrap())
            .unwrap();
        filter
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
                ProductListingSearch::new(Language::De, Currency::Usd),
                Some(vec![1.0, 2.0]),
            )
        );
        assert_eq!(Language::De, filter.search().language);
        assert_eq!(Some(&vec![1.0, 2.0]), filter.embedding());
    }

    #[test]
    fn should_keep_translation_when_only_structured_criteria_change() {
        let mut filter = translated_filter();
        let mut search = filter.search().clone();
        search.currency = Currency::Usd;

        filter.replace_search(search, None);

        assert!(filter.intent_translation().is_some());
    }

    #[test]
    fn should_discard_translation_when_semantic_text_changes() {
        let mut filter = translated_filter();
        let search = ProductListingSearch::new(Language::De, Currency::Eur)
            .with_product_listing_query("Broschen".try_into().unwrap());

        filter.replace_search(search, None);

        assert_eq!(None, filter.intent_translation());
    }

    #[test]
    fn should_refuse_a_translation_of_other_text() {
        let mut filter = translated_filter();
        let other = ProductListingSearch::new(Language::De, Currency::Eur)
            .with_product_listing_query("Broschen".try_into().unwrap());

        assert_eq!(
            Err(StaleSearchIntentTranslation),
            filter.record_intent_translation(SearchIntentTranslation::already_english(&other))
        );
        assert!(filter.intent_translation().is_some());
    }

    #[test]
    fn should_not_change_match_feedback_when_value_is_unchanged() {
        let mut product_match = SearchFilterProductListingMatch {
            user_id: UserId::new(),
            user_search_filter_id: UserSearchFilterId::new(),
            user_search_filter_name: None,
            product_listing_id: ProductListingId::new(),
            origin_event_id: EventId::new(),
            price_match_valuation: None,
            shipping_match_valuation: None,
            feedback: Some(true),
        };

        assert_eq!(
            ChangeOutcome::Unchanged,
            product_match.change_feedback(Some(true))
        );
    }
}
