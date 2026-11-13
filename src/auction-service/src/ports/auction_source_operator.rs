use party_core::party_name::PartyName;

/// Public presentation of the Party that operates an Auction's ListingSource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuctionSourceOperatorSummary {
    pub name: PartyName,
    /// Whether the operator Party currently has an active Partnership.
    pub partner: bool,
}
