use domain_primitives::query::{range_query::RangeQuery, text_query::TextQuery};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PartySearch {
    pub query: Option<TextQuery<0>>,
    pub name_query: Option<TextQuery<0>>,
    pub phone_query: Option<TextQuery<0>>,
    pub email_query: Option<TextQuery<0>>,
    pub created: Option<RangeQuery<OffsetDateTime>>,
    pub updated: Option<RangeQuery<OffsetDateTime>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_default_party_search_to_empty_filters() {
        let search = PartySearch::default();

        assert_eq!(None, search.query);
        assert_eq!(None, search.name_query);
        assert_eq!(None, search.phone_query);
        assert_eq!(None, search.email_query);
        assert_eq!(None, search.created);
        assert_eq!(None, search.updated);
    }
}
