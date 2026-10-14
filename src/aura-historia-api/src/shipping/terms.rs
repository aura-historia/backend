//! Transport codec. Nullable assertions must be explicit; reconstruction uses domain constructors.
use application::error::{BoxError, box_error};
use geo::{
    PostalCode, SubdivisionCode,
    data::geography_data::{GeographicDescriptionData, SpatialPositionData},
    reference::ReferenceRelease,
};
use listing_source_core::ListingSourceId;
use money::{Currency, Price};
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use serde::{Deserialize, Serialize};
use shipping_core::{coverage::*, inputs::*, pricing::*, terms::*, time::*};
use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU64,
};

fn bad() -> BoxError {
    box_error(std::io::Error::other(
        "invalid advertised shipping terms encoding",
    ))
}
fn code<T>(v: &str, f: impl FnOnce(&str) -> Option<T>) -> Result<T, BoxError> {
    f(v).ok_or_else(bad)
}
fn required<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::deserialize(d)
}
fn party(v: &str) -> Result<PartyId, BoxError> {
    v.parse().map_err(box_error)
}
fn location(v: &str) -> Result<PartyLocationId, BoxError> {
    v.parse().map_err(box_error)
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TermsData {
    schema: String,
    #[serde(deserialize_with = "required")]
    shipping: Option<PolicyData>,
    #[serde(deserialize_with = "required")]
    collection: Option<CollectionData>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum PolicyData {
    Prohibited {},
    Coverage { coverage: Box<CoverageData> },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoverageData {
    reference_release: String,
    completeness: String,
    applicability: ApplicabilityData,
    exclusions: Vec<AreaData>,
    zones: Vec<ZoneData>,
    #[serde(deserialize_with = "required")]
    dispatch_origin: Option<SiteData>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ZoneData {
    areas: Vec<AreaData>,
    applicability: ApplicabilityData,
    services: Vec<ServiceData>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum AreaData {
    Anywhere {},
    Domestic {},
    Country {
        code: String,
        reference_release: String,
    },
    Subdivision {
        code: String,
        reference_release: String,
    },
    Postal {
        version: String,
        country: String,
        reference_release: String,
        form: PostalData,
    },
    Unresolved {
        assertion: String,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum PostalData {
    Exact { code: String },
    GermanPrefix { prefix: String },
    GermanRange { first: String, last: String },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplicabilityData {
    #[serde(deserialize_with = "required")]
    class: Option<String>,
    #[serde(deserialize_with = "required")]
    variants: Option<Vec<String>>,
    #[serde(deserialize_with = "required")]
    quantity: Option<QuantityData>,
    #[serde(deserialize_with = "required")]
    threshold: Option<ThresholdData>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuantityData {
    minimum: NonZeroU64,
    maximum: NonZeroU64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThresholdData {
    minimum: MoneyData,
    scope: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MoneyData {
    amount_minor: u64,
    currency: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceData {
    code: String,
    applicability: ApplicabilityData,
    #[serde(deserialize_with = "required")]
    eligibility: Option<String>,
    #[serde(deserialize_with = "required")]
    rate: Option<RateData>,
    lead_times: LeadTimesData,
    features: Vec<String>,
    #[serde(deserialize_with = "required")]
    arranger: Option<ActorData>,
    #[serde(deserialize_with = "required")]
    performer: Option<ActorData>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum ActorData {
    ListingSource {},
    Buyer {},
    Party { party_id: String },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RateData {
    price: PriceData,
    #[serde(deserialize_with = "required")]
    basis: Option<String>,
    #[serde(deserialize_with = "required")]
    qualifying_scope: Option<String>,
    components: BTreeMap<String, String>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum PriceData {
    Free {
        #[serde(deserialize_with = "required")]
        currency: Option<String>,
    },
    Fixed {
        amount: MoneyData,
    },
    From {
        amount: MoneyData,
    },
    EstimatedRange {
        minimum: MoneyData,
        maximum: MoneyData,
    },
    ExternallyCalculated {
        #[serde(deserialize_with = "required")]
        currency: Option<String>,
    },
    OnRequest {
        #[serde(deserialize_with = "required")]
        currency: Option<String>,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LeadTimesData {
    #[serde(deserialize_with = "required")]
    handling: Option<TimeData>,
    #[serde(deserialize_with = "required")]
    transit: Option<TimeData>,
    #[serde(deserialize_with = "required")]
    total: Option<TimeData>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TimeData {
    minimum: u32,
    maximum: u32,
    unit: String,
    #[serde(deserialize_with = "required")]
    basis: Option<String>,
    #[serde(deserialize_with = "required")]
    trigger: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CollectionData {
    applicability: ApplicabilityData,
    dependency: String,
    #[serde(deserialize_with = "required")]
    eligibility: Option<String>,
    #[serde(deserialize_with = "required")]
    site: Option<SiteData>,
    #[serde(deserialize_with = "required")]
    arrangement: Option<ArrangementData>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum ArrangementData {
    ByAppointment {},
    ContactToArrange {},
    Window { duration: TimeData },
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum SiteData {
    PartyLocation {
        party_location_id: String,
    },
    Asserted {
        #[serde(deserialize_with = "required")]
        geography: Option<serde_json::Value>,
        #[serde(deserialize_with = "required")]
        position: Option<serde_json::Value>,
    },
}
impl MoneyData {
    fn decode(self) -> Result<Price, BoxError> {
        Ok(Price::new(
            self.amount_minor.into(),
            code(&self.currency, Currency::from_code)?,
        ))
    }
    fn encode(v: Price) -> Self {
        Self {
            amount_minor: v.monetary_amount.into(),
            currency: v.currency.as_str().into(),
        }
    }
}
impl ApplicabilityData {
    fn decode(self, s: ListingSourceId) -> Result<Applicability, BoxError> {
        Applicability::new(
            self.class
                .map(|v| SourceShippingClass::new(s, v))
                .transpose()?,
            self.variants
                .map(|vs| {
                    vs.into_iter()
                        .map(|v| SourceVariant::new(s, v))
                        .collect::<Result<_, _>>()
                })
                .transpose()?,
            self.quantity
                .map(|v| QuantityRange::new(v.minimum, v.maximum))
                .transpose()?,
            self.threshold
                .map(|v| {
                    Ok::<_, BoxError>(AmountThreshold {
                        minimum: v.minimum.decode()?,
                        scope: code(&v.scope, QualifyingScope::from_code)?,
                    })
                })
                .transpose()?,
        )
        .map_err(box_error)
    }
    fn encode(v: &Applicability) -> Self {
        Self {
            class: v.class().map(|v| v.code().into()),
            variants: v
                .variants()
                .map(|vs| vs.iter().map(|v| v.code().into()).collect()),
            quantity: v.quantity().map(|v| QuantityData {
                minimum: v.minimum(),
                maximum: v.maximum(),
            }),
            threshold: v.threshold().map(|v| ThresholdData {
                minimum: MoneyData::encode(v.minimum),
                scope: v.scope.as_str().into(),
            }),
        }
    }
}
impl TimeData {
    fn decode(self) -> Result<TimeRange, BoxError> {
        TimeRange::new(
            self.minimum,
            self.maximum,
            code(&self.unit, TimeUnit::from_code)?,
            self.basis
                .map(|v| code(&v, TimeBasis::from_code))
                .transpose()?,
            self.trigger
                .map(|v| code(&v, TimeTrigger::from_code))
                .transpose()?,
        )
        .map_err(box_error)
    }
    fn encode(v: TimeRange) -> Self {
        Self {
            minimum: v.minimum(),
            maximum: v.maximum(),
            unit: v.unit().as_str().into(),
            basis: v.basis().map(|v| v.as_str().into()),
            trigger: v.trigger().map(|v| v.as_str().into()),
        }
    }
}
impl ActorData {
    fn decode(self) -> Result<DeliveryActor, BoxError> {
        Ok(match self {
            Self::ListingSource {} => DeliveryActor::ListingSource,
            Self::Buyer {} => DeliveryActor::Buyer,
            Self::Party { party_id } => DeliveryActor::Party(party(&party_id)?),
        })
    }
    fn encode(v: DeliveryActor) -> Self {
        match v {
            DeliveryActor::ListingSource => Self::ListingSource {},
            DeliveryActor::Buyer => Self::Buyer {},
            DeliveryActor::Party(id) => Self::Party {
                party_id: id.to_string(),
            },
        }
    }
}
impl PriceData {
    fn decode(self) -> Result<ShippingPrice, BoxError> {
        let c = |v: Option<String>| v.map(|v| code(&v, Currency::from_code)).transpose();
        Ok(match self {
            Self::Free { currency } => ShippingPrice::Free(c(currency)?),
            Self::Fixed { amount } => ShippingPrice::Fixed(amount.decode()?),
            Self::From { amount } => ShippingPrice::From(amount.decode()?),
            Self::EstimatedRange { minimum, maximum } => ShippingPrice::EstimatedRange(
                PriceRange::new(minimum.decode()?, maximum.decode()?)?,
            ),
            Self::ExternallyCalculated { currency } => {
                ShippingPrice::ExternallyCalculated(c(currency)?)
            }
            Self::OnRequest { currency } => ShippingPrice::OnRequest(c(currency)?),
        })
    }
    fn encode(v: ShippingPrice) -> Self {
        match v {
            ShippingPrice::Free(c) => Self::Free {
                currency: c.map(|v| v.as_str().into()),
            },
            ShippingPrice::Fixed(v) => Self::Fixed {
                amount: MoneyData::encode(v),
            },
            ShippingPrice::From(v) => Self::From {
                amount: MoneyData::encode(v),
            },
            ShippingPrice::EstimatedRange(v) => Self::EstimatedRange {
                minimum: MoneyData::encode(v.minimum()),
                maximum: MoneyData::encode(v.maximum()),
            },
            ShippingPrice::ExternallyCalculated(c) => Self::ExternallyCalculated {
                currency: c.map(|v| v.as_str().into()),
            },
            ShippingPrice::OnRequest(c) => Self::OnRequest {
                currency: c.map(|v| v.as_str().into()),
            },
        }
    }
}
impl RateData {
    fn decode(self) -> Result<ShippingRate, BoxError> {
        Ok(ShippingRate::new(
            self.price.decode()?,
            self.basis
                .map(|v| code(&v, ChargeBasis::from_code))
                .transpose()?,
            self.qualifying_scope
                .map(|v| code(&v, QualifyingScope::from_code))
                .transpose()?,
            self.components
                .into_iter()
                .map(|(k, v)| {
                    Ok((
                        code(&k, ChargeComponent::from_code)?,
                        code(&v, ComponentInclusion::from_code)?,
                    ))
                })
                .collect::<Result<_, BoxError>>()?,
        ))
    }
    fn encode(v: &ShippingRate) -> Self {
        Self {
            price: PriceData::encode(v.price()),
            basis: v.basis().map(|v| v.as_str().into()),
            qualifying_scope: v.qualifying_scope().map(|v| v.as_str().into()),
            components: v
                .components()
                .iter()
                .map(|(k, v)| (k.as_str().into(), v.as_str().into()))
                .collect(),
        }
    }
}
impl ServiceData {
    fn decode(self, s: ListingSourceId) -> Result<ShippingService, BoxError> {
        let features: BTreeSet<_> = self
            .features
            .iter()
            .map(|v| code(v, ServiceFeature::from_code))
            .collect::<Result<_, _>>()?;
        if features.len() != self.features.len() {
            return Err(bad());
        }
        Ok(ShippingService::new(
            ShippingServiceCode::new(self.code)?,
            ShippingServiceTerms {
                applicability: self.applicability.decode(s)?,
                eligibility: self
                    .eligibility
                    .map(|v| code(&v, ShippingEligibility::from_code))
                    .transpose()?,
                rate: self.rate.map(RateData::decode).transpose()?,
                lead_times: LeadTimes {
                    handling: self.lead_times.handling.map(TimeData::decode).transpose()?,
                    transit: self.lead_times.transit.map(TimeData::decode).transpose()?,
                    total: self.lead_times.total.map(TimeData::decode).transpose()?,
                },
                features,
                arranger: self.arranger.map(ActorData::decode).transpose()?,
                performer: self.performer.map(ActorData::decode).transpose()?,
            },
        ))
    }
    fn encode(v: &ShippingService) -> Self {
        let t = v.lead_times();
        Self {
            code: v.code().as_str().into(),
            applicability: ApplicabilityData::encode(v.applicability()),
            eligibility: v.eligibility().map(|v| v.as_str().into()),
            rate: v.rate().map(RateData::encode),
            lead_times: LeadTimesData {
                handling: t.handling.map(TimeData::encode),
                transit: t.transit.map(TimeData::encode),
                total: t.total.map(TimeData::encode),
            },
            features: v.features().iter().map(|v| v.as_str().into()).collect(),
            arranger: v.arranger().map(ActorData::encode),
            performer: v.performer().map(ActorData::encode),
        }
    }
}
fn required_keys(v: &serde_json::Value, keys: &[&str]) -> Result<(), BoxError> {
    if keys.iter().any(|k| v.get(k).is_none()) {
        Err(bad())
    } else {
        Ok(())
    }
}
impl SiteData {
    fn decode(self) -> Result<DeliverySite, BoxError> {
        Ok(match self {
            Self::PartyLocation { party_location_id } => {
                DeliverySite::PartyLocation(location(&party_location_id)?)
            }
            Self::Asserted {
                geography,
                position,
            } => {
                let geography = geography
                    .map(|v| {
                        required_keys(
                            &v,
                            &[
                                "reference_release",
                                "address_text",
                                "country",
                                "subdivision",
                                "postal_code",
                            ],
                        )?;
                        let d: GeographicDescriptionData = serde_json::from_value(v)?;
                        Option::<geo::GeographicDescription>::try_from(d)?.ok_or_else(bad)
                    })
                    .transpose()?;
                let position = position
                    .map(|v| {
                        required_keys(&v, &["point", "precision", "accuracy_metres"])?;
                        let d: SpatialPositionData = serde_json::from_value(v)?;
                        geo::SpatialPosition::try_from(d).map_err(box_error)
                    })
                    .transpose()?;
                DeliverySite::Asserted(SiteAssertions::new(geography, position)?)
            }
        })
    }
    fn encode(v: &DeliverySite) -> Result<Self, BoxError> {
        Ok(match v {
            DeliverySite::PartyLocation(id) => Self::PartyLocation {
                party_location_id: id.to_string(),
            },
            DeliverySite::Asserted(v) => Self::Asserted {
                geography: v
                    .geography()
                    .map(|v| serde_json::to_value(GeographicDescriptionData::from(v)))
                    .transpose()?,
                position: v
                    .position()
                    .map(|v| serde_json::to_value(SpatialPositionData::from(v)))
                    .transpose()?,
            },
        })
    }
}
impl AreaData {
    fn decode(self) -> Result<DeliveryArea, BoxError> {
        Ok(match self {
            Self::Anywhere {} => DeliveryArea::Anywhere,
            Self::Domestic {} => DeliveryArea::Domestic,
            Self::Country {
                code,
                reference_release,
            } => DeliveryArea::Country(ReferenceCountry::new(
                &code,
                ReferenceRelease::from_code(&reference_release)?,
            )?),
            Self::Subdivision {
                code,
                reference_release,
            } => DeliveryArea::Subdivision(SubdivisionCode::from_code(
                &code,
                ReferenceRelease::from_code(&reference_release)?,
            )?),
            Self::Unresolved { assertion } => {
                DeliveryArea::Unresolved(UnresolvedArea::new(assertion)?)
            }
            Self::Postal {
                version,
                country,
                reference_release,
                form,
            } => DeliveryArea::Postal(PostalRule::new(
                code(&version, PostalRuleVersion::from_code)?,
                ReferenceCountry::new(&country, ReferenceRelease::from_code(&reference_release)?)?,
                match form {
                    PostalData::Exact { code } => PostalForm::Exact(PostalCode::new(code)?),
                    PostalData::GermanPrefix { prefix } => PostalForm::GermanPrefix(prefix),
                    PostalData::GermanRange { first, last } => {
                        PostalForm::GermanRange { first, last }
                    }
                },
            )?),
        })
    }
    fn encode(v: &DeliveryArea) -> Self {
        match v {
            DeliveryArea::Anywhere => Self::Anywhere {},
            DeliveryArea::Domestic => Self::Domestic {},
            DeliveryArea::Country(v) => Self::Country {
                code: v.country().alpha2().into(),
                reference_release: v.release().as_str().into(),
            },
            DeliveryArea::Subdivision(v) => Self::Subdivision {
                code: v.as_str().into(),
                reference_release: v.reference_release().as_str().into(),
            },
            DeliveryArea::Unresolved(v) => Self::Unresolved {
                assertion: v.as_str().into(),
            },
            DeliveryArea::Postal(v) => Self::Postal {
                version: v.version().as_str().into(),
                country: v.country().country().alpha2().into(),
                reference_release: v.country().release().as_str().into(),
                form: match v.form() {
                    PostalForm::Exact(v) => PostalData::Exact {
                        code: v.as_str().into(),
                    },
                    PostalForm::GermanPrefix(v) => PostalData::GermanPrefix { prefix: v.clone() },
                    PostalForm::GermanRange { first, last } => PostalData::GermanRange {
                        first: first.clone(),
                        last: last.clone(),
                    },
                },
            },
        }
    }
}
impl PolicyData {
    fn decode(self, s: ListingSourceId) -> Result<ShippingPolicy, BoxError> {
        Ok(match self {
            Self::Prohibited {} => ShippingPolicy::Prohibited,
            Self::Coverage { coverage: c } => ShippingPolicy::coverage(ShippingCoverage::new(
                ReferenceRelease::from_code(&c.reference_release)?,
                code(&c.completeness, CoverageCompleteness::from_code)?,
                c.applicability.decode(s)?,
                c.exclusions
                    .into_iter()
                    .map(AreaData::decode)
                    .collect::<Result<_, _>>()?,
                c.zones
                    .into_iter()
                    .map(|z| {
                        ShippingZone::new(
                            z.areas
                                .into_iter()
                                .map(AreaData::decode)
                                .collect::<Result<_, _>>()?,
                            z.applicability.decode(s)?,
                            z.services
                                .into_iter()
                                .map(|v| v.decode(s))
                                .collect::<Result<_, _>>()?,
                        )
                        .map_err(box_error)
                    })
                    .collect::<Result<_, BoxError>>()?,
                c.dispatch_origin.map(SiteData::decode).transpose()?,
            )?),
        })
    }
    fn encode(v: &ShippingPolicy) -> Result<Self, BoxError> {
        Ok(match v {
            ShippingPolicy::Prohibited => Self::Prohibited {},
            ShippingPolicy::Coverage(c) => Self::Coverage {
                coverage: Box::new(CoverageData {
                    reference_release: c.release().as_str().into(),
                    completeness: c.completeness().as_str().into(),
                    applicability: ApplicabilityData::encode(c.applicability()),
                    exclusions: c.exclusions().iter().map(AreaData::encode).collect(),
                    zones: c
                        .zones()
                        .iter()
                        .map(|z| ZoneData {
                            areas: z.areas().iter().map(AreaData::encode).collect(),
                            applicability: ApplicabilityData::encode(z.applicability()),
                            services: z.services().iter().map(ServiceData::encode).collect(),
                        })
                        .collect(),
                    dispatch_origin: c.dispatch_origin().map(SiteData::encode).transpose()?,
                }),
            },
        })
    }
}
impl CollectionData {
    fn decode(self, s: ListingSourceId) -> Result<CollectionPolicy, BoxError> {
        Ok(CollectionPolicy::new(
            self.applicability.decode(s)?,
            code(&self.dependency, CollectionDependency::from_code)?,
            self.eligibility
                .map(|v| code(&v, ShippingEligibility::from_code))
                .transpose()?,
            self.site.map(SiteData::decode).transpose()?,
            self.arrangement
                .map(|v| {
                    Ok::<_, BoxError>(match v {
                        ArrangementData::ByAppointment {} => CollectionArrangement::ByAppointment,
                        ArrangementData::ContactToArrange {} => {
                            CollectionArrangement::ContactToArrange
                        }
                        ArrangementData::Window { duration } => {
                            CollectionArrangement::Window(duration.decode()?)
                        }
                    })
                })
                .transpose()?,
        ))
    }
    fn encode(v: &CollectionPolicy) -> Result<Self, BoxError> {
        Ok(Self {
            applicability: ApplicabilityData::encode(v.applicability()),
            dependency: v.dependency().as_str().into(),
            eligibility: v.eligibility().map(|v| v.as_str().into()),
            site: v.site().map(SiteData::encode).transpose()?,
            arrangement: v.arrangement().map(|v| match v {
                CollectionArrangement::ByAppointment => ArrangementData::ByAppointment {},
                CollectionArrangement::ContactToArrange => ArrangementData::ContactToArrange {},
                CollectionArrangement::Window(v) => ArrangementData::Window {
                    duration: TimeData::encode(v),
                },
            }),
        })
    }
}
pub(super) fn decode(v: serde_json::Value, s: ListingSourceId) -> Result<DeliveryTerms, BoxError> {
    let d: TermsData = serde_json::from_value(v)?;
    if d.schema != "SHIP_TERMS_V1" {
        return Err(bad());
    }
    Ok(DeliveryTerms::new(
        d.shipping.map(|v| v.decode(s)).transpose()?,
        d.collection.map(|v| v.decode(s)).transpose()?,
    ))
}
pub(super) fn encode(v: &DeliveryTerms) -> Result<serde_json::Value, BoxError> {
    serde_json::to_value(TermsData {
        schema: "SHIP_TERMS_V1".into(),
        shipping: v.shipping().map(PolicyData::encode).transpose()?,
        collection: v.collection().map(CollectionData::encode).transpose()?,
    })
    .map_err(box_error)
}
