use crate::{listing_availability::ListingAvailability, listing_lifecycle::ListingLifecycle};

/// Whether public discovery (search, similar listings and saved-search retrieval) offers a
/// listing: it must be active and not asserted sold out, out of stock or unavailable.
///
/// Absent availability is no assertion, so it stays discoverable. `RESERVED` is a temporary hold
/// that may return to sale, so it stays discoverable too.
pub const fn is_discoverable(
    lifecycle: ListingLifecycle,
    availability: Option<ListingAvailability>,
) -> bool {
    match lifecycle {
        ListingLifecycle::Withdrawn => false,
        ListingLifecycle::Active => match availability {
            Some(availability) => availability.is_discoverable(),
            None => true,
        },
    }
}

impl ListingAvailability {
    /// Whether an active listing with this assertion stays discoverable; see [`is_discoverable`].
    pub const fn is_discoverable(self) -> bool {
        !matches!(self, Self::SoldOut | Self::OutOfStock | Self::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use strum::IntoEnumIterator;

    #[rstest]
    #[case::available(ListingAvailability::Available, true)]
    #[case::in_stock(ListingAvailability::InStock, true)]
    #[case::limited(ListingAvailability::LimitedAvailability, true)]
    #[case::back_order(ListingAvailability::BackOrder, true)]
    #[case::made_to_order(ListingAvailability::MadeToOrder, true)]
    #[case::pre_order(ListingAvailability::PreOrder, true)]
    #[case::pre_sale(ListingAvailability::PreSale, true)]
    #[case::reserved(ListingAvailability::Reserved, true)]
    #[case::unavailable(ListingAvailability::Unavailable, false)]
    #[case::out_of_stock(ListingAvailability::OutOfStock, false)]
    #[case::sold_out(ListingAvailability::SoldOut, false)]
    fn should_keep_active_listing_discoverable_unless_asserted_unbuyable(
        #[case] availability: ListingAvailability,
        #[case] expected: bool,
    ) {
        assert_eq!(
            expected,
            is_discoverable(ListingLifecycle::Active, Some(availability))
        );
    }

    #[test]
    fn should_keep_active_listing_discoverable_when_availability_is_absent() {
        assert!(is_discoverable(ListingLifecycle::Active, None));
    }

    #[test]
    fn should_hide_withdrawn_listing_whatever_its_availability() {
        assert!(!is_discoverable(ListingLifecycle::Withdrawn, None));
        for availability in ListingAvailability::iter() {
            assert!(!is_discoverable(
                ListingLifecycle::Withdrawn,
                Some(availability)
            ));
        }
    }
}
