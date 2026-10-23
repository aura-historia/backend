//! Bounded shipping for collection items. Size never grows with the advertised policy:
//! no terms, zones, service lists, per-zone conversions or pointers into omitted terms.
use super::{
    ListingShippingProjection,
    assessment::AssessmentSummaryData,
    detail::AttributionData,
    values::{DisplayData, PriceData, charge_basis, conversion},
};
use product_listing_service::shipping::{
    AdvertisedShipping, PersonalizedShipping, ShippingPricePreview,
};
use schemars::JsonSchema;
use serde::Serialize;

/// Destination-independent summary shared by every viewer of a collection item.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListingShippingSummaryData {
    #[serde(flatten)]
    shipping: AssessmentSummaryData,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "AssessmentSummaryData")]
    collection: Option<AssessmentSummaryData>,
}
impl ListingShippingProjection for ListingShippingSummaryData {
    type Source = AdvertisedShipping;
    fn project(source: AdvertisedShipping) -> Option<Self> {
        let shipping = AssessmentSummaryData::shipping(&source.assessment);
        let collection = AssessmentSummaryData::collection(&source.assessment.collection);
        (!shipping.is_empty() || collection.is_some()).then_some(Self {
            shipping,
            collection,
        })
    }
}

/// Assessment for the caller's selected browsing destination, without echoing it.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonalizedShippingSummaryData {
    #[serde(flatten)]
    shipping: AssessmentSummaryData,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "PricePreviewData")]
    price_preview: Option<PricePreviewData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "AssessmentSummaryData")]
    collection: Option<AssessmentSummaryData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "AttributionData")]
    attribution: Option<AttributionData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "DisplayData")]
    display: Option<DisplayData>,
}
impl ListingShippingProjection for PersonalizedShippingSummaryData {
    type Source = PersonalizedShipping;
    fn project(source: PersonalizedShipping) -> Option<Self> {
        let preview = source.price_preview.map(PricePreviewData::new);
        let converted = preview.as_ref().is_some_and(|(_, converted)| *converted);
        let data = Self {
            shipping: AssessmentSummaryData::shipping(&source.assessment),
            price_preview: preview.map(|(preview, _)| preview),
            collection: AssessmentSummaryData::collection(&source.assessment.collection),
            attribution: source.attribution.map(Into::into),
            display: source
                .display
                .as_ref()
                .filter(|_| converted)
                .map(Into::into),
        };
        (!data.shipping.is_empty() || data.price_preview.is_some() || data.collection.is_some())
            .then_some(data)
    }
}

/// One advertised rate of an applicable service in the requested currency where convertible.
/// It identifies its service and charge basis and never claims a quantity or total.
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "ShippingPricePreviewData")]
struct PricePreviewData {
    service: String,
    #[schemars(schema_with = "charge_basis")]
    basis: &'static str,
    price: PriceData,
}
impl PricePreviewData {
    /// Also reports whether the price is a converted requested-currency value.
    fn new(preview: ShippingPricePreview) -> (Self, bool) {
        let converted = conversion(preview.price, preview.display_price);
        let data = Self {
            service: preview.service.as_str().to_owned(),
            basis: preview.basis.as_str(),
            price: converted.unwrap_or(preview.price).into(),
        };
        (data, converted.is_some())
    }
}
