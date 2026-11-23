use crate::product_listing_image_document::ProductListingImageDocument;
use auction_core::AuctionId;
use domain_primitives::event_id::EventId;
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::Language;
use money::Currency;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_price::{PriceRange, ProductListingPrice};
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_core::source_listing_id::SourceListingId;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use time::OffsetDateTime;
use url::Url;

fn serialize_code<T, S>(
    value: &T,
    serializer: S,
    code: fn(T) -> &'static str,
) -> Result<S::Ok, S::Error>
where
    T: Copy,
    S: serde::Serializer,
{
    serializer.serialize_str(code(*value))
}

fn deserialize_code<'de, T, D>(deserializer: D, parse: fn(&str) -> Option<T>) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    parse(&value).ok_or_else(|| serde::de::Error::custom(format!("unsupported code `{value}`")))
}

pub(crate) mod currency {
    use super::*;

    pub(crate) fn serialize<S>(value: &Currency, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_code(value, serializer, Currency::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Currency, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_code(deserializer, Currency::from_code)
    }
}

pub(crate) mod language {
    use super::*;
    use strum::IntoEnumIterator;

    fn code(value: Language) -> &'static str {
        match value {
            Language::De => "DE",
            Language::En => "EN",
            Language::Fr => "FR",
            Language::Es => "ES",
            Language::It => "IT",
            Language::Zh => "ZH",
            Language::Pt => "PT",
            Language::Pl => "PL",
            Language::Tr => "TR",
            Language::Nl => "NL",
            Language::Cs => "CS",
            Language::Ja => "JA",
            Language::Ru => "RU",
            Language::Ar => "AR",
            Language::Ko => "KO",
            Language::El => "EL",
            Language::He => "HE",
            Language::Th => "TH",
            Language::Hi => "HI",
        }
    }

    fn parse(value: &str) -> Option<Language> {
        Language::iter().find(|language| code(*language) == value)
    }

    pub(crate) fn serialize<S>(value: &Language, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_code(value, serializer, code)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Language, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_code(deserializer, parse)
    }
}

pub(crate) mod source_listing_id {
    use super::*;

    pub(crate) fn serialize<S>(value: &SourceListingId, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(value.as_ref())
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<SourceListingId, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        SourceListingId::try_from(String::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

pub(crate) mod listing_availability {
    use super::*;

    pub(crate) mod option {
        use super::*;

        pub(crate) fn serialize<S>(
            value: &Option<ListingAvailability>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            match value {
                Some(value) => serializer.serialize_some(value.as_str()),
                None => serializer.serialize_none(),
            }
        }

        pub(crate) fn deserialize<'de, D>(
            deserializer: D,
        ) -> Result<Option<ListingAvailability>, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            Option::<String>::deserialize(deserializer)?
                .map(|value| {
                    ListingAvailability::from_code(&value).ok_or_else(|| {
                        serde::de::Error::custom(format!("unsupported code `{value}`"))
                    })
                })
                .transpose()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TextDocument {
    pub(crate) text: String,
    #[serde(with = "language")]
    pub(crate) language: Language,
}

impl TextDocument {
    pub(crate) fn new(text: impl Into<String>, language: Language) -> Self {
        Self {
            text: text.into(),
            language,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub(crate) enum SourcePriceDocument {
    Monetary {
        amount: u64,
        #[serde(with = "currency")]
        currency: Currency,
    },
    /// `amount` is the lower bound, so price filters read both kinds through one field.
    Range {
        amount: u64,
        #[serde(rename = "maxAmount")]
        max_amount: u64,
        #[serde(with = "currency")]
        currency: Currency,
    },
    OnRequest,
}

impl From<ProductListingPrice> for SourcePriceDocument {
    fn from(value: ProductListingPrice) -> Self {
        match value {
            ProductListingPrice::Monetary(price) => Self::Monetary {
                amount: u64::from(price.monetary_amount),
                currency: price.currency,
            },
            ProductListingPrice::Range(range) => Self::Range {
                amount: u64::from(range.min().monetary_amount),
                max_amount: u64::from(range.max().monetary_amount),
                currency: range.currency(),
            },
            ProductListingPrice::OnRequest => Self::OnRequest,
        }
    }
}

impl TryFrom<SourcePriceDocument> for ProductListingPrice {
    type Error = ProductListingDocumentValidationError;

    fn try_from(value: SourcePriceDocument) -> Result<Self, Self::Error> {
        Ok(match value {
            SourcePriceDocument::Monetary { amount, currency } => {
                Self::Monetary(money::Price::new(amount.into(), currency))
            }
            SourcePriceDocument::Range {
                amount,
                max_amount,
                currency,
            } => Self::Range(
                PriceRange::from_amounts(currency, amount, max_amount)
                    .map_err(|_| ProductListingDocumentValidationError::InvalidSourcePriceRange)?,
            ),
            SourcePriceDocument::OnRequest => Self::OnRequest,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ProductListingDocumentValidationError {
    #[error("product geographic projection is invalid")]
    InvalidGeography,
    #[error("source price range bounds are invalid")]
    InvalidSourcePriceRange,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GeographicDimensionDocument {
    pub location_id: party_core::party_location_id::PartyLocationId,
    pub basis: String,
    pub reference_release: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdivision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub point: Option<GeographicPointDocument>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precision: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "GeographicPointData")]
pub(crate) struct GeographicPointDocument {
    pub lat: f64,
    pub lon: f64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GeographicPointData {
    lat: f64,
    lon: f64,
}
impl TryFrom<GeographicPointData> for GeographicPointDocument {
    type Error = geo::InvalidPosition;
    fn try_from(p: GeographicPointData) -> Result<Self, Self::Error> {
        let p = geo::GeoPoint::new(p.lat, p.lon)?;
        Ok(Self {
            lat: p.latitude_degrees(),
            lon: p.longitude_degrees(),
        })
    }
}
impl From<&product_listing_service::geography::GeographicDimension>
    for GeographicDimensionDocument
{
    fn from(d: &product_listing_service::geography::GeographicDimension) -> Self {
        Self {
            location_id: d.location_id,
            basis: product_listing_service::geography::GeographicDimension::BASIS.into(),
            reference_release: d.reference_release.as_str().into(),
            country: d.country.map(|c| c.alpha2().into()),
            subdivision: d.subdivision.as_ref().map(|s| s.as_str().into()),
            point: d.position.map(|p| GeographicPointDocument {
                lat: p.point().latitude_degrees(),
                lon: p.point().longitude_degrees(),
            }),
            precision: d.position.map(|p| p.precision().as_str().into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductListingDocument {
    pub(crate) product_listing_id: ProductListingId,
    pub(crate) product_listing_title_slug_id: ProductListingSlugId,
    pub(crate) listing_source_id: ListingSourceId,
    #[serde(with = "source_listing_id")]
    pub(crate) source_listing_id: SourceListingId,
    pub(crate) event_id: EventId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) shipping: Option<crate::shipping_document::ShippingDocument>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) business_geography: Option<GeographicDimensionDocument>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) inventory_geography: Option<GeographicDimensionDocument>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) auction_id: Option<AuctionId>,
    pub(crate) title: TextDocument,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) title_de: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) title_en: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) title_fr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) title_es: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) title_it: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) source_price: Option<SourcePriceDocument>,
    #[serde(
        with = "listing_availability::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub(crate) availability: Option<ListingAvailability>,
    pub(crate) url: Url,
    #[serde(skip_serializing_if = "IndexSet::is_empty", default)]
    pub(crate) images: IndexSet<ProductListingImageDocument>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) embedding: Option<Vec<f32>>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub(crate) lot_bidding_opens_at: Option<OffsetDateTime>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub(crate) lot_scheduled_closes_at: Option<OffsetDateTime>,

    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) updated: OffsetDateTime,
}

impl ProductListingDocument {
    pub(crate) fn _id(&self) -> ProductListingId {
        self.product_listing_id
    }

    pub(crate) fn validate(&self) -> Result<(), ProductListingDocumentValidationError> {
        for d in [&self.business_geography, &self.inventory_geography]
            .into_iter()
            .flatten()
        {
            let check = || {
                if d.basis != product_listing_service::geography::GeographicDimension::BASIS {
                    return None;
                }
                let release =
                    geo::reference::ReferenceRelease::from_code(&d.reference_release).ok()?;
                let country = d
                    .country
                    .as_deref()
                    .map(|c| geo::country_from_code(c, release))
                    .transpose()
                    .ok()?;
                let subdivision = d
                    .subdivision
                    .as_deref()
                    .map(|c| geo::SubdivisionCode::from_code(c, release))
                    .transpose()
                    .ok()?;
                geo::GeographicDescription::new_in_release(
                    release,
                    None,
                    country,
                    subdivision,
                    None,
                )
                .ok()?;
                if d.point.is_some() != d.precision.is_some()
                    || (country.is_none() && d.subdivision.is_none() && d.point.is_none())
                    || d.precision
                        .as_deref()
                        .is_some_and(|p| geo::SpatialPrecision::from_code(p).is_none())
                {
                    return None;
                }
                Some(())
            };
            check().ok_or(ProductListingDocumentValidationError::InvalidGeography)?;
        }

        if let Some(price) = self.source_price {
            ProductListingPrice::try_from(price)?;
        }
        Ok(())
    }

    pub(crate) fn source_price(
        &self,
    ) -> Result<Option<ProductListingPrice>, ProductListingDocumentValidationError> {
        self.source_price.map(TryInto::try_into).transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use rstest::rstest;
    use time::macros::datetime;

    fn document() -> Result<ProductListingDocument, url::ParseError> {
        Ok(ProductListingDocument {
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::raw("vase-abcdef")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("sku-1")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            event_id: EventId::new(),
            shipping: None,
            business_geography: None,
            inventory_geography: None,
            auction_id: None,
            title: TextDocument::new("Vase", Language::En),
            title_de: None,
            title_en: Some("Vase".to_owned()),
            title_fr: None,
            title_es: None,
            title_it: None,
            source_price: Some(SourcePriceDocument::Monetary {
                amount: 100,
                currency: Currency::Eur,
            }),
            availability: Some(ListingAvailability::Available),
            url: Url::parse("https://shop.example/product_listings/sku-1")?,
            images: IndexSet::new(),
            embedding: None,
            lot_bidding_opens_at: None,
            lot_scheduled_closes_at: None,
            created: datetime!(2025-01-01 0:00 UTC),
            updated: datetime!(2025-01-02 0:00 UTC),
        })
    }

    #[test]
    fn should_preserve_historical_uppercase_language_codec()
    -> Result<(), Box<dyn std::error::Error>> {
        let value = serde_json::to_value(TextDocument::new("Vase", Language::En))?;

        assert_eq!(
            serde_json::json!({
                "text": "Vase",
                "language": "EN",
            }),
            value
        );

        let restored = serde_json::from_value::<TextDocument>(value)?;
        assert_eq!(TextDocument::new("Vase", Language::En), restored);

        Ok(())
    }

    #[test]
    fn should_serialize_source_price_and_availability_for_active_product()
    -> Result<(), Box<dyn std::error::Error>> {
        let value = serde_json::to_value(document()?)?;

        assert_eq!(
            Some(&serde_json::json!("MONETARY")),
            value.pointer("/sourcePrice/type")
        );
        assert_eq!(
            Some(&serde_json::json!(100)),
            value.pointer("/sourcePrice/amount")
        );
        assert_eq!(
            Some(&serde_json::json!("EUR")),
            value.pointer("/sourcePrice/currency")
        );
        assert_eq!(
            Some(&serde_json::json!("AVAILABLE")),
            value.get("availability")
        );
        assert!(value.get("productListingTitleSlugId").is_some());
        assert!(value.get("productListingSlugId").is_none());
        assert!(value.get("sourceListingSlugId").is_none());
        assert!(value.get("salePrices").is_none());
        assert!(value.get("priceEur").is_none());
        assert!(value.get("priceUsd").is_none());
        assert!(value.get("priceEstimateMinEur").is_none());
        assert!(value.get("priceEstimateMaxUsd").is_none());
        assert!(value.as_object().is_some_and(|fields| {
            fields.keys().all(|field| {
                !field.starts_with("priceEstimate") && field != "priceEur" && field != "priceUsd"
            })
        }));
        Ok(())
    }

    #[test]
    fn should_omit_absent_availability() -> Result<(), Box<dyn std::error::Error>> {
        let mut document = document()?;
        document.availability = None;

        assert!(
            serde_json::to_value(document)?
                .get("availability")
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn should_validate_compact_geography_without_accepting_empty_or_synthetic_dimensions()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = document()?;
        let dimension = GeographicDimensionDocument {
            location_id: party_core::party_location_id::PartyLocationId::new(),
            basis: product_listing_service::geography::GeographicDimension::BASIS.into(),
            reference_release: geo::reference::ReferenceRelease::CURRENT.as_str().into(),
            country: Some("DE".into()),
            subdivision: Some("DE-BY".into()),
            point: None,
            precision: None,
        };
        document.business_geography = Some(dimension.clone());
        document.validate()?;
        let mut empty = dimension.clone();
        empty.country = None;
        empty.subdivision = None;
        let mut incompatible = dimension.clone();
        incompatible.country = Some("FR".into());
        let mut partial_point = dimension;
        partial_point.point = Some(GeographicPointDocument {
            lat: 48.1,
            lon: 11.5,
        });
        for malformed in [empty, incompatible, partial_point] {
            document.business_geography = Some(malformed);
            assert!(matches!(
                document.validate(),
                Err(ProductListingDocumentValidationError::InvalidGeography)
            ));
        }
        Ok(())
    }

    #[test]
    fn should_roundtrip_on_request_source_price() -> Result<(), Box<dyn std::error::Error>> {
        let mut value = serde_json::to_value(document()?)?;
        value["sourcePrice"] = serde_json::json!({ "type": "ON_REQUEST" });

        let restored = serde_json::from_value::<ProductListingDocument>(value)?;

        assert_eq!(
            Some(ProductListingPrice::OnRequest),
            restored.source_price()?
        );
        Ok(())
    }

    #[test]
    fn should_roundtrip_a_source_price_range_with_its_lower_bound_as_amount()
    -> Result<(), Box<dyn std::error::Error>> {
        let range = PriceRange::from_amounts(Currency::Eur, 100_000_u64, 150_000_u64)?;
        let mut document = document()?;
        document.source_price = Some(ProductListingPrice::Range(range).into());

        let value = serde_json::to_value(&document)?;
        assert_eq!(
            serde_json::json!({"type": "RANGE", "amount": 100_000, "maxAmount": 150_000, "currency": "EUR"}),
            value["sourcePrice"]
        );
        let restored = serde_json::from_value::<ProductListingDocument>(value)?;
        assert_eq!(
            Some(ProductListingPrice::Range(range)),
            restored.source_price()?
        );
        Ok(())
    }

    #[rstest]
    #[case::descending(150_000, 100_000)]
    #[case::equal(100_000, 100_000)]
    fn should_reject_a_source_price_range_that_does_not_ascend(
        #[case] amount: u64,
        #[case] max_amount: u64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut document = document()?;
        document.source_price = Some(SourcePriceDocument::Range {
            amount,
            max_amount,
            currency: Currency::Eur,
        });

        assert_eq!(
            Err(ProductListingDocumentValidationError::InvalidSourcePriceRange),
            document.validate()
        );
        Ok(())
    }

    #[test]
    fn should_round_trip_canonical_typed_ids_without_retired_source_fields()
    -> Result<(), Box<dyn std::error::Error>> {
        let document = document()?;
        let value = serde_json::to_value(&document)?;

        for (field, expected) in [
            ("productListingId", document.product_listing_id.to_string()),
            ("listingSourceId", document.listing_source_id.to_string()),
            ("eventId", document.event_id.to_string()),
            ("sourceListingId", document.source_listing_id.to_string()),
        ] {
            assert_eq!(Some(&serde_json::json!(expected)), value.get(field));
        }
        assert_eq!(document.product_listing_id, document._id());
        for field in [
            "listingSourceName",
            "listingSourceSlugId",
            "shopSlugId",
            "sellerSlugId",
            "shopId",
            "sellerId",
            "shopListingId",
            "shopName",
            "sellerName",
            "shopType",
            "viewUrl",
        ] {
            assert!(
                value.get(field).is_none(),
                "retired field {field} is present"
            );
        }

        let round_tripped = serde_json::from_value::<ProductListingDocument>(value)?;
        assert_eq!(document, round_tripped);
        Ok(())
    }

    #[test]
    fn should_reject_wrong_prefixes_for_typed_document_ids()
    -> Result<(), Box<dyn std::error::Error>> {
        let value = serde_json::to_value(document()?)?;

        for (field, wrong_id) in [
            ("productListingId", EventId::new().to_string()),
            ("listingSourceId", ProductListingId::new().to_string()),
            ("eventId", ProductListingId::new().to_string()),
        ] {
            let mut malformed = value.clone();
            malformed[field] = serde_json::json!(wrong_id);
            let Err(error) = serde_json::from_value::<ProductListingDocument>(malformed) else {
                return Err("wrong-prefix document ID was accepted".into());
            };
            assert!(error.to_string().contains("wrong object ID prefix"));
        }
        Ok(())
    }

    #[test]
    fn should_reject_bare_uuids_for_typed_document_ids() -> Result<(), Box<dyn std::error::Error>> {
        let document = document()?;
        let bare_ids = [
            (
                "productListingId",
                document.product_listing_id.as_uuid().to_string(),
            ),
            (
                "listingSourceId",
                document.listing_source_id.as_uuid().to_string(),
            ),
            ("eventId", document.event_id.as_uuid().to_string()),
        ];
        let value = serde_json::to_value(document)?;

        for (field, bare_id) in bare_ids {
            let mut malformed = value.clone();
            malformed[field] = serde_json::json!(bare_id);
            let Err(error) = serde_json::from_value::<ProductListingDocument>(malformed) else {
                return Err("bare UUID document ID was accepted".into());
            };
            assert!(error.to_string().contains("malformed object ID"));
        }
        Ok(())
    }

    const RANGE: SourcePriceDocument = SourcePriceDocument::Range {
        amount: 100,
        max_amount: 200,
        currency: Currency::Eur,
    };

    /// Documents an earlier projection rule wrote for sold listings stay readable until the
    /// backfill replaces them; their sale valuation is ignored.
    #[test]
    fn should_decode_legacy_sold_document_without_its_sale_valuation()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut document = document()?;
        document.source_price = Some(RANGE);
        document.availability = Some(ListingAvailability::SoldOut);
        let mut legacy = serde_json::to_value(&document)?;
        legacy["salePrices"] = serde_json::json!({ "eur": 100, "usd": 110 });
        legacy["salePricesMax"] = serde_json::json!({ "eur": 200, "usd": 220 });
        legacy["saleObservationFxRateId"] = serde_json::json!(fxrate_core::FxRateId::new());
        legacy["saleObservedAt"] = serde_json::json!("1970-01-01T00:00:00Z");

        let decoded = serde_json::from_value::<ProductListingDocument>(legacy)?;

        decoded.validate()?;
        assert_eq!(document, decoded);
        Ok(())
    }
}
