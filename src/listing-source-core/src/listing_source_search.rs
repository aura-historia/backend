use domain_primitives::query::text_query::TextQuery;
use party_core::party_id::PartyId;

use crate::{ListingIngestionMethod, ListingSourceId, ListingSourceSlugId};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListingSourceSearch {
    pub query: Option<TextQuery<0>>,
    pub name_query: Option<TextQuery<0>>,
    pub listing_source_id: Option<ListingSourceId>,
    pub listing_source_slug_id: Option<ListingSourceSlugId>,
    pub operator_party_id: Option<PartyId>,
    pub ingestion_method: Option<ListingIngestionMethod>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_default_listing_source_search_to_empty_filters() {
        assert_eq!(
            ListingSourceSearch::default(),
            ListingSourceSearch {
                query: None,
                name_query: None,
                listing_source_id: None,
                listing_source_slug_id: None,
                operator_party_id: None,
                ingestion_method: None,
            }
        );
    }
}
