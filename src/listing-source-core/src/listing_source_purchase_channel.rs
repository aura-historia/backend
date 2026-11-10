use std::str::FromStr;

use strum::IntoEnumIterator;
use strum_macros::EnumIter;

/// Whether a ListingSource lets visitors buy online, curated by administrators.
///
/// It is descriptive only: ingestion, listing validation and crawling do not depend on it. An
/// `EnquiryOnly` source shows its goods but sells them by contact, so its listings carry an
/// enquiry action and usually no price.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter)]
pub enum ListingSourcePurchaseChannel {
    /// Goods can be bought online at the source.
    OnlineShop,
    /// Goods are sold by enquiry (mail, phone, form) and cannot be bought online.
    EnquiryOnly,
}

impl ListingSourcePurchaseChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OnlineShop => "ONLINE_SHOP",
            Self::EnquiryOnly => "ENQUIRY_ONLY",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid listing source purchase channel '{value}'")]
pub struct InvalidListingSourcePurchaseChannel {
    value: String,
}

impl FromStr for ListingSourcePurchaseChannel {
    type Err = InvalidListingSourcePurchaseChannel;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::iter()
            .find(|channel| channel.as_str() == value)
            .ok_or_else(|| InvalidListingSourcePurchaseChannel {
                value: value.to_owned(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_round_trip_every_purchase_channel_code() {
        for channel in ListingSourcePurchaseChannel::iter() {
            assert_eq!(Ok(channel), channel.as_str().parse());
        }
    }

    #[test]
    fn should_reject_unknown_or_noncanonical_purchase_channel_codes() {
        for code in [
            "",
            "online_shop",
            "OnlineShop",
            " ONLINE_SHOP",
            "ENQUIRY",
            "SHOP",
        ] {
            assert!(
                code.parse::<ListingSourcePurchaseChannel>().is_err(),
                "{code:?}"
            );
        }
    }
}
