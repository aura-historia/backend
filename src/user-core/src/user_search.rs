use crate::{role::UserRole, tier::UserTier};
use domain_primitives::query::{
    any_of_query::AnyOfQuery, range_query::RangeQuery, text_query::TextQuery,
};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UserSearch {
    pub query: Option<TextQuery<0>>,
    pub email_query: Option<TextQuery<0>>,
    pub first_name_query: Option<TextQuery<0>>,
    pub last_name_query: Option<TextQuery<0>>,
    pub tier_query: AnyOfQuery<UserTier>,
    pub role_query: AnyOfQuery<UserRole>,

    pub created: Option<RangeQuery<OffsetDateTime>>,
    pub updated: Option<RangeQuery<OffsetDateTime>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_default_user_search_to_empty_filters() {
        let search = UserSearch::default();

        assert_eq!(None, search.query);
        assert_eq!(None, search.email_query);
        assert_eq!(None, search.first_name_query);
        assert_eq!(None, search.last_name_query);
        assert!(search.tier_query.is_empty());
        assert!(search.role_query.is_empty());

        assert_eq!(None, search.created);
        assert_eq!(None, search.updated);
    }
}
