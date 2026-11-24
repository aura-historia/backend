//! A price slot that shows the item's status instead of an amount, such as the `Sold` a sold
//! page prints where its price was. The ProductListing normalization owns the status labels;
//! this module only decides whether a captured price is one of them.

use product_listing_core::listing_availability::ListingAvailability;
use product_listing_normalization::{ListingAvailabilityQuickCheck, quick_check_availability};

/// Whether `price` holds no digit and the ProductListing normalization resolves it to an item
/// that is gone: sold out, out of stock, reserved or unavailable. A price on application, such
/// as "POA", resolves to none of them and stays a price.
pub(crate) fn is_price_status_label(price: &str) -> bool {
    if price.chars().any(|character| character.is_ascii_digit()) {
        return false;
    }
    matches!(
        quick_check_availability(price)
            .ok()
            .and_then(ListingAvailabilityQuickCheck::availability),
        Some(
            ListingAvailability::SoldOut
                | ListingAvailability::OutOfStock
                | ListingAvailability::Reserved
                | ListingAvailability::Unavailable
        )
    )
}
