use listing_source_core::{
    ListingSourceId, ListingSourceName, ListingSourceSlugId, ReferralConfiguration,
};
use party_core::party_name::PartyName;

/// Public source identity and presentation needed by ProductListing reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingSourceSummary {
    pub listing_source_id: ListingSourceId,
    pub name: ListingSourceName,
    pub slug_id: ListingSourceSlugId,
    pub operator: ListingSourceOperatorSummary,
}

/// Public presentation of the Party that operates a ListingSource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingSourceOperatorSummary {
    pub name: PartyName,
    /// Whether the operator Party currently has an active Partnership. Derived on read from
    /// authoritative Partnership state; never copied into projections or snapshots.
    pub partner: bool,
}

/// Source data needed to hydrate ProductListing search results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingSourceSummaryWithReferral {
    pub summary: ListingSourceSummary,
    pub referral_configuration: Option<ReferralConfiguration>,
}
