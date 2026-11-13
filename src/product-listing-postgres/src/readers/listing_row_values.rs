//! Decoders for persisted ProductListing column values shared by the listing detail readers.

use indexmap::IndexSet;
use localization::{Language, Localized};
use money::{Currency, MonetaryAmount, Price};
use product_listing_core::content_policy::ContentPolicyDecision;
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_lifecycle::ListingLifecycle;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::title::Title;
use serde::Deserialize;
use url::Url;

#[derive(Debug, Deserialize)]
struct ProductListingImageJson {
    url: String,
}

pub(super) fn localized_title(
    text: Option<String>,
    language: Option<String>,
) -> Result<Option<Localized<Language, Title>>, ()> {
    match (text, language) {
        (Some(text), Some(language)) => {
            let language = parse_language(&language)?;
            Ok(crate::title::decode_nonempty_title(&text)?
                .map(|title| Localized::new(language, title)))
        }
        (None, None) => Ok(None),
        _ => Err(()),
    }
}

pub(super) fn localized_description(
    text: Option<String>,
    language: Option<String>,
) -> Result<Option<Localized<Language, Description>>, ()> {
    match (text, language) {
        (Some(text), Some(language)) => {
            let description = Description::from(text.as_str());
            if description.as_ref().is_empty() || description.as_ref() != text.as_str() {
                return Err(());
            }
            Ok(Some(Localized::new(
                parse_language(&language)?,
                description,
            )))
        }
        (None, None) => Ok(None),
        _ => Err(()),
    }
}

pub(super) fn product_listing_price(
    kind: Option<String>,
    amount: Option<i64>,
    currency: Option<String>,
) -> Result<Option<product_listing_core::product_listing_price::ProductListingPrice>, ()> {
    match (kind.as_deref(), amount, currency) {
        (None, None, None) => Ok(None),
        (Some("MONETARY"), Some(amount), Some(currency)) => Ok(Some(
            product_listing_core::product_listing_price::ProductListingPrice::Monetary(Price::new(
                MonetaryAmount::from(u64::try_from(amount).map_err(|_| ())?),
                parse_currency(&currency)?,
            )),
        )),
        (Some("ON_REQUEST"), None, None) => Ok(Some(
            product_listing_core::product_listing_price::ProductListingPrice::OnRequest,
        )),
        _ => Err(()),
    }
}

pub(super) fn price(amount: Option<i64>, currency: Option<String>) -> Result<Option<Price>, ()> {
    match (amount, currency) {
        (Some(amount), Some(currency)) => Ok(Some(Price::new(
            MonetaryAmount::from(u64::try_from(amount).map_err(|_| ())?),
            parse_currency(&currency)?,
        ))),
        (None, None) => Ok(None),
        _ => Err(()),
    }
}

pub(super) fn images(value: serde_json::Value) -> Result<IndexSet<ProductListingImage>, ()> {
    serde_json::from_value::<Vec<ProductListingImageJson>>(value)
        .map_err(|_| ())?
        .into_iter()
        .map(|image| {
            Ok(ProductListingImage::new(
                Url::parse(&image.url).map_err(|_| ())?,
            ))
        })
        .collect()
}

fn parse_language(value: &str) -> Result<Language, ()> {
    Language::from_code(value).ok_or(())
}

fn parse_currency(value: &str) -> Result<Currency, ()> {
    Currency::from_code(value).ok_or(())
}

pub(super) fn content_policy(
    decision: Option<&str>,
    category: Option<&str>,
) -> Result<Option<ContentPolicyDecision>, ()> {
    match (decision, category) {
        (None, None) => Ok(None),
        (Some(decision), category) => ContentPolicyDecision::from_codes(decision, category)
            .map(Some)
            .ok_or(()),
        (None, Some(_)) => Err(()),
    }
}

pub(super) fn availability(value: Option<&str>) -> Result<Option<ListingAvailability>, ()> {
    value
        .map(|value| ListingAvailability::from_code(value).ok_or(()))
        .transpose()
}

pub(super) fn lifecycle(value: &str) -> Result<ListingLifecycle, ()> {
    ListingLifecycle::from_code(value).ok_or(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_selected_title_when_text_is_not_canonical() {
        let result = localized_title(Some("title".to_owned()), Some("en".to_owned()));

        assert!(result.is_err());
    }

    #[test]
    fn should_reject_selected_description_when_text_is_empty() {
        let result = localized_description(Some(" ".to_owned()), Some("en".to_owned()));

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
}
