//! A price slot that shows the item's status instead of an amount, such as the `Sold` a sold
//! page prints where its price was. The ProductListing normalization owns the status labels;
//! this module only decides whether a captured price is one of them and which part of it is
//! the label (`docs/crawler/extraction-and-review.md#price-status-labels`).

use product_listing_core::listing_availability::ListingAvailability;
use product_listing_normalization::{ListingAvailabilityQuickCheck, quick_check_availability};
use regex::regex;

/// The status label `price` holds instead of an amount: a price without a digit that the
/// ProductListing normalization resolves to an item that is gone (sold out, out of stock,
/// reserved or unavailable) once a price caption before it and a currency marker beside it are
/// set aside, as a shop template prints them around whatever the price cell holds (`Price:
/// SOLD`, `VERKAUFT €`). A price on request, such as "POA" or "Preis auf Anfrage €", is a
/// price the normalization reads, and other free text, such as `Kom €`, resolves to nothing:
/// both stay a price.
pub(crate) fn price_status_label(price: &str) -> Option<&str> {
    if price.chars().any(|character| character.is_ascii_digit()) {
        return None;
    }
    let label = without_caption_and_currency(price);
    is_gone(resolved_availability(label)?).then_some(label)
}

/// Whether the ProductListing normalization resolves `state` to an item that is gone, so that
/// it wins over a status label in the price. A `state` that resolves to an item on offer does
/// not: shops leave an enquiry or cart action on a sold page, and the normalization reads such an
/// action as on offer only because a sold page shows a sold label instead.
pub(crate) fn resolves_to_gone(state: &str) -> bool {
    resolved_availability(state).is_some_and(is_gone)
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

fn resolved_availability(text: &str) -> Option<ListingAvailability> {
    quick_check_availability(text)
        .ok()
        .and_then(ListingAvailabilityQuickCheck::availability)
}

const fn is_gone(availability: ListingAvailability) -> bool {
    matches!(
        availability,
        ListingAvailability::SoldOut
            | ListingAvailability::OutOfStock
            | ListingAvailability::Reserved
            | ListingAvailability::Unavailable
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::sold_out("Sold Out", "Sold Out")]
    #[case::sold("Sold", "Sold")]
    #[case::reserved("Reserved", "Reserved")]
    #[case::german("Verkauft", "Verkauft")]
    #[case::currency_suffix_after_a_no_break_space("VERKAUFT\u{a0}€", "VERKAUFT")]
    #[case::lower_case_with_currency_suffix("verkauft €", "verkauft")]
    #[case::price_caption("Price: SOLD", "SOLD")]
    #[case::currency_code_before("EUR Sold", "Sold")]
    #[case::unavailable("Nicht verfügbar", "Nicht verfügbar")]
    fn should_take_the_status_label_out_of_a_price_that_holds_one(
        #[case] price: &str,
        #[case] label: &str,
    ) {
        assert_eq!(price_status_label(price), Some(label));
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
        assert_eq!(price_status_label(price), None);
    }

    #[rstest]
    #[case::sold("SOLD OUT", true)]
    #[case::reserved("Reserved", true)]
    #[case::enquiry_action("Enquire", false)]
    #[case::cart_action("Add to cart", false)]
    #[case::unrecognised("Inquire", false)]
    fn should_let_only_a_gone_state_win_over_the_label(#[case] state: &str, #[case] wins: bool) {
        assert_eq!(resolves_to_gone(state), wins);
    }
}
