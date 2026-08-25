use crate::listing_availability::ListingAvailability;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum_macros::EnumIter)]
pub enum ListingOrderability {
    OrderableNow,
    OrderableConditionally,
    NotOrderable,
}

impl ListingOrderability {
    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;

        Self::iter().find(|orderability| orderability.as_str() == value)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OrderableNow => "ORDERABLE_NOW",
            Self::OrderableConditionally => "ORDERABLE_CONDITIONALLY",
            Self::NotOrderable => "NOT_ORDERABLE",
        }
    }
}

impl ListingAvailability {
    pub const fn orderability(self) -> ListingOrderability {
        match self {
            Self::Available | Self::InStock | Self::LimitedAvailability => {
                ListingOrderability::OrderableNow
            }
            Self::BackOrder | Self::MadeToOrder | Self::PreOrder | Self::PreSale => {
                ListingOrderability::OrderableConditionally
            }
            Self::Unavailable | Self::Reserved | Self::OutOfStock | Self::SoldOut => {
                ListingOrderability::NotOrderable
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_derive_orderability_exhaustively() {
        assert_eq!(
            ListingOrderability::OrderableNow,
            ListingAvailability::InStock.orderability()
        );
        assert_eq!(
            ListingOrderability::OrderableConditionally,
            ListingAvailability::PreOrder.orderability()
        );
        assert_eq!(
            ListingOrderability::NotOrderable,
            ListingAvailability::SoldOut.orderability()
        );
    }
}
