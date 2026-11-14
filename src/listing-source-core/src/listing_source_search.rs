use domain_primitives::query::text_query::TextQuery;
use geo::CountryCode;
use party_core::party_id::PartyId;

use crate::{
    ListingIngestionMethod, ListingSourceId, ListingSourceSlugId, ListingSourceType,
    PlatformTechnology,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListingSourceSearch {
    pub query: Option<TextQuery<0>>,
    pub name_query: Option<TextQuery<0>>,
    pub listing_source_id: Option<ListingSourceId>,
    pub listing_source_slug_id: Option<ListingSourceSlugId>,
    pub operator_party_id: Option<PartyId>,
    pub listing_source_type: Option<ListingSourceType>,
    pub platform_technology: Option<PlatformTechnology>,
    pub ingestion_method: Option<ListingIngestionMethod>,
    /// Country of the source's effective location default; see `docs/geography.md`.
    pub country: Option<CountryCode>,
}
