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
        ListingLifecycle::Active => is_for_sale(availability),
    }
}

/// Whether the source still offers the listing: it does not assert it sold out, out of stock or
/// unavailable.
pub const fn is_for_sale(availability: Option<ListingAvailability>) -> bool {
    match availability {
        Some(availability) => availability.is_discoverable(),
        None => true,
    }
}

/// Whether an availability change puts a listing that was not for sale back on sale, including
/// by clearing the assertion.
pub const fn returns_to_sale(
    previous: Option<ListingAvailability>,
    current: Option<ListingAvailability>,
) -> bool {
    !is_for_sale(previous) && is_for_sale(current)
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

    #[rstest]
    #[case::sold_out_to_in_stock(
        Some(ListingAvailability::SoldOut),
        Some(ListingAvailability::InStock),
        true
    )]
    #[case::out_of_stock_to_reserved(
        Some(ListingAvailability::OutOfStock),
        Some(ListingAvailability::Reserved),
        true
    )]
    #[case::unavailable_to_absent(Some(ListingAvailability::Unavailable), None, true)]
    #[case::sold_out_to_out_of_stock(
        Some(ListingAvailability::SoldOut),
        Some(ListingAvailability::OutOfStock),
        false
    )]
    #[case::in_stock_to_available(
        Some(ListingAvailability::InStock),
        Some(ListingAvailability::Available),
        false
    )]
    #[case::absent_to_in_stock(None, Some(ListingAvailability::InStock), false)]
    #[case::in_stock_to_sold_out(
        Some(ListingAvailability::InStock),
        Some(ListingAvailability::SoldOut),
        false
    )]
    fn should_return_to_sale_only_when_leaving_an_unbuyable_assertion(
        #[case] previous: Option<ListingAvailability>,
        #[case] current: Option<ListingAvailability>,
        #[case] expected: bool,
    ) {
        assert_eq!(expected, returns_to_sale(previous, current));
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
