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
