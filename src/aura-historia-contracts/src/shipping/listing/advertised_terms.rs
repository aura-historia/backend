//! Public read projection of effective advertised terms for single-listing details.
//! Unlike the strict `SHIP_TERMS_V1` protocol, which stays unchanged for storage, imports and
//! management, it omits absent assertions, empty optional collections and the schema marker.
//! Dispatch origins and collection sites are never disclosed.
use super::values::{
    MoneyData, PriceData, charge_basis, codes, conversion, eligibility, qualifying_scope,
    reference_release,
};
use party_core::party_id::PartyId;
use product_listing_service::shipping::ShippingDisplay;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::Serialize;
use shipping_core::{
    coverage::{CoverageCompleteness, DeliveryArea, PostalForm, ShippingCoverage, ShippingZone},
    inputs::{AmountThreshold, Applicability, QuantityRange},
    pricing::{ChargeComponent, ComponentInclusion, ShippingRate},
    terms::{
        CollectionDependency, CollectionPolicy, DeliveryActor, DeliveryTerms, ServiceFeature,
        ShippingEligibility, ShippingPolicy, ShippingService,
    },
    time::{CollectionArrangement, LeadTimes, TimeBasis, TimeRange, TimeTrigger, TimeUnit},
};
use std::{collections::BTreeMap, num::NonZeroU64};

fn completeness(_: &mut SchemaGenerator) -> Schema {
    use CoverageCompleteness::*;
    codes([Partial, Complete].map(CoverageCompleteness::as_str))
}
fn dependency(_: &mut SchemaGenerator) -> Schema {
    use CollectionDependency::*;
    codes([Independent, Shipping].map(CollectionDependency::as_str))
}
fn time_unit(_: &mut SchemaGenerator) -> Schema {
    use TimeUnit::*;
    codes([Hours, Days, Weeks].map(TimeUnit::as_str))
}
fn time_basis(_: &mut SchemaGenerator) -> Schema {
    use TimeBasis::*;
    codes([Calendar, Business].map(TimeBasis::as_str))
}
fn trigger(_: &mut SchemaGenerator) -> Schema {
    use TimeTrigger::*;
    codes(
        [
            Order,
            OrderConfirmation,
            Payment,
            Dispatch,
            ReadyForCollection,
        ]
        .map(TimeTrigger::as_str),
    )
}
fn features(_: &mut SchemaGenerator) -> Schema {
    use ServiceFeature::*;
    let feature =
        codes([Tracking, Insured, WhiteGlove, Installation, Packing].map(ServiceFeature::as_str));
    schemars::json_schema!({"type": "array", "items": feature, "minItems": 1, "uniqueItems": true})
}
fn components(_: &mut SchemaGenerator) -> Schema {
    use ChargeComponent::*;
    let keys = codes(
        [
            Transport,
            Packing,
            Insurance,
            Handling,
            Installation,
            ImportDuty,
            Tax,
        ]
        .map(ChargeComponent::as_str),
    );
    let inclusion = codes(
        [ComponentInclusion::Included, ComponentInclusion::Excluded]
            .map(ComponentInclusion::as_str),
    );
    schemars::json_schema!({"type": "object", "propertyNames": keys, "additionalProperties": inclusion, "minProperties": 1})
}
fn postal_version(_: &mut SchemaGenerator) -> Schema {
    codes([shipping_core::coverage::PostalRuleVersion::V1.as_str()])
}
fn party_id(_: &mut SchemaGenerator) -> Schema {
    crate::wire::schema::object_id(PartyId::PREFIX)
}

#[derive(Debug, Serialize, JsonSchema)]
#[schemars(extend("minProperties" = 1))]
#[schemars(rename = "ListingShippingTermsData")]
pub struct TermsData {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "PolicyData")]
    shipping: Option<PolicyData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "CollectionData")]
    collection: Option<CollectionData>,
}
#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
#[schemars(rename = "ListingShippingPolicyData")]
enum PolicyData {
    Prohibited,
    Coverage { coverage: Box<CoverageData> },
}
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "ListingShippingCoverageData")]
struct CoverageData {
    #[schemars(schema_with = "reference_release")]
    reference_release: &'static str,
    #[schemars(schema_with = "completeness")]
    completeness: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ApplicabilityData")]
    applicability: Option<ApplicabilityData>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[schemars(length(min = 1, max = 64))]
    exclusions: Vec<AreaData>,
    /// Ordered; the first zone matching a destination applies.
    #[schemars(length(max = 64))]
    zones: Vec<ZoneData>,
}
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "ListingShippingZoneData")]
struct ZoneData {
    #[schemars(length(min = 1, max = 64))]
    areas: Vec<AreaData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ApplicabilityData")]
    applicability: Option<ApplicabilityData>,
    #[schemars(length(min = 1, max = 64))]
    services: Vec<ServiceData>,
}
#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
#[schemars(rename = "ShippingAreaData")]
enum AreaData {
    Anywhere,
    Domestic,
    Country {
        code: &'static str,
        #[schemars(schema_with = "reference_release")]
        reference_release: &'static str,
    },
    Subdivision {
        code: &'static str,
        #[schemars(schema_with = "reference_release")]
        reference_release: &'static str,
    },
    Postal {
        #[schemars(schema_with = "postal_version")]
        version: &'static str,
        country: &'static str,
        #[schemars(schema_with = "reference_release")]
        reference_release: &'static str,
        form: PostalFormData,
    },
    Unresolved {
        assertion: String,
    },
}
#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
#[schemars(rename = "ShippingPostalFormData")]
enum PostalFormData {
    Exact { code: String },
    GermanPrefix { prefix: String },
    GermanRange { first: String, last: String },
}
/// Present only when at least one condition is asserted; every asserted condition must hold.
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(extend("minProperties" = 1))]
#[schemars(rename = "ListingShippingApplicabilityData")]
struct ApplicabilityData {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>", length(min = 1, max = 64))]
    variants: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "QuantityData")]
    quantity: Option<QuantityData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ThresholdData")]
    threshold: Option<ThresholdData>,
}
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "ShippingQuantityData")]
struct QuantityData {
    #[schemars(range(max = u64::MAX))]
    minimum: NonZeroU64,
    #[schemars(range(max = u64::MAX))]
    maximum: NonZeroU64,
}
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "ShippingThresholdData")]
struct ThresholdData {
    minimum: MoneyData,
    #[schemars(schema_with = "qualifying_scope")]
    scope: &'static str,
}
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "ListingShippingServiceData")]
struct ServiceData {
    code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ApplicabilityData")]
    applicability: Option<ApplicabilityData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "eligibility")]
    eligibility: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "RateData")]
    rate: Option<RateData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "LeadTimesData")]
    lead_times: Option<LeadTimesData>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[schemars(schema_with = "features")]
    features: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ActorData")]
    arranger: Option<ActorData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ActorData")]
    performer: Option<ActorData>,
}
/// An advertised rate. `display_price` is the requested-currency value of an amount
/// advertised in another currency; `price` always keeps the advertised source value.
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "ListingShippingRateData")]
pub struct RateData {
    price: PriceData,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "PriceData")]
    display_price: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "charge_basis")]
    basis: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "qualifying_scope")]
    qualifying_scope: Option<&'static str>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(schema_with = "components")]
    components: BTreeMap<&'static str, &'static str>,
}
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(extend("minProperties" = 1))]
#[schemars(rename = "ListingShippingLeadTimesData")]
struct LeadTimesData {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "TimeData")]
    handling: Option<TimeData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "TimeData")]
    transit: Option<TimeData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "TimeData")]
    total: Option<TimeData>,
}
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "ListingShippingTimeData")]
struct TimeData {
    #[schemars(range(max = u32::MAX))]
    minimum: u32,
    #[schemars(range(max = u32::MAX))]
    maximum: u32,
    #[schemars(schema_with = "time_unit")]
    unit: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "time_basis")]
    basis: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "trigger")]
    trigger: Option<&'static str>,
}
#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
#[schemars(rename = "ShippingActorData")]
enum ActorData {
    ListingSource,
    Buyer,
    Party {
        #[schemars(schema_with = "party_id")]
        party_id: String,
    },
}
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "ListingShippingCollectionData")]
struct CollectionData {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ApplicabilityData")]
    applicability: Option<ApplicabilityData>,
    #[schemars(schema_with = "dependency")]
    dependency: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "eligibility")]
    eligibility: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ArrangementData")]
    arrangement: Option<ArrangementData>,
}
#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
#[schemars(rename = "ListingShippingCollectionArrangementData")]
enum ArrangementData {
    ByAppointment,
    ContactToArrange,
    Window { duration: TimeData },
}

impl TermsData {
    pub(super) fn is_empty(&self) -> bool {
        self.shipping.is_none() && self.collection.is_none()
    }
    /// Also reports whether any rate carries a converted requested-currency value.
    pub(super) fn new(terms: &DeliveryTerms, display: Option<&ShippingDisplay>) -> (Self, bool) {
        let mut converted = false;
        let shipping = terms.shipping().map(|policy| match policy {
            ShippingPolicy::Prohibited => PolicyData::Prohibited,
            ShippingPolicy::Coverage(coverage) => PolicyData::Coverage {
                coverage: Box::new(CoverageData::new(coverage, display, &mut converted)),
            },
        });
        let data = Self {
            shipping,
            collection: terms.collection().map(CollectionData::new),
        };
        (data, converted)
    }
}
impl CoverageData {
    fn new(
        coverage: &ShippingCoverage,
        display: Option<&ShippingDisplay>,
        converted: &mut bool,
    ) -> Self {
        Self {
            reference_release: coverage.release().as_str(),
            completeness: coverage.completeness().as_str(),
            applicability: ApplicabilityData::new(coverage.applicability()),
            exclusions: coverage.exclusions().iter().map(AreaData::new).collect(),
            zones: coverage
                .zones()
                .iter()
                .enumerate()
                .map(|(index, zone)| ZoneData::new(index, zone, display, converted))
                .collect(),
        }
    }
}
impl ZoneData {
    fn new(
        zone_index: usize,
        zone: &ShippingZone,
        display: Option<&ShippingDisplay>,
        converted: &mut bool,
    ) -> Self {
        Self {
            areas: zone.areas().iter().map(AreaData::new).collect(),
            applicability: ApplicabilityData::new(zone.applicability()),
            services: zone
                .services()
                .iter()
                .enumerate()
                .map(|(service_index, service)| {
                    let display =
                        display.and_then(|display| display.price(zone_index, service_index));
                    let data = ServiceData::new(service, display);
                    *converted |= data.rate.as_ref().is_some_and(RateData::is_converted);
                    data
                })
                .collect(),
        }
    }
}
impl AreaData {
    fn new(area: &DeliveryArea) -> Self {
        match area {
            DeliveryArea::Anywhere => Self::Anywhere,
            DeliveryArea::Domestic => Self::Domestic,
            DeliveryArea::Country(country) => Self::Country {
                code: country.country().alpha2(),
                reference_release: country.release().as_str(),
            },
            DeliveryArea::Subdivision(subdivision) => Self::Subdivision {
                code: subdivision.as_str(),
                reference_release: subdivision.reference_release().as_str(),
            },
            DeliveryArea::Postal(rule) => Self::Postal {
                version: rule.version().as_str(),
                country: rule.country().country().alpha2(),
                reference_release: rule.country().release().as_str(),
                form: match rule.form() {
                    PostalForm::Exact(code) => PostalFormData::Exact {
                        code: code.as_str().to_owned(),
                    },
                    PostalForm::GermanPrefix(prefix) => PostalFormData::GermanPrefix {
                        prefix: prefix.clone(),
                    },
                    PostalForm::GermanRange { first, last } => PostalFormData::GermanRange {
                        first: first.clone(),
                        last: last.clone(),
                    },
                },
            },
            DeliveryArea::Unresolved(assertion) => Self::Unresolved {
                assertion: assertion.as_str().to_owned(),
            },
        }
    }
}
impl ApplicabilityData {
    /// `None` for an unconditional rule.
    fn new(applicability: &Applicability) -> Option<Self> {
        let data = Self {
            class: applicability.class().map(|class| class.code().to_owned()),
            variants: applicability.variants().map(|variants| {
                variants
                    .iter()
                    .map(|variant| variant.code().to_owned())
                    .collect()
            }),
            quantity: applicability.quantity().map(QuantityData::new),
            threshold: applicability.threshold().map(ThresholdData::new),
        };
        (data.class.is_some()
            || data.variants.is_some()
            || data.quantity.is_some()
            || data.threshold.is_some())
        .then_some(data)
    }
}
impl QuantityData {
    fn new(range: QuantityRange) -> Self {
        Self {
            minimum: range.minimum(),
            maximum: range.maximum(),
        }
    }
}
impl ThresholdData {
    fn new(threshold: AmountThreshold) -> Self {
        Self {
            minimum: threshold.minimum.into(),
            scope: threshold.scope.as_str(),
        }
    }
}
impl ServiceData {
    fn new(
        service: &ShippingService,
        display: Option<shipping_core::pricing::ShippingPrice>,
    ) -> Self {
        Self {
            code: service.code().as_str().to_owned(),
            applicability: ApplicabilityData::new(service.applicability()),
            eligibility: service.eligibility().map(ShippingEligibility::as_str),
            rate: service.rate().map(|rate| RateData::new(rate, display)),
            lead_times: LeadTimesData::new(service.lead_times()),
            features: service
                .features()
                .iter()
                .map(|feature| feature.as_str())
                .collect(),
            arranger: service.arranger().map(ActorData::new),
            performer: service.performer().map(ActorData::new),
        }
    }
}
impl RateData {
    pub(super) fn new(
        rate: &ShippingRate,
        display: Option<shipping_core::pricing::ShippingPrice>,
    ) -> Self {
        Self {
            price: rate.price().into(),
            display_price: conversion(rate.price(), display).map(Into::into),
            basis: rate.basis().map(|basis| basis.as_str()),
            qualifying_scope: rate.qualifying_scope().map(|scope| scope.as_str()),
            components: rate
                .components()
                .iter()
                .map(|(component, inclusion)| (component.as_str(), inclusion.as_str()))
                .collect(),
        }
    }
    pub(super) fn is_converted(&self) -> bool {
        self.display_price.is_some()
    }
}
impl LeadTimesData {
    fn new(lead_times: LeadTimes) -> Option<Self> {
        let LeadTimes {
            handling,
            transit,
            total,
        } = lead_times;
        (handling.is_some() || transit.is_some() || total.is_some()).then(|| Self {
            handling: handling.map(TimeData::new),
            transit: transit.map(TimeData::new),
            total: total.map(TimeData::new),
        })
    }
}
impl TimeData {
    fn new(range: TimeRange) -> Self {
        Self {
            minimum: range.minimum(),
            maximum: range.maximum(),
            unit: range.unit().as_str(),
            basis: range.basis().map(TimeBasis::as_str),
            trigger: range.trigger().map(TimeTrigger::as_str),
        }
    }
}
impl ActorData {
    fn new(actor: DeliveryActor) -> Self {
        match actor {
            DeliveryActor::ListingSource => Self::ListingSource,
            DeliveryActor::Buyer => Self::Buyer,
            DeliveryActor::Party(party) => Self::Party {
                party_id: party.to_string(),
            },
        }
    }
}
impl CollectionData {
    /// Sites are omitted: a referenced location does not authorize publishing its address.
    fn new(policy: &CollectionPolicy) -> Self {
        Self {
            applicability: ApplicabilityData::new(policy.applicability()),
            dependency: policy.dependency().as_str(),
            eligibility: policy.eligibility().map(ShippingEligibility::as_str),
            arrangement: policy.arrangement().map(|arrangement| match arrangement {
                CollectionArrangement::ByAppointment => ArrangementData::ByAppointment,
                CollectionArrangement::ContactToArrange => ArrangementData::ContactToArrange,
                CollectionArrangement::Window(duration) => ArrangementData::Window {
                    duration: TimeData::new(duration),
                },
            }),
        }
    }
}
