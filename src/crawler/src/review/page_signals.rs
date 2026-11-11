//! What a review page shows that a candidate schema should capture: a price, as an amount with its
//! currency in a price element or as a price above zero the offers of the page's own JSON-LD item
//! state, and a block of prose in a description element. They are conservative heuristics over the
//! page's own content, so a shop that shows neither never needs the rule; the approval guard
//! (`docs/crawler/extraction-and-review.md`) compares them with the rules of the candidate.

use crate::review::model::{PageSignals, SchemaPageReference};
use crate::review::product_markup::json_ld_scripts;
use crate::scraper::css_selector::rule::{JsonLdOfferProperty, json_ld_offer_value};
use crate::scraper::plausibility::is_price_text;
use scraper::{ElementRef, Html, node::Element};

/// The fewest words and characters a description element holds to count as a block of prose, so a
/// label or a heading in a `description` class does not.
const MIN_DESCRIPTION_WORDS: usize = 8;
const MIN_DESCRIPTION_CHARS: usize = 50;

/// Elements outside the page's own content: site chrome, other products' teasers and carts.
const CHROME_TAGS: [&str; 7] = [
    "nav", "header", "footer", "aside", "script", "style", "noscript",
];
const OTHER_CONTENT_WORDS: [&str; 9] = [
    "related",
    "similar",
    "recommend",
    "upsell",
    "cross-sell",
    "carousel",
    "cart",
    "basket",
    "minicart",
];

/// The signals of the parsed page `html`, which the matrix refers to as `page_reference`.
pub(crate) fn page_signals(
    page_reference: SchemaPageReference,
    url: &str,
    html: &Html,
) -> PageSignals {
    let mut signals = PageSignals {
        page_reference,
        url: url.to_owned(),
        shows_price_text: false,
        shows_description_block: false,
    };
    for element in html
        .root_element()
        .descendants()
        .filter_map(ElementRef::wrap)
    {
        if (signals.shows_price_text && signals.shows_description_block)
            || !is_page_content(element)
        {
            continue;
        }
        signals.shows_price_text |= is_price_element(element) && holds_price_text(element);
        signals.shows_description_block |= is_description_element(element) && holds_prose(element);
    }
    signals.shows_price_text = signals.shows_price_text || offers_a_price(html);
    signals
}

/// Whether the offers of the item a JSON-LD script of the page is about state a price above zero,
/// as a `json_ld_offer` price rule would read it: a theme that shows the price in no element named
/// for it still publishes it there. Offers of other products the page lists are not followed.
fn offers_a_price(html: &Html) -> bool {
    json_ld_scripts(html)
        .any(|json| json_ld_offer_value(&json, JsonLdOfferProperty::Price).is_some())
}

/// Whether `element` is part of the page's own content: none of its ancestors, itself included,
/// is site chrome, a cart, or a list of other products.
fn is_page_content(element: ElementRef<'_>) -> bool {
    std::iter::once(element)
        .chain(element.ancestors().filter_map(ElementRef::wrap))
        .all(|candidate| {
            let value = candidate.value();
            !CHROME_TAGS.contains(&value.name()) && !names_other_content(value)
        })
}

fn names_other_content(element: &Element) -> bool {
    let names = names_of(element);
    OTHER_CONTENT_WORDS.iter().any(|word| names.contains(word))
}

/// The lowercase class, id and `itemprop` of `element`, one string.
fn names_of(element: &Element) -> String {
    ["class", "id", "itemprop"]
        .into_iter()
        .filter_map(|attribute| element.attr(attribute))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn is_price_element(element: ElementRef<'_>) -> bool {
    names_of(element.value()).contains("price")
}

fn holds_price_text(element: ElementRef<'_>) -> bool {
    let content = element.value().attr("content").unwrap_or_default();
    [content.to_owned(), collapsed_text(element)]
        .iter()
        .any(|text| is_price_text(text) && !has_only_zero_digits(text))
}

fn has_only_zero_digits(text: &str) -> bool {
    let mut digits = text.chars().filter(char::is_ascii_digit).peekable();
    digits.peek().is_some() && digits.all(|digit| digit == '0')
}

fn is_description_element(element: ElementRef<'_>) -> bool {
    names_of(element.value()).contains("description")
}

fn holds_prose(element: ElementRef<'_>) -> bool {
    let text = collapsed_text(element);
    text.chars().count() >= MIN_DESCRIPTION_CHARS
        && text.split_whitespace().count() >= MIN_DESCRIPTION_WORDS
}

fn collapsed_text(element: ElementRef<'_>) -> String {
    element
        .text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals_of(body: &str) -> (bool, bool) {
        let html = Html::parse_document(&format!("<html><body>{body}</body></html>"));
        let signals = page_signals(
            SchemaPageReference::Input { page_index: 0 },
            "https://shop.example/item",
            &html,
        );
        (signals.shows_price_text, signals.shows_description_block)
    }

    const PROSE: &str = "A Georgian oak chest of drawers with original brass handles, restored in 2019 and in very good condition.";

    #[test]
    fn should_see_a_price_and_a_description_in_the_pages_own_content() {
        assert_eq!(
            signals_of(&format!(
                "<h1>Chest</h1><span class=\"product-price\">Price £113</span><div class=\"product-description\"><p>{PROSE}</p></div>"
            )),
            (true, true)
        );
        assert_eq!(
            signals_of(&format!(
                "<span itemprop=\"price\" content=\"EUR 75\"></span><div itemprop=\"description\">{PROSE}</div>"
            )),
            (true, true)
        );
    }

    /// A status label beside the amount keeps a `state` value from being price text, but a price
    /// element that says "Sold for £500" still shows a price the candidate should capture.
    #[test]
    fn should_see_a_price_beside_a_status_label() {
        assert_eq!(
            signals_of("<span class=\"price\">Sold for £500</span>"),
            (true, false)
        );
    }

    /// A theme that shows the price in no element named for it, as a Divi poster shop does, still
    /// shows a price when its own JSON-LD item offers one above zero; a zero price, as templates
    /// publish for sold or price-on-request items, and another product's offer in a list do not.
    #[test]
    fn should_see_a_price_the_page_s_own_json_ld_offers() {
        let script = |json: &str| {
            format!(r#"<h1>Poster</h1><script type="application/ld+json">{json}</script>"#)
        };

        assert_eq!(
            signals_of(&script(
                r#"{"@type":"Product","name":"Poster","offers":{"@type":"Offer","price":7900,"priceCurrency":"CHF"}}"#
            )),
            (true, false)
        );
        assert_eq!(
            signals_of(&script(
                r#"{"@graph":[{"@type":"WebPage"},{"@type":"Product","offers":[{"price":"5600.00"}]}]}"#
            )),
            (true, false)
        );
        assert_eq!(
            signals_of(&script(
                r#"{"@type":"Product","offers":{"price":"0","priceCurrency":"CHF"}}"#
            )),
            (false, false)
        );
        assert_eq!(
            signals_of(&script(
                r#"{"@type":"ItemList","itemListElement":[{"@type":"Product","offers":{"price":120}}]}"#
            )),
            (false, false)
        );
        assert_eq!(signals_of(&script("not json")), (false, false));
    }

    #[test]
    fn should_see_neither_on_a_page_that_shows_neither() {
        assert_eq!(
            signals_of(
                "<h1>Chest</h1><span class=\"price\">Price on application</span><div class=\"description\">Sold</div>"
            ),
            (false, false)
        );
        assert_eq!(
            signals_of("<span class=\"price\">£0.00</span>"),
            (false, false)
        );
    }

    #[test]
    fn should_ignore_chrome_carts_and_other_products() {
        assert_eq!(
            signals_of(&format!(
                "<header><span class=\"price\">£5</span></header>\
                 <div class=\"mini-cart\"><span class=\"price\">£10</span></div>\
                 <div class=\"related-products\"><span class=\"price\">£20</span>\
                 <div class=\"description\">{PROSE}</div></div>\
                 <footer><div class=\"description\">{PROSE}</div></footer>"
            )),
            (false, false)
        );
    }
}
