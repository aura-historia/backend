use crate::error::NormalizationFailureScope;
use crate::price::normalize_machine_decimal_price;
use crate::text::{detect_description_language, localize_normalized_title};
use crate::{
    AvailabilityNormalizationError, ImageUrlNormalizationError, ListingAvailabilityQuickCheck,
    NormalizationError, PriceField, PriceNormalizationError, ProductListingNormalizationInput,
    RawProductListingOperation, detect_language, normalize_description, normalize_image_urls,
    normalize_price, normalize_product_listing_price,
    normalize_source_listing_id_with_url_sha_fallback, normalize_title, quick_check_availability,
};
use localization::{Language, Localized};
use money::{Currency, Price};
use product_listing_core::{
    description::Description, product_listing_image::ProductListingImage,
    product_listing_price::ProductListingPrice, source_listing_id::SourceListingId, title::Title,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

use std::collections::BTreeMap;
use strum::IntoEnumIterator;
use url::Url;

pub const PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION: u16 = 1;

/// Stable provider-neutral encoding for raw price patch values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::EnumIter)]
pub enum ProductListingRawValuesPriceFormat {
    DisplayText,
    MachineDecimal,
}

impl ProductListingRawValuesPriceFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DisplayText => "DISPLAY_TEXT",
            Self::MachineDecimal => "MACHINE_DECIMAL",
        }
    }
}

impl Serialize for ProductListingRawValuesPriceFormat {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ProductListingRawValuesPriceFormat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let code = String::deserialize(deserializer)?;
        Self::iter()
            .find(|format| format.as_str() == code.as_str())
            .ok_or_else(|| D::Error::custom("raw price format is unsupported"))
    }
}

/// Provider-neutral protocol for one raw field update.
///
/// `CLEAR` and `UNCHANGED` remain distinct from `SET`, including when a set value
/// normalizes to an empty canonical value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", content = "value", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductListingRawValuesPatch<T> {
    Set(T),
    Clear,
    Unchanged,
}

/// Provider-neutral raw values for one current UPSERT normalization input.
///
/// `priceFormat` is required and applies to the main and estimate price patches. `DISPLAY_TEXT`
/// preserves display-price parsing, while `MACHINE_DECIMAL` uses strict unsigned ASCII decimal
/// parsing with the normalization-context fallback currency for nonblank `SET` values. Blank
/// `SET` values normalize to `CLEAR`. Each mutable listing field uses the explicit
/// set/clear/unchanged protocol. Dynamic attributes use source-selected names and do not add
/// provider-specific fields to this contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProductListingRawValues {
    pub source_listing_id: String,
    pub title: ProductListingRawValuesPatch<String>,
    pub description: ProductListingRawValuesPatch<Vec<String>>,
    pub price_format: ProductListingRawValuesPriceFormat,
    pub price: ProductListingRawValuesPatch<String>,
    pub price_estimate_min: ProductListingRawValuesPatch<String>,
    pub price_estimate_max: ProductListingRawValuesPatch<String>,
    pub availability: ProductListingRawValuesPatch<String>,
    pub url: ProductListingRawValuesPatch<String>,
    pub images: ProductListingRawValuesPatch<Vec<String>>,
    #[serde(default)]
    pub attributes: BTreeMap<String, ProductListingRawValuesPatch<Vec<String>>>,
}

/// Generic normalization inputs that are not provider payload fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProductListingNormalizationContextV1 {
    pub base_url: String,
    #[serde(default)]
    pub fallback_currency: Option<String>,
    #[serde(default)]
    pub fallback_language: Option<String>,
}

/// Deterministically normalized values for an UPSERT observation.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingRawValuesResolved {
    pub shipping: crate::shipping::ShippingQuickCheck,
    pub source_listing_id: SourceListingId,
    pub title: ProductListingRawValuesPatch<Localized<Language, Title>>,
    pub description: ProductListingRawValuesPatch<Localized<Language, Description>>,
    pub price: ProductListingRawValuesPatch<ProductListingPrice>,
    pub price_estimate_min: ProductListingRawValuesPatch<Price>,
    pub price_estimate_max: ProductListingRawValuesPatch<Price>,
    pub availability: ProductListingRawValuesPatch<ListingAvailabilityQuickCheck>,
    pub url: ProductListingRawValuesPatch<Url>,
    pub images: ProductListingRawValuesPatch<Vec<ProductListingImage>>,
    pub attributes: BTreeMap<String, ProductListingRawValuesPatch<Vec<String>>>,
}

/// One complete raw-values normalization result.
#[derive(Debug)]
pub enum ProductListingRawValuesNormalizationOutcome {
    Resolved(Box<ProductListingRawValuesResolved>),
    Invalid(ProductListingRawValuesNormalizationError),
    Delete,
}

/// Typed reason why an UPSERT raw-values input could not normalize.
#[derive(Debug, thiserror::Error)]
pub enum ProductListingRawValuesNormalizationError {
    #[error("raw values schema version is unsupported")]
    UnsupportedRawValuesSchemaVersion { version: u16 },
    #[error("raw values do not match the current contract")]
    InvalidRawValues(#[source] serde_json::Error),
    #[error("normalization context does not match the current contract")]
    InvalidNormalizationContextV1(#[source] serde_json::Error),
    #[error("normalization context base URL is invalid")]
    InvalidBaseUrl(#[source] url::ParseError),
    #[error("listing URL is invalid")]
    InvalidUrl(#[source] url::ParseError),
    #[error(
        "normalization context fallback currency is required for nonblank machine-decimal prices"
    )]
    MachineDecimalFallbackCurrencyRequired,
    #[error("normalization context fallback currency is unsupported")]
    UnsupportedFallbackCurrency,
    #[error("normalization context fallback language is unsupported")]
    UnsupportedFallbackLanguage,
    #[error("source listing ID or text is invalid")]
    Text(#[source] NormalizationError),
    #[error("price is invalid")]
    Price(#[source] NormalizationError),
    #[error("image URL is invalid")]
    ImageUrl(#[source] NormalizationError),
    #[error("availability is invalid")]
    Availability(#[source] NormalizationError),
}

impl ProductListingRawValuesNormalizationError {
    /// Distinguishes terminal candidate data from fail-closed normalizer system failures.
    pub const fn failure_scope(&self) -> NormalizationFailureScope {
        match self {
            Self::Text(error)
            | Self::Price(error)
            | Self::ImageUrl(error)
            | Self::Availability(error) => error.failure_scope(),
            _ => NormalizationFailureScope::CandidateData,
        }
    }
}

/// Pure raw-values normalizer for the current schema.
///
/// DELETE inputs deliberately bypass raw-values decoding and field normalization. Their source
/// record identity belongs to the capture input, not an UPSERT field projection.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProductListingRawValuesNormalizer;

impl ProductListingRawValuesNormalizer {
    pub const fn new() -> Self {
        Self
    }

    pub fn normalize(
        &self,
        input: &ProductListingNormalizationInput,
    ) -> ProductListingRawValuesNormalizationOutcome {
        if input.operation() == RawProductListingOperation::Delete {
            return ProductListingRawValuesNormalizationOutcome::Delete;
        }

        match self.normalize_upsert(input) {
            Ok(resolved) => {
                ProductListingRawValuesNormalizationOutcome::Resolved(Box::new(resolved))
            }
            Err(error) => ProductListingRawValuesNormalizationOutcome::Invalid(error),
        }
    }

    fn normalize_upsert(
        &self,
        input: &ProductListingNormalizationInput,
    ) -> Result<ProductListingRawValuesResolved, ProductListingRawValuesNormalizationError> {
        if input.raw_values_schema_version() != PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION {
            return Err(
                ProductListingRawValuesNormalizationError::UnsupportedRawValuesSchemaVersion {
                    version: input.raw_values_schema_version(),
                },
            );
        }
        let raw: ProductListingRawValues =
            serde_json::from_value(input.raw_values().value().clone())
                .map_err(ProductListingRawValuesNormalizationError::InvalidRawValues)?;
        let shipping = if input.payload_format()
            == crate::RawProductListingPayloadFormat::CrawlerExtractedProduct
        {
            normalize_item_shipping(&raw)
        } else {
            crate::shipping::ShippingQuickCheck::NoAssertion
        };
        let context: ProductListingNormalizationContextV1 = serde_json::from_value(
            input.normalization_context().value().clone(),
        )
        .map_err(ProductListingRawValuesNormalizationError::InvalidNormalizationContextV1)?;
        let base_url = Url::parse(context.base_url.as_str())
            .map_err(ProductListingRawValuesNormalizationError::InvalidBaseUrl)?;
        let fallback_currency = context
            .fallback_currency
            .as_deref()
            .map(|code| {
                Currency::from_code(code)
                    .ok_or(ProductListingRawValuesNormalizationError::UnsupportedFallbackCurrency)
            })
            .transpose()?;
        let price_format = raw.price_format;
        let fallback_language = context
            .fallback_language
            .as_deref()
            .map(|code| {
                Language::from_code(code)
                    .ok_or(ProductListingRawValuesNormalizationError::UnsupportedFallbackLanguage)
            })
            .transpose()?;

        let description_language = match &raw.description {
            ProductListingRawValuesPatch::Set(fragments) => {
                detect_description_language(fragments).or(fallback_language)
            }
            ProductListingRawValuesPatch::Clear | ProductListingRawValuesPatch::Unchanged => {
                fallback_language
            }
        };
        let source_listing_id = normalize_source_listing_id_with_url_sha_fallback(
            raw.source_listing_id.as_str(),
            &base_url,
        )
        .map_err(ProductListingRawValuesNormalizationError::Text)?;
        let title = normalize_title_patch(raw.title, description_language)?;
        let description = normalize_description_patch(
            raw.description,
            title_language(&title).or(fallback_language),
        )?;
        let price = normalize_product_listing_price_patch(
            raw.price,
            fallback_currency,
            price_format,
            PriceField::Price,
        )?;
        let price_estimate_min = normalize_price_patch(
            raw.price_estimate_min,
            fallback_currency,
            price_format,
            PriceField::EstimateMin,
        )?;
        let price_estimate_max = normalize_price_patch(
            raw.price_estimate_max,
            fallback_currency,
            price_format,
            PriceField::EstimateMax,
        )?;
        let availability = normalize_availability_patch(raw.availability)?;
        let url = normalize_url_patch(raw.url, &base_url)?;
        let images = normalize_images_patch(raw.images, &base_url)?;
        Ok(ProductListingRawValuesResolved {
            shipping,
            source_listing_id,
            title,
            description,
            price,
            price_estimate_min,
            price_estimate_max,
            availability,
            url,
            images,
            attributes: raw.attributes,
        })
    }
}

fn normalize_item_shipping(raw: &ProductListingRawValues) -> crate::shipping::ShippingQuickCheck {
    // Numeric legacy fields lack destination/service/basis. Never reinterpret them as rates.
    if ["rawShipmentMin", "rawShipmentMax"].iter().any(|key| {
        matches!(raw.attributes.get(*key), Some(ProductListingRawValuesPatch::Set(v)) if v.iter().any(|s| !s.trim().is_empty()))
    }) {
        return crate::shipping::ShippingQuickCheck::Unsupported;
    }
    let mut fragments = Vec::new();
    for key in ["rawShipment", "rawShipmentNote"] {
        if let Some(ProductListingRawValuesPatch::Set(values)) = raw.attributes.get(key) {
            fragments.extend(values.iter().map(String::as_str));
        }
    }
    // Keep description context too: a contradictory condition must veto an attribute match.
    let description = match &raw.description {
        ProductListingRawValuesPatch::Set(values) => {
            values.iter().map(String::as_str).collect::<Vec<_>>()
        }
        _ => Vec::new(),
    };
    crate::shipping::quick_check_shipping_with_context(&fragments, &description)
}

fn title_language(
    title: &ProductListingRawValuesPatch<Localized<Language, Title>>,
) -> Option<Language> {
    match title {
        ProductListingRawValuesPatch::Set(title) => Some(title.localization),
        ProductListingRawValuesPatch::Clear | ProductListingRawValuesPatch::Unchanged => None,
    }
}

fn normalize_title_patch(
    patch: ProductListingRawValuesPatch<String>,
    description_language: Option<Language>,
) -> Result<
    ProductListingRawValuesPatch<Localized<Language, Title>>,
    ProductListingRawValuesNormalizationError,
> {
    match patch {
        ProductListingRawValuesPatch::Set(raw) => {
            let title = normalize_title(raw.as_str())
                .map_err(ProductListingRawValuesNormalizationError::Text)?;
            let title_language = detect_language(title.as_ref());
            localize_normalized_title(title, title_language, description_language)
                .map(ProductListingRawValuesPatch::Set)
                .map_err(ProductListingRawValuesNormalizationError::Text)
        }
        ProductListingRawValuesPatch::Clear => Ok(ProductListingRawValuesPatch::Clear),
        ProductListingRawValuesPatch::Unchanged => Ok(ProductListingRawValuesPatch::Unchanged),
    }
}

fn normalize_description_patch(
    patch: ProductListingRawValuesPatch<Vec<String>>,
    fallback_language: Option<Language>,
) -> Result<
    ProductListingRawValuesPatch<Localized<Language, Description>>,
    ProductListingRawValuesNormalizationError,
> {
    match patch {
        ProductListingRawValuesPatch::Set(raw) => normalize_description(raw, fallback_language)
            .map(|description| match description {
                Some(description) => ProductListingRawValuesPatch::Set(description),
                None => ProductListingRawValuesPatch::Clear,
            })
            .map_err(ProductListingRawValuesNormalizationError::Text),
        ProductListingRawValuesPatch::Clear => Ok(ProductListingRawValuesPatch::Clear),
        ProductListingRawValuesPatch::Unchanged => Ok(ProductListingRawValuesPatch::Unchanged),
    }
}

fn normalize_product_listing_price_patch(
    patch: ProductListingRawValuesPatch<String>,
    fallback_currency: Option<Currency>,
    price_format: ProductListingRawValuesPriceFormat,
    field: PriceField,
) -> Result<
    ProductListingRawValuesPatch<ProductListingPrice>,
    ProductListingRawValuesNormalizationError,
> {
    match patch {
        ProductListingRawValuesPatch::Set(raw) if raw.trim().is_empty() => {
            Ok(ProductListingRawValuesPatch::Clear)
        }
        ProductListingRawValuesPatch::Set(raw) => match price_format {
            ProductListingRawValuesPriceFormat::DisplayText => {
                normalize_product_listing_price(Some(raw.as_str()), fallback_currency)
                    .map(|price| match price {
                        Some(price) => ProductListingRawValuesPatch::Set(price),
                        None => ProductListingRawValuesPatch::Clear,
                    })
                    .map_err(|error| map_price_error(error, field))
            }
            ProductListingRawValuesPriceFormat::MachineDecimal => {
                let currency = fallback_currency.ok_or(
                    ProductListingRawValuesNormalizationError::MachineDecimalFallbackCurrencyRequired,
                )?;
                normalize_machine_decimal_price(raw.as_str(), currency)
                    .map(ProductListingPrice::from)
                    .map(ProductListingRawValuesPatch::Set)
                    .map_err(|error| map_price_error(error, field))
            }
        },
        ProductListingRawValuesPatch::Clear => Ok(ProductListingRawValuesPatch::Clear),
        ProductListingRawValuesPatch::Unchanged => Ok(ProductListingRawValuesPatch::Unchanged),
    }
}

fn normalize_price_patch(
    patch: ProductListingRawValuesPatch<String>,
    fallback_currency: Option<Currency>,
    price_format: ProductListingRawValuesPriceFormat,
    field: PriceField,
) -> Result<ProductListingRawValuesPatch<Price>, ProductListingRawValuesNormalizationError> {
    match patch {
        ProductListingRawValuesPatch::Set(raw) if raw.trim().is_empty() => {
            Ok(ProductListingRawValuesPatch::Clear)
        }
        ProductListingRawValuesPatch::Set(raw) => match price_format {
            ProductListingRawValuesPriceFormat::DisplayText => {
                normalize_price(Some(raw.as_str()), fallback_currency)
                    .map(|price| match price {
                        Some(price) => ProductListingRawValuesPatch::Set(price),
                        None => ProductListingRawValuesPatch::Clear,
                    })
                    .map_err(|error| map_price_error(error, field))
            }
            ProductListingRawValuesPriceFormat::MachineDecimal => {
                let currency = fallback_currency.ok_or(
                    ProductListingRawValuesNormalizationError::MachineDecimalFallbackCurrencyRequired,
                )?;
                normalize_machine_decimal_price(raw.as_str(), currency)
                    .map(ProductListingRawValuesPatch::Set)
                    .map_err(|error| map_price_error(error, field))
            }
        },
        ProductListingRawValuesPatch::Clear => Ok(ProductListingRawValuesPatch::Clear),
        ProductListingRawValuesPatch::Unchanged => Ok(ProductListingRawValuesPatch::Unchanged),
    }
}

fn normalize_availability_patch(
    patch: ProductListingRawValuesPatch<String>,
) -> Result<
    ProductListingRawValuesPatch<ListingAvailabilityQuickCheck>,
    ProductListingRawValuesNormalizationError,
> {
    match patch {
        ProductListingRawValuesPatch::Set(raw) => quick_check_availability(raw.as_str())
            .map(ProductListingRawValuesPatch::Set)
            .map_err(map_availability_error),
        ProductListingRawValuesPatch::Clear => Ok(ProductListingRawValuesPatch::Clear),
        ProductListingRawValuesPatch::Unchanged => Ok(ProductListingRawValuesPatch::Unchanged),
    }
}

fn normalize_url_patch(
    patch: ProductListingRawValuesPatch<String>,
    base_url: &Url,
) -> Result<ProductListingRawValuesPatch<Url>, ProductListingRawValuesNormalizationError> {
    match patch {
        ProductListingRawValuesPatch::Set(raw) => Url::parse(raw.as_str())
            .or_else(|_| base_url.join(raw.as_str()))
            .map(ProductListingRawValuesPatch::Set)
            .map_err(ProductListingRawValuesNormalizationError::InvalidUrl),
        ProductListingRawValuesPatch::Clear => Ok(ProductListingRawValuesPatch::Clear),
        ProductListingRawValuesPatch::Unchanged => Ok(ProductListingRawValuesPatch::Unchanged),
    }
}

fn normalize_images_patch(
    patch: ProductListingRawValuesPatch<Vec<String>>,
    base_url: &Url,
) -> Result<
    ProductListingRawValuesPatch<Vec<ProductListingImage>>,
    ProductListingRawValuesNormalizationError,
> {
    match patch {
        ProductListingRawValuesPatch::Set(raw) => normalize_image_urls(raw, base_url)
            .map(ProductListingRawValuesPatch::Set)
            .map_err(map_image_error),
        ProductListingRawValuesPatch::Clear => Ok(ProductListingRawValuesPatch::Clear),
        ProductListingRawValuesPatch::Unchanged => Ok(ProductListingRawValuesPatch::Unchanged),
    }
}

fn map_price_error(
    error: PriceNormalizationError,
    field: PriceField,
) -> ProductListingRawValuesNormalizationError {
    let error = match error {
        PriceNormalizationError::UnknownCurrency => {
            NormalizationError::PriceUnknownCurrency { field }
        }
        PriceNormalizationError::ParseFailure => NormalizationError::PriceParseError { field },
    };
    ProductListingRawValuesNormalizationError::Price(error)
}

fn map_image_error(error: ImageUrlNormalizationError) -> ProductListingRawValuesNormalizationError {
    let error = match error {
        ImageUrlNormalizationError::InvalidUrl(source) => {
            NormalizationError::InvalidImageUrl(source)
        }
    };
    ProductListingRawValuesNormalizationError::ImageUrl(error)
}

fn map_availability_error(
    error: AvailabilityNormalizationError,
) -> ProductListingRawValuesNormalizationError {
    let error = match error {
        AvailabilityNormalizationError::InputTooLong { len, max } => {
            NormalizationError::AvailabilityTextTooLong { len, max }
        }
        AvailabilityNormalizationError::EmbeddedNul => {
            NormalizationError::AvailabilityTextEmbeddedNul
        }
        AvailabilityNormalizationError::RegexSetCompilationFailed => {
            NormalizationError::AvailabilityRegexSetCompilationFailed
        }
    };
    ProductListingRawValuesNormalizationError::Availability(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        NormalizationContext, RawProductListingPayloadFormat, RawProductListingValues,
        SourcePayload,
    };
    use serde_json::json;

    fn input(raw_values: serde_json::Value) -> ProductListingNormalizationInput {
        ProductListingNormalizationInput::new(
            RawProductListingOperation::Upsert,
            RawProductListingPayloadFormat::CrawlerExtractedProduct,
            1,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            SourcePayload::new(json!({})).unwrap_or_else(|error| panic!("source payload: {error}")),
            RawProductListingValues::new(raw_values).unwrap_or_else(|error| panic!("raw values: {error}")),
            NormalizationContext::new(json!({"baseUrl": "https://example.test/", "fallbackCurrency": "EUR", "fallbackLanguage": "en"})).unwrap_or_else(|error| panic!("context: {error}")),
        ).unwrap_or_else(|error| panic!("input: {error}"))
    }

    #[test]
    fn should_normalize_generic_raw_values_without_auction_contract() {
        let outcome = ProductListingRawValuesNormalizer::new().normalize(&input(json!({
            "sourceListingId": "listing-123", "title": {"action": "SET", "value": "Vase"},
            "description": {"action": "CLEAR"}, "priceFormat": "DISPLAY_TEXT",
            "price": {"action": "SET", "value": "EUR 100"},
            "priceEstimateMin": {"action": "UNCHANGED"}, "priceEstimateMax": {"action": "UNCHANGED"},
            "availability": {"action": "SET", "value": "in stock"}, "url": {"action": "SET", "value": "listing-123"},
            "images": {"action": "SET", "value": []}
        })));
        assert!(matches!(
            outcome,
            ProductListingRawValuesNormalizationOutcome::Resolved(_)
        ));
    }

    #[test]
    fn should_reject_auction_fields_from_current_raw_contract() {
        let outcome = ProductListingRawValuesNormalizer::new().normalize(&input(json!({
            "sourceListingId": "listing-123", "title": {"action": "SET", "value": "Vase"},
            "description": {"action": "CLEAR"}, "priceFormat": "DISPLAY_TEXT", "price": {"action": "CLEAR"},
            "priceEstimateMin": {"action": "UNCHANGED"}, "priceEstimateMax": {"action": "UNCHANGED"},
            "availability": {"action": "UNCHANGED"}, "url": {"action": "SET", "value": "listing-123"},
            "images": {"action": "SET", "value": []}, "auction": {"action": "UNCHANGED"}
        })));
        assert!(matches!(
            outcome,
            ProductListingRawValuesNormalizationOutcome::Invalid(
                ProductListingRawValuesNormalizationError::InvalidRawValues(_)
            )
        ));
    }

    #[test]
    fn shipping_context_is_conservative_and_optional() {
        let values = json!({
            "sourceListingId": "item-1", "title": {"action":"SET", "value":"An antique English vase"},
            "description": {"action":"SET", "value":["A restored ceramic vase."]},
            "priceFormat":"DISPLAY_TEXT", "price":{"action":"CLEAR"},
            "priceEstimateMin":{"action":"UNCHANGED"}, "priceEstimateMax":{"action":"UNCHANGED"},
            "availability":{"action":"UNCHANGED"}, "url":{"action":"SET", "value":"item-1"},
            "images":{"action":"SET", "value":[]},
            "attributes":{"rawShipment":{"action":"SET", "value":["This item is collection only"]}}
        });
        let resolved_shipping = |values| {
            let ProductListingRawValuesNormalizationOutcome::Resolved(v) =
                ProductListingRawValuesNormalizer::new().normalize(&input(values))
            else {
                panic!("optional shipping must not reject valid catalog data");
            };
            v.shipping
        };
        assert!(
            matches!(resolved_shipping(values.clone()), crate::shipping::ShippingQuickCheck::Resolved(a) if a.prohibited)
        );
        let mut contradictory = values.clone();
        contradictory["description"]["value"] =
            json!(["Shipping for this item is available worldwide"]);
        assert_eq!(
            crate::shipping::ShippingQuickCheck::Unsupported,
            resolved_shipping(contradictory)
        );
        let mut unscoped_price = values.clone();
        unscoped_price["attributes"]["rawShipmentMin"] =
            json!({"action":"SET", "value":["EUR 80"]});
        assert_eq!(
            crate::shipping::ShippingQuickCheck::Unsupported,
            resolved_shipping(unscoped_price)
        );
        let mut oversized = values.clone();
        oversized["attributes"]["rawShipment"]["value"] =
            json!(["x".repeat(crate::shipping::MAX_SHIPPING_TEXT_BYTES + 1)]);
        assert_eq!(
            crate::shipping::ShippingQuickCheck::Unsupported,
            resolved_shipping(oversized)
        );
        let mut cleared = values.clone();
        cleared["attributes"]["rawShipment"] = json!({"action":"CLEAR"});
        assert_eq!(
            crate::shipping::ShippingQuickCheck::NoAssertion,
            resolved_shipping(cleared)
        );
        let historical = input(values);
        for format in [
            crate::RawProductListingPayloadFormat::ShopifyProduct,
            crate::RawProductListingPayloadFormat::WoocommerceProduct,
        ] {
            let native = ProductListingNormalizationInput::new(
                historical.operation(),
                format,
                1,
                1,
                historical.source_payload().clone(),
                historical.raw_values().clone(),
                historical.normalization_context().clone(),
            )
            .unwrap();
            let ProductListingRawValuesNormalizationOutcome::Resolved(v) =
                ProductListingRawValuesNormalizer::new().normalize(&native)
            else {
                panic!("native catalog");
            };
            assert_eq!(crate::shipping::ShippingQuickCheck::NoAssertion, v.shipping);
        }
    }
}
