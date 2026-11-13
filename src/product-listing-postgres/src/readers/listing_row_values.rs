//! Decoders for persisted ProductListing column values shared by the listing detail readers.
//!
//! Every failure names the column it rejects and keeps the decoding error as its cause, so
//! invalid persisted state stays diagnosable after the reader fails closed.

use application::error::{BoxError, box_error, static_error};
use indexmap::IndexSet;
use localization::{Language, Localized};
use money::{Currency, MonetaryAmount, Price};
use product_listing_core::content_policy::ContentPolicyDecision;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_lifecycle::ListingLifecycle;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_listing_price::ProductListingPrice;
use product_listing_core::title::Title;
use serde::Deserialize;
use url::Url;

/// A persisted listing value that does not decode into its domain type.
#[derive(Debug, thiserror::Error)]
#[error("invalid persisted listing {field}")]
pub(super) struct InvalidListingRowValue {
    field: &'static str,
    #[source]
    source: BoxError,
}

impl InvalidListingRowValue {
    /// Maps a decoding failure of `field`, keeping it as the cause.
    pub(super) fn of<E>(field: &'static str) -> impl FnOnce(E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        move |source| Self {
            field,
            source: box_error(source),
        }
    }

    /// Rejects a stored value of `field` that no decoder error describes, such as an unknown
    /// code or an incomplete combination of columns.
    pub(super) fn rejected(field: &'static str, reason: &'static str) -> Self {
        Self {
            field,
            source: static_error(reason),
        }
    }
}

type Decoded<T> = Result<T, InvalidListingRowValue>;

const PARTIAL_COLUMNS: &str = "related columns must be present or absent together";

#[derive(Debug, Deserialize)]
struct ProductListingImageJson {
    url: String,
}

pub(super) fn localized_title(
    text: Option<String>,
    language: Option<String>,
) -> Decoded<Option<Localized<Language, Title>>> {
    match (text, language) {
        (Some(text), Some(language)) => {
            let language = parse_language("title language", &language)?;
            let title = crate::title::decode_nonempty_title(&text).map_err(|()| {
                InvalidListingRowValue::rejected("title", "title is empty or not canonical")
            })?;
            Ok(title.map(|title| Localized::new(language, title)))
        }
        (None, None) => Ok(None),
        _ => Err(InvalidListingRowValue::rejected("title", PARTIAL_COLUMNS)),
    }
}

pub(super) fn product_listing_price(
    kind: Option<String>,
    amount: Option<i64>,
    currency: Option<String>,
) -> Decoded<Option<ProductListingPrice>> {
    match (kind.as_deref(), amount, currency) {
        (None, None, None) => Ok(None),
        (Some("MONETARY"), Some(amount), Some(currency)) => Ok(Some(
            ProductListingPrice::Monetary(monetary("price", amount, &currency)?),
        )),
        (Some("ON_REQUEST"), None, None) => Ok(Some(ProductListingPrice::OnRequest)),
        _ => Err(InvalidListingRowValue::rejected(
            "price",
            "kind, amount and currency do not form a price",
        )),
    }
}

/// Decodes an optional monetary `field`, such as a price estimate bound.
pub(super) fn price(
    field: &'static str,
    amount: Option<i64>,
    currency: Option<String>,
) -> Decoded<Option<Price>> {
    match (amount, currency) {
        (Some(amount), Some(currency)) => Ok(Some(monetary(field, amount, &currency)?)),
        (None, None) => Ok(None),
        _ => Err(InvalidListingRowValue::rejected(field, PARTIAL_COLUMNS)),
    }
}

fn monetary(field: &'static str, amount: i64, currency: &str) -> Decoded<Price> {
    Ok(Price::new(
        MonetaryAmount::from(u64::try_from(amount).map_err(InvalidListingRowValue::of(field))?),
        parse_currency(field, currency)?,
    ))
}

pub(super) fn images(value: serde_json::Value) -> Decoded<IndexSet<ProductListingImage>> {
    serde_json::from_value::<Vec<ProductListingImageJson>>(value)
        .map_err(InvalidListingRowValue::of("images"))?
        .into_iter()
        .map(|image| {
            Ok(ProductListingImage::new(
                Url::parse(&image.url).map_err(InvalidListingRowValue::of("image URL"))?,
            ))
        })
        .collect()
}

fn parse_language(field: &'static str, value: &str) -> Decoded<Language> {
    Language::from_code(value)
        .ok_or_else(|| InvalidListingRowValue::rejected(field, "unsupported language code"))
}

fn parse_currency(field: &'static str, value: &str) -> Decoded<Currency> {
    Currency::from_code(value)
        .ok_or_else(|| InvalidListingRowValue::rejected(field, "unsupported currency code"))
}

pub(super) fn content_policy(
    decision: Option<&str>,
    category: Option<&str>,
) -> Decoded<Option<ContentPolicyDecision>> {
    match (decision, category) {
        (None, None) => Ok(None),
        (Some(decision), category) => ContentPolicyDecision::from_codes(decision, category)
            .map(Some)
            .ok_or_else(|| {
                InvalidListingRowValue::rejected(
                    "content policy",
                    "unknown decision or category code",
                )
            }),
        (None, Some(_)) => Err(InvalidListingRowValue::rejected(
            "content policy",
            "category without a decision",
        )),
    }
}

pub(super) fn availability(value: Option<&str>) -> Decoded<Option<ListingAvailability>> {
    value
        .map(|value| {
            ListingAvailability::from_code(value).ok_or_else(|| {
                InvalidListingRowValue::rejected("availability", "unknown availability code")
            })
        })
        .transpose()
}

pub(super) fn lifecycle(value: &str) -> Decoded<ListingLifecycle> {
    ListingLifecycle::from_code(value)
        .ok_or_else(|| InvalidListingRowValue::rejected("lifecycle", "unknown lifecycle code"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn should_reject_selected_title_when_text_is_not_canonical() {
        let result = localized_title(Some("title".to_owned()), Some("en".to_owned()));

        assert!(result.is_err());
    }

    #[test]
    fn should_reject_selected_text_when_language_is_unrecognized() {
        let result = localized_title(Some("Title".to_owned()), Some("xx".to_owned()));

        assert!(result.is_err());
    }

    #[test]
    fn should_reject_invalid_persisted_listing_availability() {
        let result = availability(Some("BROKEN"));

        assert!(result.is_err());
    }

    #[test]
    fn should_name_the_field_and_keep_the_cause_of_an_invalid_value() {
        let Err(error) = price("estimate minimum", Some(-1), Some("EUR".to_owned())) else {
            panic!("a negative amount must be rejected");
        };

        assert_eq!(
            "invalid persisted listing estimate minimum",
            error.to_string()
        );
        assert!(
            error
                .source()
                .is_some_and(|source| source.is::<std::num::TryFromIntError>())
        );
    }

    #[test]
    fn should_keep_the_url_parse_error_of_an_invalid_image() {
        let Err(error) = images(serde_json::json!([{ "url": "not a url" }])) else {
            panic!("an invalid image URL must be rejected");
        };

        assert_eq!("invalid persisted listing image URL", error.to_string());
        assert!(
            error
                .source()
                .is_some_and(|source| source.is::<url::ParseError>())
        );
    }
}
