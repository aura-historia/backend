//! A price slot that shows the item's status instead of an amount, such as the `Sold` a sold
//! page prints where its price was. The ProductListing normalization owns the status labels and
//! reads every captured value; this module only decides whether a captured price is one of them,
//! so the review flags the price rule that reads the status slot
//! (`docs/crawler/extraction-and-review.md#plausibility-checks`).

use product_listing_core::listing_availability::ListingAvailability;
use product_listing_normalization::{ListingAvailabilityQuickCheck, quick_check_availability};
use regex::regex;

/// Whether `price` holds a status label instead of an amount: a price without a digit that the
/// ProductListing normalization resolves to an item that is gone (sold out, out of stock,
/// reserved or unavailable) once a price caption before it and a currency marker beside it are
/// set aside, as a shop template prints them around whatever the price cell holds (`Price:
/// SOLD`, `VERKAUFT €`). A price on request, such as "POA" or "Preis auf Anfrage €", is a
/// price the normalization reads, and other free text, such as `Kom €`, resolves to nothing:
/// both stay a price.
pub(crate) fn is_price_status_label(price: &str) -> bool {
    !price.chars().any(|character| character.is_ascii_digit())
        && is_gone(without_caption_and_currency(price))
}

fn without_caption_and_currency(price: &str) -> &str {
    let caption_and_currency = regex!(
        r"(?i)^\s*(?:(?:price|preis|prix|prijs|precio|prezzo)\s*:\s*)?(?:[£€$¥₹₩₽₺₪円원]|(?:eur|gbp|usd|chf|jpy|cad|aud|nzd|sek|nok|dkk|pln|czk|huf|ron|krw)\b)?\s*(?P<label>.*?)\s*(?:[£€$¥₹₩₽₺₪円원]|\b(?:eur|gbp|usd|chf|jpy|cad|aud|nzd|sek|nok|dkk|pln|czk|huf|ron|krw))?\s*$"
    );
    caption_and_currency
        .captures(price)
        .and_then(|captures| captures.name("label"))
        .map_or(price, |label| label.as_str())
}

fn is_gone(label: &str) -> bool {
    matches!(
        quick_check_availability(label)
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

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::sold_out("Sold Out")]
    #[case::sold("Sold")]
    #[case::reserved("Reserved")]
    #[case::german("Verkauft")]
    #[case::currency_suffix_after_a_no_break_space("VERKAUFT\u{a0}€")]
    #[case::lower_case_with_currency_suffix("verkauft €")]
    #[case::price_caption("Price: SOLD")]
    #[case::currency_code_before("EUR Sold")]
    #[case::unavailable("Nicht verfügbar")]
    fn should_recognise_a_price_that_holds_a_gone_status_label(#[case] price: &str) {
        assert!(is_price_status_label(price));
    }

    #[rstest]
    #[case::amount("€ 1.250")]
    #[case::status_beside_an_amount("Sold for £500")]
    #[case::price_on_application("POA")]
    #[case::price_on_request_with_currency_suffix("Preis auf Anfrage\u{a0}€")]
    #[case::unrecognised_request_text("nach Anfrage\u{a0}€")]
    #[case::free_text_with_currency_suffix("Kom\u{a0}€")]
    #[case::an_available_action("Add to cart")]
    #[case::currency_alone("€")]
    #[case::blank(" ")]
    fn should_leave_a_price_that_is_no_gone_status_label(#[case] price: &str) {
        assert!(!is_price_status_label(price));
    }
}
