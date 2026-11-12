//! The rules a structured-data draft tries per field, in order: the page's own JSON-LD where it
//! holds machine values (price, availability, identity), then the markup common shop platforms
//! (WooCommerce, Shopify, Shopware) and schema.org microdata give the visible element, then the
//! generic element. Description and images come only from visible elements: shops shorten the
//! JSON-LD description and list one image of a larger gallery there.

use crate::scraper::css_selector::rule::{
    CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule, HtmlAttributeName,
    JsonLdOfferProperty, JsonLdProductProperty,
};

const JSON_LD_SCRIPT: &str = "script[type='application/ld+json']";

pub(super) fn title() -> Vec<ExtractionRule> {
    [
        "h1.product_title",
        "h1.product-single__title",
        ".product__title h1",
        "h1.product__title",
        "h1.product-meta__title",
        "h1.product-detail-name",
        "h1.product-title",
        "h1[itemprop='name']",
        ".product-title h1",
        "h1",
    ]
    .into_iter()
    .map(|selector| text(&[selector], ExtractionCardinality::First))
    .collect()
}

pub(super) fn json_ld_price() -> ExtractionRule {
    rule(
        &[JSON_LD_SCRIPT],
        ExtractionKind::JsonLdOffer {
            property: JsonLdOfferProperty::Price,
        },
        ExtractionCardinality::First,
    )
}

pub(super) fn visible_price() -> Vec<ExtractionRule> {
    let mut rules = vec![attribute_or_text(&["[itemprop='price']"], "content")];
    rules.extend(
        [
            ".summary .price",
            ".product-info .price",
            ".price__regular .price-item--regular",
            ".product__price",
            ".product-single__price",
            ".product-detail-price",
            ".product-price",
            ".price",
        ]
        .into_iter()
        .map(|selector| text(&[selector], ExtractionCardinality::First)),
    );
    rules
}

pub(super) fn json_ld_availability() -> ExtractionRule {
    rule(
        &[JSON_LD_SCRIPT],
        ExtractionKind::JsonLdOffer {
            property: JsonLdOfferProperty::Availability,
        },
        ExtractionCardinality::First,
    )
}

/// Visible availability labels and cart actions. A WooCommerce page shows a stock line on sold
/// pages, and on live ones only with stock management, so its cart button follows as a fallback.
pub(super) fn visible_state() -> Vec<ExtractionRule> {
    vec![
        // The schema.org availability where a page marks it up, the visible stock line or
        // delivery note where it does not, such as Shopware's sold notice beside live pages'
        // `InStock` link.
        attribute_or_text(
            &[
                "link[itemprop='availability']",
                ".product-detail__out-of-stock",
                ".product-detail-buy .delivery-information",
                ".summary p.stock",
            ],
            "href",
        ),
        text(
            &[".summary p.stock", ".summary .single_add_to_cart_button"],
            ExtractionCardinality::First,
        ),
        attribute_or_text(&["link[itemprop='availability']"], "href"),
        attribute_or_text(&["[itemprop='availability']"], "content"),
        attribute_or_text(&["button[name='add']", ".product-form__submit"], "value"),
        text(
            &[".product-detail-buy .delivery-information", ".btn-buy"],
            ExtractionCardinality::First,
        ),
        text(
            &["form.cart button[type='submit']"],
            ExtractionCardinality::First,
        ),
    ]
}

pub(super) fn source_listing_id() -> Vec<ExtractionRule> {
    vec![
        attribute(&["input[name='product-id']"], "value"),
        attribute(&["input[name='product_id']"], "value"),
        attribute(&["button[name='add-to-cart']"], "value"),
        attribute(&["[data-product-id]"], "data-product-id"),
        rule(
            &[JSON_LD_SCRIPT],
            ExtractionKind::JsonLdProduct {
                property: JsonLdProductProperty::ProductId,
            },
            ExtractionCardinality::First,
        ),
        attribute_or_text(&["[itemprop='productID']"], "content"),
        rule(
            &[JSON_LD_SCRIPT],
            ExtractionKind::JsonLdProduct {
                property: JsonLdProductProperty::Sku,
            },
            ExtractionCardinality::First,
        ),
        attribute_or_text(&["[itemprop='sku']"], "content"),
    ]
}

/// WooCommerce splits a description into a short summary and the description tab, so its rule
/// collects both.
pub(super) fn description() -> Vec<ExtractionRule> {
    let mut rules = vec![text(
        &[
            ".woocommerce-product-details__short-description",
            "#tab-description",
        ],
        ExtractionCardinality::All,
    )];
    rules.extend(
        [
            ".product__description",
            ".et_pb_wc_description",
            ".product-single__description",
            ".product-detail-description-text",
            ".product-description",
            "[itemprop='description']",
            ".description",
        ]
        .into_iter()
        .map(|selector| text(&[selector], ExtractionCardinality::All)),
    );
    rules
}

pub(super) fn images() -> Vec<ExtractionRule> {
    [
        ".woocommerce-product-gallery__image img",
        "a.wcgs-slider-lightbox",
        ".product__media-list img",
        ".product__media img",
        ".product-single__media img",
        ".product-gallery img",
        ".product__photos img",
        ".gallery-slider-image",
        ".product-detail-media img",
        ".product-images img",
        "img[itemprop='image']",
    ]
    .into_iter()
    .map(|selector| {
        rule(
            &[selector],
            ExtractionKind::ImageUrl,
            ExtractionCardinality::All,
        )
    })
    .collect()
}

fn text(selectors: &[&str], cardinality: ExtractionCardinality) -> ExtractionRule {
    rule(selectors, ExtractionKind::Text, cardinality)
}

fn attribute(selectors: &[&str], name: &str) -> ExtractionRule {
    rule(
        selectors,
        ExtractionKind::Attribute {
            name: HtmlAttributeName::from(name),
        },
        ExtractionCardinality::First,
    )
}

fn attribute_or_text(selectors: &[&str], name: &str) -> ExtractionRule {
    rule(
        selectors,
        ExtractionKind::AttributeOrText {
            name: HtmlAttributeName::from(name),
        },
        ExtractionCardinality::First,
    )
}

fn rule(
    selectors: &[&str],
    extract: ExtractionKind,
    cardinality: ExtractionCardinality,
) -> ExtractionRule {
    let mut selectors = selectors
        .iter()
        .map(|selector| CssSelector::from(*selector));
    ExtractionRule {
        selector: selectors
            .next()
            .unwrap_or_else(|| CssSelector::from("body")),
        additional_selectors: selectors.collect(),
        extract,
        cardinality,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_offer_only_rules_that_validate() {
        let every = title()
            .into_iter()
            .chain([json_ld_price(), json_ld_availability()])
            .chain(visible_price())
            .chain(visible_state())
            .chain(source_listing_id())
            .chain(description())
            .chain(images());
        for rule in every {
            assert!(rule.validate().is_ok(), "{rule:?}");
        }
    }
}
