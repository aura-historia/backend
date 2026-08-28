use listing_source_core::{
    ListingSourceId, ListingSourceName, ListingSourceSlugId, ReferralConfiguration,
};

/// Public source identity and presentation needed by ProductListing reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingSourceSummary {
    pub listing_source_id: ListingSourceId,
    pub name: ListingSourceName,
    pub slug_id: ListingSourceSlugId,
}

/// Source data needed to hydrate ProductListing search results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingSourceSummaryWithReferral {
    pub summary: ListingSourceSummary,
    pub referral_configuration: Option<ReferralConfiguration>,
}
