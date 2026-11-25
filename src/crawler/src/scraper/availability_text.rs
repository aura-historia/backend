//! The crawler's one reading of extracted text as an availability, shared by the scraper's
//! plausibility checks and the review's page checks so a finding and a review check cannot
//! disagree about the same `state` or `price` text.
//!
//! It only reads the ProductListing normalization's quick check: the crawler sends values as
//! extracted, and the ProductListing worker stays the normalization authority. Text the quick
//! check refuses, such as text above its length limit, reads as no availability.

use product_listing_core::listing_availability::ListingAvailability;
use product_listing_normalization::{ListingAvailabilityQuickCheck, quick_check_availability};

/// The availability the ProductListing normalization resolves `text` to; `None` for text it
/// does not resolve, reads as no assertion or refuses.
pub(crate) fn resolved_availability(text: &str) -> Option<ListingAvailability> {
    quick_check_availability(text)
        .ok()
        .and_then(ListingAvailabilityQuickCheck::availability)
}

/// Whether the ProductListing normalization recognises `text` as an availability statement:
/// it resolves it, or reads it as asserting none, as for blank text or a schema.org
/// availability that names no stock state.
pub(crate) fn is_recognised_availability(text: &str) -> bool {
    matches!(
        quick_check_availability(text),
        Ok(ListingAvailabilityQuickCheck::Resolved(_) | ListingAvailabilityQuickCheck::NoAssertion)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_resolve_a_status_label_to_its_availability() {
        assert_eq!(
            Some(ListingAvailability::SoldOut),
            resolved_availability("Sold")
        );
        assert!(is_recognised_availability("Sold"));
    }

    #[test]
    fn should_resolve_a_schema_org_availability_url() {
        assert_eq!(
            Some(ListingAvailability::InStock),
            resolved_availability("https://schema.org/InStock")
        );
    }

    #[test]
    fn should_recognise_blank_text_as_no_availability_assertion() {
        assert_eq!(None, resolved_availability("  "));
        assert!(is_recognised_availability("  "));
    }

    #[test]
    fn should_read_unrecognised_text_as_no_availability() {
        assert_eq!(None, resolved_availability("Price on application"));
        assert!(!is_recognised_availability("Price on application"));
    }

    #[test]
    fn should_read_text_the_quick_check_refuses_as_no_availability() {
        let embedded_nul = "Sold\0";

        assert_eq!(None, resolved_availability(embedded_nul));
        assert!(!is_recognised_availability(embedded_nul));
    }
}
