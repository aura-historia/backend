use crate::diagnostics::{
    NormalizationField, NormalizationIssue, NormalizationIssueDetails, NormalizationIssueHandling,
    NormalizationIssueReason, NormalizationReport, availability_normalization_error,
    price_normalization_error,
};
use crate::error::NormalizationFailureScope;
use crate::price::normalize_machine_decimal_price;
use crate::text::{detect_description_language, localize_normalized_title};
use crate::{
    ImageUrlNormalizationError, ListingAvailabilityQuickCheck, NormalizationError, PriceField,
    PriceNormalizationError, ProductListingNormalizationInput, RawProductListingOperation,
    detect_language, normalize_description, normalize_image_urls, normalize_price,
    normalize_product_listing_price, normalize_source_listing_id_with_url_sha_fallback,
    normalize_title, quick_check_availability,
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

/// Provenance version of the raw-values normalization heuristics. Bump it when the decision for
/// an existing input can change. It never triggers reprocessing of completed revisions.
pub const PRODUCT_LISTING_RAW_VALUES_NORMALIZER_VERSION: u16 = 1;

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
    Delete {
        source_listing_id: Option<SourceListingId>,
    },
}

/// Typed reason why a raw-values input could not normalize.
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

    /// Typed diagnostic for this error. `field` names the evaluated field for errors whose
    /// variant does not identify one (for example a machine-decimal estimate bound).
    pub fn issue(&self, field: NormalizationField) -> NormalizationIssue {
        use NormalizationIssueReason as Reason;
        let none = NormalizationIssueDetails::None;
        match self {
            Self::UnsupportedRawValuesSchemaVersion { version } => NormalizationIssue::envelope(
                NormalizationField::RawValuesSchemaVersion,
                Reason::RawValuesSchemaUnsupported,
                NormalizationIssueDetails::SchemaVersion {
                    observed: *version,
                    supported: PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
                },
            ),
            Self::InvalidRawValues(source) => NormalizationIssue::envelope(
                NormalizationField::RawValues,
                Reason::RawValuesInvalid,
                NormalizationIssueDetails::json_contract(source),
            ),
            Self::InvalidNormalizationContextV1(source) => NormalizationIssue::envelope(
                NormalizationField::NormalizationContext,
                Reason::NormalizationContextInvalid,
                NormalizationIssueDetails::json_contract(source),
            ),
            Self::InvalidBaseUrl(source) => NormalizationIssue::envelope(
                NormalizationField::BaseUrl,
                Reason::BaseUrlInvalid,
                NormalizationIssueDetails::url_parse(source),
            ),
            Self::UnsupportedFallbackCurrency => NormalizationIssue::envelope(
                NormalizationField::FallbackCurrency,
                Reason::FallbackCurrencyUnsupported,
                none,
            ),
            Self::UnsupportedFallbackLanguage => NormalizationIssue::envelope(
                NormalizationField::FallbackLanguage,
                Reason::FallbackLanguageUnsupported,
                none,
            ),
            Self::InvalidUrl(source) => NormalizationIssue::field(
                NormalizationField::Url,
                Reason::UrlInvalid,
                NormalizationIssueHandling::RejectedInput,
                NormalizationIssueDetails::url_parse(source),
            ),
            Self::MachineDecimalFallbackCurrencyRequired => NormalizationIssue::field(
                field,
                Reason::MachineDecimalFallbackCurrencyRequired,
                NormalizationIssueHandling::RejectedInput,
                none,
            ),
            Self::Text(error)
            | Self::Price(error)
            | Self::ImageUrl(error)
            | Self::Availability(error) => NormalizationIssue::from_normalization_error(error),
        }
    }
}

/// One complete raw-values evaluation: the unchanged decision plus every observed issue.
///
/// Issues include soft misses inside resolved outcomes and independent field failures that the
/// decision does not name. An envelope failure that prevents field evaluation is reported once.
#[derive(Debug)]
pub struct ProductListingRawValuesEvaluation {
    pub outcome: ProductListingRawValuesNormalizationOutcome,
    pub report: NormalizationReport,
}

/// Pure raw-values normalizer for the current schema.
///
/// DELETE inputs decode only an optional canonical identity and bypass UPSERT field normalization.
/// Without an explicit identity, deletion targets the stream's existing canonical binding.
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
        self.evaluate(input).outcome
    }

    /// Production decision plus diagnostics. Replay and the worker both call this function.
    pub fn evaluate(
        &self,
        input: &ProductListingNormalizationInput,
    ) -> ProductListingRawValuesEvaluation {
        let mut report = NormalizationReport::new();
        if input.operation() == RawProductListingOperation::Delete {
            let outcome = match input
                .raw_values()
                .value()
                .get("sourceListingId")
                .map(|value| serde_json::from_value::<SourceListingId>(value.clone()))
                .transpose()
            {
                Ok(source_listing_id) => {
                    ProductListingRawValuesNormalizationOutcome::Delete { source_listing_id }
                }
                Err(source) => {
                    let error = ProductListingRawValuesNormalizationError::InvalidRawValues(source);
                    report.push(error.issue(NormalizationField::SourceListingId));
                    ProductListingRawValuesNormalizationOutcome::Invalid(error)
                }
            };
            return ProductListingRawValuesEvaluation { outcome, report };
        }

        let outcome = match self.normalize_upsert(input, &mut report) {
            Ok(resolved) => {
                ProductListingRawValuesNormalizationOutcome::Resolved(Box::new(resolved))
            }
            Err(error) => ProductListingRawValuesNormalizationOutcome::Invalid(error),
        };
        ProductListingRawValuesEvaluation { outcome, report }
    }

    fn normalize_upsert(
        &self,
        input: &ProductListingNormalizationInput,
        report: &mut NormalizationReport,
    ) -> Result<ProductListingRawValuesResolved, ProductListingRawValuesNormalizationError> {
        let envelope = |error: ProductListingRawValuesNormalizationError,
                        report: &mut NormalizationReport| {
            report.push(error.issue(NormalizationField::RawValues));
            error
        };
        if input.raw_values_schema_version() != PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION {
            return Err(envelope(
                ProductListingRawValuesNormalizationError::UnsupportedRawValuesSchemaVersion {
                    version: input.raw_values_schema_version(),
                },
                report,
            ));
        }
        let raw: ProductListingRawValues =
            serde_json::from_value(input.raw_values().value().clone()).map_err(|error| {
                envelope(
                    ProductListingRawValuesNormalizationError::InvalidRawValues(error),
                    report,
                )
            })?;
        // Shipping depends only on raw values, so its diagnostics survive later failures.
        let shipping = if input.payload_format()
            == crate::RawProductListingPayloadFormat::CrawlerExtractedProduct
        {
            normalize_item_shipping(&raw)
        } else {
            crate::shipping::ShippingQuickCheck::NoAssertion
        };
        if let crate::shipping::ShippingQuickCheck::Unsupported(reason) = &shipping {
            report.push(NormalizationIssue::shipping_unsupported(reason));
        }
        let context: ProductListingNormalizationContextV1 = serde_json::from_value(
            input.normalization_context().value().clone(),
        )
        .map_err(|error| {
            envelope(
                ProductListingRawValuesNormalizationError::InvalidNormalizationContextV1(error),
                report,
            )
        })?;
        let base_url = Url::parse(context.base_url.as_str()).map_err(|error| {
            envelope(
                ProductListingRawValuesNormalizationError::InvalidBaseUrl(error),
                report,
            )
        })?;
        let fallback_currency = context
            .fallback_currency
            .as_deref()
            .map(|code| {
                Currency::from_code(code)
                    .ok_or(ProductListingRawValuesNormalizationError::UnsupportedFallbackCurrency)
            })
            .transpose()
            .map_err(|error| envelope(error, report))?;
        let price_format = raw.price_format;
        let fallback_language = context
            .fallback_language
            .as_deref()
            .map(|code| {
                Language::from_code(code)
                    .ok_or(ProductListingRawValuesNormalizationError::UnsupportedFallbackLanguage)
            })
            .transpose()
            .map_err(|error| envelope(error, report))?;

        let description_language = match &raw.description {
            ProductListingRawValuesPatch::Set(fragments) => {
                detect_description_language(fragments).or(fallback_language)
            }
            ProductListingRawValuesPatch::Clear | ProductListingRawValuesPatch::Unchanged => {
                fallback_language
            }
        };

        // Every field is evaluated independently. The decision keeps the first failure in
        // field order, while the report retains every independent field issue.
        let mut fields = FieldResults::new(report);
        let source_listing_id = fields.take(NormalizationField::SourceListingId, |_| {
            normalize_source_listing_id_with_url_sha_fallback(
                raw.source_listing_id.as_str(),
                &base_url,
            )
            .map_err(ProductListingRawValuesNormalizationError::Text)
        });
        let title = fields.take(NormalizationField::Title, |_| {
            normalize_title_patch(raw.title, description_language)
        });
        let title_language = title.as_ref().and_then(title_language);
        let description = fields.take(NormalizationField::Description, |_| {
            normalize_description_patch(raw.description, title_language.or(fallback_language))
        });
        let price = fields.take(NormalizationField::Price, |report| {
            normalize_product_listing_price_patch(
                raw.price,
                fallback_currency,
                price_format,
                PriceField::Price,
                report,
            )
        });
        let price_estimate_min = fields.take(NormalizationField::PriceEstimateMin, |report| {
            normalize_price_patch(
                raw.price_estimate_min,
                fallback_currency,
                price_format,
                PriceField::EstimateMin,
                report,
            )
        });
        let price_estimate_max = fields.take(NormalizationField::PriceEstimateMax, |report| {
            normalize_price_patch(
                raw.price_estimate_max,
                fallback_currency,
                price_format,
                PriceField::EstimateMax,
                report,
            )
        });
        let availability = fields.take(NormalizationField::Availability, |report| {
            normalize_availability_patch(raw.availability, report)
        });
        let url = fields.take(NormalizationField::Url, |_| {
            normalize_url_patch(raw.url, &base_url)
        });
        let images = fields.take(NormalizationField::Images, |_| {
            normalize_images_patch(raw.images, &base_url)
        });
        if let Some(error) = fields.first_error {
            return Err(error);
        }
        match (
            source_listing_id,
            title,
            description,
            price,
            price_estimate_min,
            price_estimate_max,
            availability,
            url,
            images,
        ) {
            (
                Some(source_listing_id),
                Some(title),
                Some(description),
                Some(price),
                Some(price_estimate_min),
                Some(price_estimate_max),
                Some(availability),
                Some(url),
                Some(images),
            ) => Ok(ProductListingRawValuesResolved {
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
            }),
            _ => unreachable!("every missing field result records its error"),
        }
    }
}

/// Collects independent field results without short-circuiting.
struct FieldResults<'a> {
    report: &'a mut NormalizationReport,
    first_error: Option<ProductListingRawValuesNormalizationError>,
}

impl<'a> FieldResults<'a> {
    const fn new(report: &'a mut NormalizationReport) -> Self {
        Self {
            report,
            first_error: None,
        }
    }

    fn take<T>(
        &mut self,
        field: NormalizationField,
        evaluate: impl FnOnce(
            &mut NormalizationReport,
        ) -> Result<T, ProductListingRawValuesNormalizationError>,
    ) -> Option<T> {
        match evaluate(self.report) {
            Ok(value) => Some(value),
            Err(error) => {
                self.report.push(error.issue(field));
                self.first_error.get_or_insert(error);
                None
            }
        }
    }
}

fn normalize_item_shipping(raw: &ProductListingRawValues) -> crate::shipping::ShippingQuickCheck {
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

/// A supplied display price without determinable currency is unresolved, not absent: the
/// canonical field stays unchanged and the report names the reason.
fn normalize_product_listing_price_patch(
    patch: ProductListingRawValuesPatch<String>,
    fallback_currency: Option<Currency>,
    price_format: ProductListingRawValuesPriceFormat,
    field: PriceField,
    report: &mut NormalizationReport,
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
                match normalize_product_listing_price(Some(raw.as_str()), fallback_currency) {
                    Ok(Some(price)) => Ok(ProductListingRawValuesPatch::Set(price)),
                    Ok(None) => Ok(ProductListingRawValuesPatch::Clear),
                    Err(PriceNormalizationError::UnknownCurrency) => {
                        report.push(NormalizationIssue::price_unknown_currency(field));
                        Ok(ProductListingRawValuesPatch::Unchanged)
                    }
                    Err(error) => Err(map_price_error(error, field)),
                }
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
    report: &mut NormalizationReport,
) -> Result<ProductListingRawValuesPatch<Price>, ProductListingRawValuesNormalizationError> {
    match patch {
        ProductListingRawValuesPatch::Set(raw) if raw.trim().is_empty() => {
            Ok(ProductListingRawValuesPatch::Clear)
        }
        ProductListingRawValuesPatch::Set(raw) => match price_format {
            ProductListingRawValuesPriceFormat::DisplayText => {
                match normalize_price(Some(raw.as_str()), fallback_currency) {
                    Ok(Some(price)) => Ok(ProductListingRawValuesPatch::Set(price)),
                    Ok(None) => Ok(ProductListingRawValuesPatch::Clear),
                    Err(PriceNormalizationError::UnknownCurrency) => {
                        report.push(NormalizationIssue::price_unknown_currency(field));
                        Ok(ProductListingRawValuesPatch::Unchanged)
                    }
                    Err(error) => Err(map_price_error(error, field)),
                }
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
    report: &mut NormalizationReport,
) -> Result<
    ProductListingRawValuesPatch<ListingAvailabilityQuickCheck>,
    ProductListingRawValuesNormalizationError,
> {
    match patch {
        ProductListingRawValuesPatch::Set(raw) => {
            let check = quick_check_availability(raw.as_str()).map_err(|error| {
                ProductListingRawValuesNormalizationError::Availability(
                    availability_normalization_error(error),
                )
            })?;
            if let ListingAvailabilityQuickCheck::Unsupported(reason) = check {
                report.push(NormalizationIssue::availability_unsupported(reason));
            }
            Ok(ProductListingRawValuesPatch::Set(check))
        }
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
    ProductListingRawValuesNormalizationError::Price(price_normalization_error(error, field))
}

fn map_image_error(error: ImageUrlNormalizationError) -> ProductListingRawValuesNormalizationError {
    let error = match error {
        ImageUrlNormalizationError::InvalidUrl(source) => {
            NormalizationError::InvalidImageUrl(source)
        }
    };
    ProductListingRawValuesNormalizationError::ImageUrl(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        NormalizationContext, NormalizationStage, RawProductListingPayloadFormat,
        RawProductListingValues, SourcePayload,
    };
    use serde_json::json;

    fn delete_input(raw_values: serde_json::Value) -> ProductListingNormalizationInput {
        ProductListingNormalizationInput::new(
            RawProductListingOperation::Delete,
            RawProductListingPayloadFormat::WoocommerceProduct,
            1,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            SourcePayload::new(json!({})).unwrap(),
            RawProductListingValues::new(raw_values).unwrap(),
            NormalizationContext::new(json!({"baseUrl": false})).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn should_validate_delete_identity_without_normalizing_unrelated_fields() {
        let evaluation = ProductListingRawValuesNormalizer::new().evaluate(&delete_input(json!({
            "sourceListingId": "  source-123  ",
            "price": false,
            "images": "invalid"
        })));

        assert!(matches!(
            evaluation.outcome,
            ProductListingRawValuesNormalizationOutcome::Delete { source_listing_id: Some(id) }
                if id.as_ref() == "source-123"
        ));
        assert!(evaluation.report.into_issues().is_empty());
    }

    #[test]
    fn should_leave_delete_identity_unasserted_when_not_provided() {
        assert!(matches!(
            ProductListingRawValuesNormalizer::new().normalize(&delete_input(json!({}))),
            ProductListingRawValuesNormalizationOutcome::Delete {
                source_listing_id: None
            }
        ));
    }

    #[test]
    fn should_reject_invalid_delete_identity_instead_of_using_a_stream_binding() {
        for identity in [json!(null), json!(42), json!(" "), json!("x".repeat(513))] {
            let evaluation = ProductListingRawValuesNormalizer::new()
                .evaluate(&delete_input(json!({"sourceListingId": identity})));

            assert!(matches!(
                evaluation.outcome,
                ProductListingRawValuesNormalizationOutcome::Invalid(
                    ProductListingRawValuesNormalizationError::InvalidRawValues(_)
                )
            ));
            assert_eq!(1, evaluation.report.into_issues().len());
        }
    }

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
        // Obsolete attributes in retained raw inputs do not participate in shipping interpretation.
        let mut historical_values = values.clone();
        historical_values["attributes"]["rawShipmentMin"] = json!({"action":"SET", "value":["10"]});
        historical_values["attributes"]["rawShipmentMax"] = json!({"action":"SET", "value":["15"]});
        assert!(
            matches!(resolved_shipping(historical_values.clone()), crate::shipping::ShippingQuickCheck::Resolved(a) if a.prohibited)
        );
        historical_values["attributes"]
            .as_object_mut()
            .unwrap()
            .remove("rawShipment");
        assert_eq!(
            crate::shipping::ShippingQuickCheck::NoAssertion,
            resolved_shipping(historical_values)
        );
        let mut contradictory = values.clone();
        contradictory["description"]["value"] =
            json!(["Shipping for this item is available worldwide"]);
        assert_eq!(
            crate::shipping::ShippingQuickCheck::Unsupported(
                crate::shipping::ShippingUnsupportedReason::ContradictoryAssertions
            ),
            resolved_shipping(contradictory)
        );
        let mut unscoped_price = values.clone();
        unscoped_price["attributes"]["rawShipment"] =
            json!({"action":"SET", "value":["Shipping EUR 80"]});
        assert_eq!(
            crate::shipping::ShippingQuickCheck::Unsupported(
                crate::shipping::ShippingUnsupportedReason::UnrecognizedFieldClause
            ),
            resolved_shipping(unscoped_price)
        );
        let mut oversized = values.clone();
        oversized["attributes"]["rawShipment"]["value"] =
            json!(["x".repeat(crate::shipping::MAX_SHIPPING_TEXT_BYTES + 1)]);
        assert!(matches!(
            resolved_shipping(oversized),
            crate::shipping::ShippingQuickCheck::Unsupported(
                crate::shipping::ShippingUnsupportedReason::TextTooLarge { .. }
            )
        ));
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

    fn base_values() -> serde_json::Value {
        json!({
            "sourceListingId": "listing-123", "title": {"action": "SET", "value": "Antique oak chair"},
            "description": {"action": "SET", "value": ["A restored English oak chair."]},
            "priceFormat": "DISPLAY_TEXT",
            "price": {"action": "SET", "value": "EUR 100"},
            "priceEstimateMin": {"action": "UNCHANGED"}, "priceEstimateMax": {"action": "UNCHANGED"},
            "availability": {"action": "SET", "value": "in stock"},
            "url": {"action": "SET", "value": "listing-123"},
            "images": {"action": "SET", "value": []}
        })
    }

    fn reasons(
        evaluation: &ProductListingRawValuesEvaluation,
    ) -> Vec<(NormalizationField, NormalizationIssueReason)> {
        evaluation
            .report
            .issues()
            .iter()
            .map(|issue| (issue.field, issue.reason))
            .collect()
    }

    fn input_without_fallback_currency(
        raw_values: serde_json::Value,
    ) -> ProductListingNormalizationInput {
        ProductListingNormalizationInput::new(
            RawProductListingOperation::Upsert,
            RawProductListingPayloadFormat::CrawlerExtractedProduct,
            1,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            SourcePayload::new(json!({})).unwrap(),
            RawProductListingValues::new(raw_values).unwrap(),
            NormalizationContext::new(json!({"baseUrl": "https://example.test/"})).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn should_preserve_supplied_prices_without_currency_and_report_them() {
        let mut values = base_values();
        values["price"] = json!({"action": "SET", "value": "5500"});
        values["priceEstimateMin"] = json!({"action": "SET", "value": "100"});
        let evaluation = ProductListingRawValuesNormalizer::new()
            .evaluate(&input_without_fallback_currency(values));
        let ProductListingRawValuesNormalizationOutcome::Resolved(resolved) = &evaluation.outcome
        else {
            panic!("missing currency must not reject the observation");
        };
        assert_eq!(ProductListingRawValuesPatch::Unchanged, resolved.price);
        assert_eq!(
            ProductListingRawValuesPatch::Unchanged,
            resolved.price_estimate_min
        );
        assert_eq!(
            vec![
                (
                    NormalizationField::Price,
                    NormalizationIssueReason::PriceUnknownCurrency
                ),
                (
                    NormalizationField::PriceEstimateMin,
                    NormalizationIssueReason::PriceUnknownCurrency
                ),
            ],
            reasons(&evaluation)
        );
        assert!(
            evaluation
                .report
                .issues()
                .iter()
                .all(|issue| issue.handling == NormalizationIssueHandling::PreservedField)
        );
    }

    #[test]
    fn should_report_soft_misses_inside_resolved_outcomes() {
        let mut values = base_values();
        values["availability"] = json!({"action": "SET", "value": "not 2 available"});
        values["attributes"] =
            json!({"rawShipment": {"action": "SET", "value": ["Shipping EUR 80"]}});
        let evaluation = ProductListingRawValuesNormalizer::new().evaluate(&input(values));
        assert!(matches!(
            evaluation.outcome,
            ProductListingRawValuesNormalizationOutcome::Resolved(_)
        ));
        assert_eq!(
            vec![
                (
                    NormalizationField::Shipping,
                    NormalizationIssueReason::ShippingUnrecognizedFieldClause
                ),
                (
                    NormalizationField::Availability,
                    NormalizationIssueReason::AvailabilityQualifiedQuantity
                ),
            ],
            reasons(&evaluation)
        );
    }

    #[test]
    fn should_collect_every_independent_field_issue_and_keep_the_first_decision() {
        let mut values = base_values();
        values["title"] = json!({"action": "SET", "value": "   "});
        values["price"] = json!({"action": "SET", "value": "-100 EUR"});
        values["priceEstimateMax"] = json!({"action": "SET", "value": "EUR 1.2345"});
        values["availability"] =
            json!({"action": "SET", "value": "sold out; 2 available next month"});
        values["attributes"] =
            json!({"rawShipment": {"action": "SET", "value": ["This item is collection only"]}});
        let evaluation = ProductListingRawValuesNormalizer::new().evaluate(&input(values));
        assert!(matches!(
            evaluation.outcome,
            ProductListingRawValuesNormalizationOutcome::Invalid(
                ProductListingRawValuesNormalizationError::Text(NormalizationError::TitleEmpty)
            )
        ));
        assert_eq!(
            vec![
                (
                    NormalizationField::Title,
                    NormalizationIssueReason::TitleEmpty
                ),
                (
                    NormalizationField::Price,
                    NormalizationIssueReason::PriceNegative
                ),
                (
                    NormalizationField::PriceEstimateMax,
                    NormalizationIssueReason::PriceExcessPrecision
                ),
                (
                    NormalizationField::Availability,
                    NormalizationIssueReason::AvailabilityQualifiedQuantity
                ),
            ],
            reasons(&evaluation)
        );
    }

    #[test]
    fn should_report_envelope_failures_once_with_safe_details() {
        let mut values = base_values();
        values["unexpected"] = json!("private text");
        let evaluation = ProductListingRawValuesNormalizer::new().evaluate(&input(values));
        let [issue] = evaluation.report.issues() else {
            panic!("one envelope issue expected");
        };
        assert_eq!(NormalizationStage::Envelope, issue.stage);
        assert_eq!(NormalizationIssueReason::RawValuesInvalid, issue.reason);
        assert!(!serde_json::to_string(issue).unwrap().contains("private"));
    }

    #[test]
    fn should_keep_valid_resolved_inputs_free_of_issues() {
        let evaluation = ProductListingRawValuesNormalizer::new().evaluate(&input(base_values()));
        assert!(evaluation.report.is_empty());
    }
}
