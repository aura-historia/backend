#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum_macros::EnumIter)]
pub enum ListingAvailability {
    Available,
    InStock,
    LimitedAvailability,
    BackOrder,
    MadeToOrder,
    PreOrder,
    PreSale,
    Unavailable,
    Reserved,
    OutOfStock,
    SoldOut,
}

impl ListingAvailability {
    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;

        Self::iter().find(|availability| availability.as_str() == value)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "AVAILABLE",
            Self::InStock => "IN_STOCK",
            Self::LimitedAvailability => "LIMITED_AVAILABILITY",
            Self::BackOrder => "BACK_ORDER",
            Self::MadeToOrder => "MADE_TO_ORDER",
            Self::PreOrder => "PRE_ORDER",
            Self::PreSale => "PRE_SALE",
            Self::Unavailable => "UNAVAILABLE",
            Self::Reserved => "RESERVED",
            Self::OutOfStock => "OUT_OF_STOCK",
            Self::SoldOut => "SOLD_OUT",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use strum::IntoEnumIterator;

    #[test]
    fn should_round_trip_each_canonical_code() {
        for availability in ListingAvailability::iter() {
            assert_eq!(
                Some(availability),
                ListingAvailability::from_code(availability.as_str())
            );
        }
    }

    #[test]
    fn should_use_unique_exact_canonical_codes() {
        let codes = ListingAvailability::iter()
            .map(ListingAvailability::as_str)
            .collect::<HashSet<_>>();

        assert_eq!(ListingAvailability::iter().count(), codes.len());
        assert_eq!(None, ListingAvailability::from_code("in_stock"));
        assert_eq!(None, ListingAvailability::from_code("UNKNOWN"));
    }
}
