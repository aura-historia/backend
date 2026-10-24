//! Clean shipping for single-listing details: material advertised terms and, for the
//! caller's selected destination, self-contained options. No management bookkeeping.
use super::{
    ListingShippingProjection,
    advertised_terms::{RateData, TermsData},
    assessment::{AssessmentSummaryData, applicable},
    values::{ChargeData, DisplayData, reference_release},
};
use geo::GeographicDescription;
use product_listing_service::shipping::{
    AdvertisedShipping, DestinationAttribution, PersonalizedShipping, ShippingDisplay,
};
use schemars::JsonSchema;
use serde::Serialize;
use shipping_core::assessment::{ServiceAssessment, ShippingAssessment};

/// Effective public terms shared by every viewer of the listing; omitted when unasserted.
/// Terms that cannot be resolved leave only the explanation of the undetermined outcome.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(extend("minProperties" = 1))]
pub struct ListingShippingDetailData {
    #[serde(flatten)]
    shipping: AssessmentSummaryData,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "TermsData")]
    terms: Option<TermsData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "DisplayData")]
    display: Option<DisplayData>,
}
impl ListingShippingProjection for ListingShippingDetailData {
    type Source = AdvertisedShipping;
    fn project(source: AdvertisedShipping) -> Option<Self> {
        let terms = TermsData::new(source.effective.terms(), source.display.as_ref());
        let converted = terms.as_ref().is_some_and(TermsData::is_converted);
        let shipping = AssessmentSummaryData::shipping(&source.assessment);
        (terms.is_some() || !shipping.is_empty()).then(|| Self {
            shipping,
            terms,
            display: source
                .display
                .as_ref()
                .filter(|_| converted)
                .map(Into::into),
        })
    }
}

/// Assessment for the caller's selected browsing destination.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonalizedShippingDetailData {
    destination: DestinationData,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "AttributionData")]
    attribution: Option<AttributionData>,
    #[serde(flatten)]
    shipping: AssessmentSummaryData,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[schemars(length(min = 1, max = 64))]
    options: Vec<OptionData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "AssessmentSummaryData")]
    collection: Option<AssessmentSummaryData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "DisplayData")]
    display: Option<DisplayData>,
}
impl ListingShippingProjection for PersonalizedShippingDetailData {
    type Source = PersonalizedShipping;
    fn project(source: PersonalizedShipping) -> Option<Self> {
        let options = OptionData::all(&source.assessment, source.display.as_ref());
        let converted = options.iter().any(OptionData::is_converted);
        Some(Self {
            destination: DestinationData::new(&source.destination),
            attribution: source.attribution.map(Into::into),
            shipping: AssessmentSummaryData::shipping(&source.assessment),
            options,
            collection: AssessmentSummaryData::collection(&source.assessment.collection),
            display: source
                .display
                .as_ref()
                .filter(|_| converted)
                .map(Into::into),
        })
    }
}

/// Minimal area facts: no street, coordinates, address identity or provider evidence.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "DeliveryAssessmentGeography")]
struct DestinationData {
    #[schemars(schema_with = "reference_release")]
    reference_release: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String", regex(pattern = "^[A-Z]{2}$"))]
    country: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    subdivision: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    postal_code: Option<String>,
}
impl DestinationData {
    fn new(destination: &GeographicDescription) -> Self {
        Self {
            reference_release: destination.reference_release().as_str(),
            country: destination.country().map(|country| country.alpha2()),
            subdivision: destination
                .subdivision()
                .map(|subdivision| subdivision.as_str()),
            postal_code: destination
                .postal_code()
                .map(|postal| postal.as_str().to_owned()),
        }
    }
}

/// Attribution required by the geographic evidence behind the assessed destination.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "DeliveryAssessmentAttribution")]
pub(super) struct AttributionData {
    provider: String,
    attribution_url: String,
}
impl From<DestinationAttribution> for AttributionData {
    fn from(attribution: DestinationAttribution) -> Self {
        Self {
            provider: attribution.provider,
            attribution_url: attribution.attribution_url,
        }
    }
}

/// An assessed service of the destination's zone, identified by its code and advertised rate.
/// A charge appears only when browsing supplied every count its basis needs.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "DeliveryAssessmentOption")]
struct OptionData {
    service: String,
    #[serde(flatten)]
    assessment: AssessmentSummaryData,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "RateData")]
    rate: Option<RateData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ChargeData")]
    charge: Option<ChargeData>,
}
impl OptionData {
    fn all(assessment: &ShippingAssessment, display: Option<&ShippingDisplay>) -> Vec<Self> {
        let Some(zone) = assessment.selected_zone else {
            return Vec::new();
        };
        assessment
            .services
            .iter()
            .enumerate()
            .filter(|(_, service)| applicable(service))
            .map(|(index, service)| {
                Self::new(
                    service,
                    display.and_then(|display| display.price(zone, index)),
                )
            })
            .collect()
    }
    fn new(
        service: &ServiceAssessment,
        display: Option<shipping_core::pricing::ShippingPrice>,
    ) -> Self {
        Self {
            service: service.service.code().as_str().to_owned(),
            assessment: AssessmentSummaryData::option(service),
            rate: service
                .service
                .rate()
                .map(|rate| RateData::new(rate, display)),
            charge: service.charge.map(Into::into),
        }
    }
    fn is_converted(&self) -> bool {
        self.rate.as_ref().is_some_and(RateData::is_converted)
    }
}
