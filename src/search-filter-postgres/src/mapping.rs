use common::currency::domain::Currency;
use common::event_id::EventId;
use common::language::domain::Language;
use common::product_id::ProductId;
use common::resource_state::domain::ResourceState;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use common::user_search_filter_name::UserSearchFilterName;
use product_core::product_search::ProductSearch;
use search_filter_core::{SearchFilter, SearchFilterProductMatch};
use search_filter_service::ports::{
    SearchFilterReadError, SearchFilterRepositoryError, SearchFilterView,
};
use sqlx::FromRow;

#[derive(FromRow)]
pub(crate) struct FilterRow {
    pub user_search_filter_id: uuid::Uuid,
    pub user_id: uuid::Uuid,
    pub name: String,
    pub notifications: bool,
    pub state: String,
    pub embedding: Option<Vec<f32>>,
    pub language: String,
    pub currency: String,
    pub created: time::OffsetDateTime,
    pub updated: time::OffsetDateTime,
    pub last_hybrid_search_matched: time::OffsetDateTime,
}

impl FilterRow {
    pub(crate) fn into_domain(self) -> Result<SearchFilter, SearchFilterRepositoryError> {
        Ok(SearchFilter::rehydrate(
            UserSearchFilterId::from(self.user_search_filter_id),
            UserId::from(self.user_id),
            UserSearchFilterName::from(self.name),
            self.notifications,
            parse_filter_state_repository(&self.state)?,
            ProductSearch::new(
                parse_language(&self.language),
                parse_currency(&self.currency),
            ),
            self.embedding,
        ))
    }

    pub(crate) fn into_view(self) -> Result<SearchFilterView, SearchFilterReadError> {
        Ok(SearchFilterView {
            filter: SearchFilter::rehydrate(
                UserSearchFilterId::from(self.user_search_filter_id),
                UserId::from(self.user_id),
                UserSearchFilterName::from(self.name),
                self.notifications,
                parse_filter_state_read(&self.state)?,
                ProductSearch::new(
                    parse_language(&self.language),
                    parse_currency(&self.currency),
                ),
                self.embedding,
            ),
            created: self.created,
            updated: self.updated,
            last_hybrid_search_matched: self.last_hybrid_search_matched,
        })
    }
}

#[derive(FromRow)]
pub(crate) struct MatchRow {
    pub user_id: uuid::Uuid,
    pub user_search_filter_id: uuid::Uuid,
    pub product_id: uuid::Uuid,
    pub origin_event_id: uuid::Uuid,
    pub user_search_filter_name: Option<String>,
    pub enhanced_match_reason: Option<String>,
    pub feedback: Option<bool>,
    pub created: time::OffsetDateTime,
    pub updated: time::OffsetDateTime,
}

impl From<MatchRow> for SearchFilterProductMatch {
    fn from(row: MatchRow) -> Self {
        SearchFilterProductMatch {
            user_id: UserId::from(row.user_id),
            user_search_filter_id: UserSearchFilterId::from(row.user_search_filter_id),
            user_search_filter_name: row.user_search_filter_name.map(UserSearchFilterName::from),
            product_id: ProductId::from(row.product_id),
            origin_event_id: EventId::from(row.origin_event_id),
            enhanced_match_reason: row.enhanced_match_reason.map(Into::into),
            feedback: row.feedback,
            created: row.created,
            updated: row.updated,
        }
    }
}

pub(crate) fn format_state(state: ResourceState) -> &'static str {
    match state {
        ResourceState::Active => "Active",
        ResourceState::InactiveByUser => "InactiveByUser",
        ResourceState::InactiveByRestrictedPlan => "InactiveByRestrictedPlan",
    }
}

pub(crate) fn user_search_filter_uuid(id: UserSearchFilterId) -> uuid::Uuid {
    uuid::Uuid::parse_str(&id.to_string()).unwrap_or_else(|_| uuid::Uuid::nil())
}

fn parse_filter_state(value: &str) -> Option<ResourceState> {
    match value {
        "Active" | "active" => Some(ResourceState::Active),
        "InactiveByUser" | "inactive_by_user" => Some(ResourceState::InactiveByUser),
        "InactiveByRestrictedPlan" | "inactive_by_restricted_plan" => {
            Some(ResourceState::InactiveByRestrictedPlan)
        }
        _ => None,
    }
}

fn parse_filter_state_repository(
    value: &str,
) -> Result<ResourceState, SearchFilterRepositoryError> {
    parse_filter_state(value).ok_or(SearchFilterRepositoryError::InvalidPersistedState)
}

fn parse_filter_state_read(value: &str) -> Result<ResourceState, SearchFilterReadError> {
    parse_filter_state(value).ok_or(SearchFilterReadError::InvalidPersistedState)
}

fn parse_language(value: &str) -> Language {
    match value {
        "de" => Language::De,
        "fr" => Language::Fr,
        "es" => Language::Es,
        "it" => Language::It,
        "zh" => Language::Zh,
        "pt" => Language::Pt,
        "pl" => Language::Pl,
        "tr" => Language::Tr,
        "nl" => Language::Nl,
        "cs" => Language::Cs,
        "ja" => Language::Ja,
        "ru" => Language::Ru,
        "ar" => Language::Ar,
        _ => Language::En,
    }
}

fn parse_currency(value: &str) -> Currency {
    match value {
        "GBP" => Currency::Gbp,
        "USD" => Currency::Usd,
        "AUD" => Currency::Aud,
        "CAD" => Currency::Cad,
        "NZD" => Currency::Nzd,
        "CNY" => Currency::Cny,
        "BRL" => Currency::Brl,
        "PLN" => Currency::Pln,
        "TRY" => Currency::Try,
        "CHF" => Currency::Chf,
        "JPY" => Currency::Jpy,
        "RUB" => Currency::Rub,
        "AED" => Currency::Aed,
        "SAR" => Currency::Sar,
        "HKD" => Currency::Hkd,
        "SGD" => Currency::Sgd,
        "CZK" => Currency::Czk,
        _ => Currency::Eur,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_format_all_states() {
        assert_eq!("Active", format_state(ResourceState::Active));
        assert_eq!(
            "InactiveByUser",
            format_state(ResourceState::InactiveByUser)
        );
        assert_eq!(
            "InactiveByRestrictedPlan",
            format_state(ResourceState::InactiveByRestrictedPlan)
        );
    }

    #[test]
    fn should_reject_invalid_state() {
        assert!(matches!(
            parse_filter_state_repository("bad"),
            Err(SearchFilterRepositoryError::InvalidPersistedState)
        ));
    }
}
