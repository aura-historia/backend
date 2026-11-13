use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId, ViewLink};
use party_core::party_name::PartyName;
use url::Url;

/// Safe ListingSource card shared by public collection and slug reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicListingSourceSummary {
    pub listing_source_id: ListingSourceId,
    pub listing_source_slug_id: ListingSourceSlugId,
    pub name: ListingSourceName,
    pub operator: PublicListingSourceOperatorSummary,
    pub website: Option<PublicListingSourceWebsite>,
    pub image: Option<Url>,
}

/// The source's own website. `view_link` is the link to send visitors to: `url` with the source's
/// referral or tracking applied, the way a ProductListing's view link is derived from its URL.
/// The two travel together so a view link never exists without the URL it was derived from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicListingSourceWebsite {
    pub url: Url,
    pub view_link: ViewLink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicListingSourceOperatorSummary {
    pub name: PartyName,
    /// Whether the operator Party currently has an active Partnership.
    pub partner: bool,
}
