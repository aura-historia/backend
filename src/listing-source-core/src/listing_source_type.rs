use std::str::FromStr;

use strum::IntoEnumIterator;
use strum_macros::EnumIter;

/// The primary sales model of a ListingSource, curated by administrators.
///
/// It combines whose inventory the source sells (its operator's own or consigned inventory, or
/// other sellers' as a host) with how it sells (fixed price or auction). It is descriptive only:
/// ingestion, listing validation and Auction facts do not depend on it. A seller's storefront on
/// a marketplace is classified by that seller, not by the hosting marketplace.
///
/// The set is closed: public responses expose one count per type, so a new type is a deliberate
/// contract change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter)]
pub enum ListingSourceType {
    /// Sells its operator's own or consigned inventory at fixed prices.
    Dealer,
    /// Sells its operator's own or consigned inventory by auction.
    AuctionHouse,
    /// Hosts other sellers who sell at fixed prices.
    Marketplace,
    /// Hosts other sellers who sell by auction.
    AuctionPlatform,
}

impl ListingSourceType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dealer => "DEALER",
            Self::AuctionHouse => "AUCTION_HOUSE",
            Self::Marketplace => "MARKETPLACE",
            Self::AuctionPlatform => "AUCTION_PLATFORM",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid listing source type '{value}'")]
pub struct InvalidListingSourceType {
    value: String,
}

impl FromStr for ListingSourceType {
    type Err = InvalidListingSourceType;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::iter()
            .find(|source_type| source_type.as_str() == value)
            .ok_or_else(|| InvalidListingSourceType {
                value: value.to_owned(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_round_trip_every_listing_source_type_code() {
        for source_type in ListingSourceType::iter() {
            assert_eq!(Ok(source_type), source_type.as_str().parse());
        }
    }

    #[test]
    fn should_reject_unknown_or_noncanonical_listing_source_type_codes() {
        for code in ["", "dealer", "Dealer", " DEALER", "AUCTION-HOUSE", "SELLER"] {
            assert!(code.parse::<ListingSourceType>().is_err(), "{code:?}");
        }
    }
}
