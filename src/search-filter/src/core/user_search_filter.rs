use crate::core::user_search_filter_name::UserSearchFilterName;
use common::{
    resource_state::domain::ResourceState, string_newtype, user_id::UserId,
    user_search_filter_id::UserSearchFilterId,
};
use product::core::product_search::ProductSearch;
use serde_fields::SerdeField;
use time::OffsetDateTime;

string_newtype!(EnhancedSearchDescription, max_length(1000));

#[derive(Debug, Clone)]
pub struct UserSearchFilterSummary {
    pub user_id: UserId,
    pub user_search_filter_id: UserSearchFilterId,
    pub name: UserSearchFilterName,
    pub enhanced_search_description: Option<EnhancedSearchDescription>,
    pub notifications: bool,
    pub state: ResourceState,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
    pub last_hybrid_search_matched: OffsetDateTime,
}

#[derive(Debug, Clone, SerdeField)]
pub struct UserSearchFilter {
    pub user_id: UserId,
    pub user_search_filter_id: UserSearchFilterId,
    pub name: UserSearchFilterName,
    pub notifications: bool,
    pub state: ResourceState,
    pub search: ProductSearch,
    pub enhanced_search_description: Option<EnhancedSearchDescription>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
    pub last_hybrid_search_matched: OffsetDateTime,
}

impl From<UserSearchFilter> for UserSearchFilterSummary {
    fn from(filter: UserSearchFilter) -> Self {
        UserSearchFilterSummary {
            user_id: filter.user_id,
            user_search_filter_id: filter.user_search_filter_id,
            name: filter.name,
            enhanced_search_description: filter.enhanced_search_description,
            notifications: filter.notifications,
            state: filter.state,
            created: filter.created,
            updated: filter.updated,
            last_hybrid_search_matched: filter.last_hybrid_search_matched,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for UserSearchFilter {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            UserSearchFilter {
                user_id: config.fake_with_rng(rng),
                user_search_filter_id: config.fake_with_rng(rng),
                name: config.fake_with_rng(rng),
                notifications: true,
                state: ResourceState::Active,
                search: config.fake_with_rng(rng),
                enhanced_search_description: None,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
                last_hybrid_search_matched: OffsetDateTime::now_utc(),
            }
        }
    }

    impl Dummy<Faker> for UserSearchFilterSummary {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            UserSearchFilterSummary {
                user_id: config.fake_with_rng(rng),
                user_search_filter_id: config.fake_with_rng(rng),
                name: config.fake_with_rng(rng),
                enhanced_search_description: None,
                notifications: true,
                state: ResourceState::Active,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
                last_hybrid_search_matched: OffsetDateTime::now_utc(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_accept_short_enhanced_search_description() {
        let desc = EnhancedSearchDescription::from("Golden cufflinks");
        assert_eq!(desc.as_ref(), "Golden cufflinks");
    }

    #[test]
    fn should_truncate_enhanced_search_description_exceeding_max_length() {
        let long = "a".repeat(1200);
        let desc = EnhancedSearchDescription::from(long.as_str());
        assert_eq!(desc.as_ref().len(), 1000);
    }

    #[test]
    fn should_keep_exactly_1000_chars_for_enhanced_search_description() {
        let exact = "b".repeat(1000);
        let desc = EnhancedSearchDescription::from(exact.as_str());
        assert_eq!(desc.as_ref().len(), 1000);
    }

    #[test]
    fn should_trim_whitespace_for_enhanced_search_description() {
        let desc = EnhancedSearchDescription::from("   hello   ");
        assert_eq!(desc.as_ref(), "hello");
    }

    #[test]
    fn should_handle_empty_enhanced_search_description() {
        let desc = EnhancedSearchDescription::from("");
        assert_eq!(desc.as_ref(), "");
    }

    #[test]
    fn should_trim_then_truncate_for_enhanced_search_description() {
        let padded = format!("   {}   ", "c".repeat(1100));
        let desc = EnhancedSearchDescription::from(padded.as_str());
        assert_eq!(desc.as_ref().len(), 1000);
    }

    #[test]
    fn should_create_from_string_with_truncation() {
        let long = "d".repeat(1111);
        let desc = EnhancedSearchDescription::from(long);
        assert_eq!(desc.as_ref().len(), 1000);
    }
}
