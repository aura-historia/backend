//! Shared shipping terms and listing delivery JSON protocols. References use adapter codecs.
//! Nullable assertions must be present; reconstruction uses domain constructors.
use application::error::{BoxError, box_error};
use geo::{
    PostalCode, SubdivisionCode,
    data::geography_data::{GeographicDescriptionData, SpatialPositionData},
    reference::ReferenceRelease,
};
use listing_source_core::ListingSourceId;
use money::{Currency, Price};
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use shipping_core::{
    coverage::{
        CoverageCompleteness, DeliveryArea, PostalForm, PostalRule, PostalRuleVersion,
        ReferenceCountry, ShippingCoverage, ShippingZone, UnresolvedArea,
    },
    inputs::{AmountThreshold, Applicability, QuantityRange, SourceShippingClass, SourceVariant},
    pricing::{
        ChargeBasis, ChargeComponent, ComponentInclusion, PriceRange, QualifyingScope,
        ShippingPrice, ShippingRate,
    },
    terms::{
        CollectionDependency, CollectionPolicy, DeliveryActor, DeliverySite, DeliveryTerms,
        ServiceFeature, ShippingEligibility, ShippingPolicy, ShippingService, ShippingServiceCode,
        ShippingServiceTerms, SiteAssertions,
    },
    time::{CollectionArrangement, LeadTimes, TimeBasis, TimeRange, TimeTrigger, TimeUnit},
};
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
// A present JSON null and an omitted field are different protocol inputs.
// Every occurrence uses `deserialize_with`, so Serde rejects missing fields.
#[derive(Serialize)]
#[serde(transparent)]
struct RequiredNullable<T>(Option<T>);

impl<T: JsonSchema> JsonSchema for RequiredNullable<T> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        format!("RequiredNullable_{}", T::schema_name()).into()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        Option::<T>::json_schema(generator)
    }
}

fn required<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<RequiredNullable<T>, D::Error> {
    Option::deserialize(d).map(RequiredNullable)
}
/// Each consuming adapter owns its object-reference representation.
/// The shared protocol never chooses between public TypeIDs and persisted UUID text.
pub trait ShippingTermsReferenceCodec {
    fn decode_party(value: &str) -> Result<PartyId, BoxError>;
    fn encode_party(value: PartyId) -> String;
    fn decode_location(value: &str) -> Result<PartyLocationId, BoxError>;
    fn encode_location(value: PartyLocationId) -> String;
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TermsData {
    #[schemars(schema_with = "schema::version")]
    schema: String,
    #[serde(deserialize_with = "required")]
    shipping: RequiredNullable<PolicyData>,
    #[serde(deserialize_with = "required")]
    collection: RequiredNullable<CollectionData>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum PolicyData {
    Prohibited {},
    Coverage { coverage: Box<CoverageData> },
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CoverageData {
    #[schemars(schema_with = "schema::release")]
    reference_release: String,
    #[schemars(schema_with = "schema::completeness")]
    completeness: String,
    applicability: ApplicabilityData,
    exclusions: Vec<AreaData>,
    zones: Vec<ZoneData>,
    #[serde(deserialize_with = "required")]
    dispatch_origin: RequiredNullable<SiteData>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ZoneData {
    areas: Vec<AreaData>,
    applicability: ApplicabilityData,
    services: Vec<ServiceData>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum AreaData {
    Anywhere {},
    Domestic {},
    Country {
        #[schemars(schema_with = "schema::country")]
        code: String,
        #[schemars(schema_with = "schema::release")]
        reference_release: String,
    },
    Subdivision {
        #[schemars(schema_with = "schema::subdivision")]
        code: String,
        #[schemars(schema_with = "schema::release")]
        reference_release: String,
    },
    Postal {
        #[schemars(schema_with = "schema::postal_version")]
        version: String,
        #[schemars(schema_with = "schema::country")]
        country: String,
        #[schemars(schema_with = "schema::release")]
        reference_release: String,
        form: PostalData,
    },
    Unresolved {
        assertion: String,
    },
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum PostalData {
    Exact { code: String },
    GermanPrefix { prefix: String },
    GermanRange { first: String, last: String },
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ApplicabilityData {
    #[serde(deserialize_with = "required")]
    class: RequiredNullable<String>,
    #[serde(deserialize_with = "required")]
    variants: RequiredNullable<Vec<String>>,
    #[serde(deserialize_with = "required")]
    quantity: RequiredNullable<QuantityData>,
    #[serde(deserialize_with = "required")]
    threshold: RequiredNullable<ThresholdData>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct QuantityData {
    #[schemars(range(max = u64::MAX))]
    minimum: NonZeroU64,
    #[schemars(range(max = u64::MAX))]
    maximum: NonZeroU64,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ThresholdData {
    minimum: MoneyData,
    #[schemars(schema_with = "schema::scope")]
    scope: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MoneyData {
    #[schemars(range(max = u64::MAX))]
    amount_minor: u64,
    #[schemars(schema_with = "schema::money_currency")]
    currency: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ServiceData {
    code: String,
    applicability: ApplicabilityData,
    #[serde(deserialize_with = "required")]
    #[schemars(schema_with = "schema::eligibility")]
    eligibility: RequiredNullable<String>,
    #[serde(deserialize_with = "required")]
    rate: RequiredNullable<RateData>,
    lead_times: LeadTimesData,
    #[schemars(schema_with = "schema::features")]
    features: Vec<String>,
    #[serde(deserialize_with = "required")]
    arranger: RequiredNullable<ActorData>,
    #[serde(deserialize_with = "required")]
    performer: RequiredNullable<ActorData>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum ActorData {
    ListingSource {},
    Buyer {},
    Party { party_id: String },
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RateData {
    price: PriceData,
    #[serde(deserialize_with = "required")]
    #[schemars(schema_with = "schema::charge_basis")]
    basis: RequiredNullable<String>,
    #[serde(deserialize_with = "required")]
    #[schemars(schema_with = "schema::optional_scope")]
    qualifying_scope: RequiredNullable<String>,
    #[schemars(schema_with = "schema::components")]
    components: BTreeMap<String, String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum PriceData {
    Free {
        #[serde(deserialize_with = "required")]
        #[schemars(schema_with = "schema::currency")]
        currency: RequiredNullable<String>,
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
        #[schemars(schema_with = "schema::currency")]
        currency: RequiredNullable<String>,
    },
    OnRequest {
        #[serde(deserialize_with = "required")]
        #[schemars(schema_with = "schema::currency")]
        currency: RequiredNullable<String>,
    },
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LeadTimesData {
    #[serde(deserialize_with = "required")]
    handling: RequiredNullable<TimeData>,
    #[serde(deserialize_with = "required")]
    transit: RequiredNullable<TimeData>,
    #[serde(deserialize_with = "required")]
    total: RequiredNullable<TimeData>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TimeData {
    #[schemars(range(max = u32::MAX))]
    minimum: u32,
    #[schemars(range(max = u32::MAX))]
    maximum: u32,
    #[schemars(schema_with = "schema::time_unit")]
    unit: String,
    #[serde(deserialize_with = "required")]
    #[schemars(schema_with = "schema::time_basis")]
    basis: RequiredNullable<String>,
    #[serde(deserialize_with = "required")]
    #[schemars(schema_with = "schema::trigger")]
    trigger: RequiredNullable<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CollectionData {
    applicability: ApplicabilityData,
    #[schemars(schema_with = "schema::dependency")]
    dependency: String,
    #[serde(deserialize_with = "required")]
    #[schemars(schema_with = "schema::eligibility")]
    eligibility: RequiredNullable<String>,
    #[serde(deserialize_with = "required")]
    site: RequiredNullable<SiteData>,
    #[serde(deserialize_with = "required")]
    arrangement: RequiredNullable<ArrangementData>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum ArrangementData {
    ByAppointment {},
    ContactToArrange {},
    Window { duration: TimeData },
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum SiteData {
    PartyLocation {
        party_location_id: String,
    },
    Asserted {
        #[serde(deserialize_with = "required")]
        #[schemars(schema_with = "schema::geography")]
        geography: RequiredNullable<serde_json::Value>,
        #[serde(deserialize_with = "required")]
        #[schemars(schema_with = "schema::position")]
        position: RequiredNullable<serde_json::Value>,
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
                .0
                .map(|v| SourceShippingClass::new(s, v))
                .transpose()?,
            self.variants
                .0
                .map(|vs| {
                    vs.into_iter()
                        .map(|v| SourceVariant::new(s, v))
                        .collect::<Result<_, _>>()
                })
                .transpose()?,
            self.quantity
                .0
                .map(|v| QuantityRange::new(v.minimum, v.maximum))
                .transpose()?,
            self.threshold
                .0
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
            class: RequiredNullable(v.class().map(|v| v.code().into())),
            variants: RequiredNullable(
                v.variants()
                    .map(|vs| vs.iter().map(|v| v.code().into()).collect()),
            ),
            quantity: RequiredNullable(v.quantity().map(|v| QuantityData {
                minimum: v.minimum(),
                maximum: v.maximum(),
            })),
            threshold: RequiredNullable(v.threshold().map(|v| ThresholdData {
                minimum: MoneyData::encode(v.minimum),
                scope: v.scope.as_str().into(),
            })),
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
                .0
                .map(|v| code(&v, TimeBasis::from_code))
                .transpose()?,
            self.trigger
                .0
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
            basis: RequiredNullable(v.basis().map(|v| v.as_str().into())),
            trigger: RequiredNullable(v.trigger().map(|v| v.as_str().into())),
        }
    }
}
impl ActorData {
    fn decode<C: ShippingTermsReferenceCodec>(self) -> Result<DeliveryActor, BoxError> {
        Ok(match self {
            Self::ListingSource {} => DeliveryActor::ListingSource,
            Self::Buyer {} => DeliveryActor::Buyer,
            Self::Party { party_id } => DeliveryActor::Party(C::decode_party(&party_id)?),
        })
    }
    fn encode<C: ShippingTermsReferenceCodec>(v: DeliveryActor) -> Self {
        match v {
            DeliveryActor::ListingSource => Self::ListingSource {},
            DeliveryActor::Buyer => Self::Buyer {},
            DeliveryActor::Party(id) => Self::Party {
                party_id: C::encode_party(id),
            },
        }
    }
}
impl PriceData {
    fn decode(self) -> Result<ShippingPrice, BoxError> {
        let c = |v: Option<String>| v.map(|v| code(&v, Currency::from_code)).transpose();
        Ok(match self {
            Self::Free { currency } => ShippingPrice::Free(c(currency.0)?),
            Self::Fixed { amount } => ShippingPrice::Fixed(amount.decode()?),
            Self::From { amount } => ShippingPrice::From(amount.decode()?),
            Self::EstimatedRange { minimum, maximum } => ShippingPrice::EstimatedRange(
                PriceRange::new(minimum.decode()?, maximum.decode()?)?,
            ),
            Self::ExternallyCalculated { currency } => {
                ShippingPrice::ExternallyCalculated(c(currency.0)?)
            }
            Self::OnRequest { currency } => ShippingPrice::OnRequest(c(currency.0)?),
        })
    }
    fn encode(v: ShippingPrice) -> Self {
        match v {
            ShippingPrice::Free(c) => Self::Free {
                currency: RequiredNullable(c.map(|v| v.as_str().into())),
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
                currency: RequiredNullable(c.map(|v| v.as_str().into())),
            },
            ShippingPrice::OnRequest(c) => Self::OnRequest {
                currency: RequiredNullable(c.map(|v| v.as_str().into())),
            },
        }
    }
}
impl RateData {
    fn decode(self) -> Result<ShippingRate, BoxError> {
        Ok(ShippingRate::new(
            self.price.decode()?,
            self.basis
                .0
                .map(|v| code(&v, ChargeBasis::from_code))
                .transpose()?,
            self.qualifying_scope
                .0
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
            basis: RequiredNullable(v.basis().map(|v| v.as_str().into())),
            qualifying_scope: RequiredNullable(v.qualifying_scope().map(|v| v.as_str().into())),
            components: v
                .components()
                .iter()
                .map(|(k, v)| (k.as_str().into(), v.as_str().into()))
                .collect(),
        }
    }
}
impl ServiceData {
    fn decode<C: ShippingTermsReferenceCodec>(
        self,
        s: ListingSourceId,
    ) -> Result<ShippingService, BoxError> {
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
                    .0
                    .map(|v| code(&v, ShippingEligibility::from_code))
                    .transpose()?,
                rate: self.rate.0.map(RateData::decode).transpose()?,
                lead_times: LeadTimes {
                    handling: self
                        .lead_times
                        .handling
                        .0
                        .map(TimeData::decode)
                        .transpose()?,
                    transit: self
                        .lead_times
                        .transit
                        .0
                        .map(TimeData::decode)
                        .transpose()?,
                    total: self.lead_times.total.0.map(TimeData::decode).transpose()?,
                },
                features,
                arranger: self.arranger.0.map(ActorData::decode::<C>).transpose()?,
                performer: self.performer.0.map(ActorData::decode::<C>).transpose()?,
            },
        ))
    }
    fn encode<C: ShippingTermsReferenceCodec>(v: &ShippingService) -> Self {
        let t = v.lead_times();
        Self {
            code: v.code().as_str().into(),
            applicability: ApplicabilityData::encode(v.applicability()),
            eligibility: RequiredNullable(v.eligibility().map(|v| v.as_str().into())),
            rate: RequiredNullable(v.rate().map(RateData::encode)),
            lead_times: LeadTimesData {
                handling: RequiredNullable(t.handling.map(TimeData::encode)),
                transit: RequiredNullable(t.transit.map(TimeData::encode)),
                total: RequiredNullable(t.total.map(TimeData::encode)),
            },
            features: v.features().iter().map(|v| v.as_str().into()).collect(),
            arranger: RequiredNullable(v.arranger().map(ActorData::encode::<C>)),
            performer: RequiredNullable(v.performer().map(ActorData::encode::<C>)),
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
    fn decode<C: ShippingTermsReferenceCodec>(self) -> Result<DeliverySite, BoxError> {
        Ok(match self {
            Self::PartyLocation { party_location_id } => {
                DeliverySite::PartyLocation(C::decode_location(&party_location_id)?)
            }
            Self::Asserted {
                geography,
                position,
            } => {
                let geography = geography
                    .0
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
                    .0
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
    fn encode<C: ShippingTermsReferenceCodec>(v: &DeliverySite) -> Result<Self, BoxError> {
        Ok(match v {
            DeliverySite::PartyLocation(id) => Self::PartyLocation {
                party_location_id: C::encode_location(*id),
            },
            DeliverySite::Asserted(v) => Self::Asserted {
                geography: RequiredNullable(
                    v.geography()
                        .map(|v| serde_json::to_value(GeographicDescriptionData::from(v)))
                        .transpose()?,
                ),
                position: RequiredNullable(
                    v.position()
                        .map(|v| serde_json::to_value(SpatialPositionData::from(v)))
                        .transpose()?,
                ),
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
    fn decode<C: ShippingTermsReferenceCodec>(
        self,
        s: ListingSourceId,
    ) -> Result<ShippingPolicy, BoxError> {
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
                                .map(|v| v.decode::<C>(s))
                                .collect::<Result<_, _>>()?,
                        )
                        .map_err(box_error)
                    })
                    .collect::<Result<_, BoxError>>()?,
                c.dispatch_origin.0.map(SiteData::decode::<C>).transpose()?,
            )?),
        })
    }
    fn encode<C: ShippingTermsReferenceCodec>(v: &ShippingPolicy) -> Result<Self, BoxError> {
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
                            services: z.services().iter().map(ServiceData::encode::<C>).collect(),
                        })
                        .collect(),
                    dispatch_origin: RequiredNullable(
                        c.dispatch_origin().map(SiteData::encode::<C>).transpose()?,
                    ),
                }),
            },
        })
    }
}
impl CollectionData {
    fn decode<C: ShippingTermsReferenceCodec>(
        self,
        s: ListingSourceId,
    ) -> Result<CollectionPolicy, BoxError> {
        Ok(CollectionPolicy::new(
            self.applicability.decode(s)?,
            code(&self.dependency, CollectionDependency::from_code)?,
            self.eligibility
                .0
                .map(|v| code(&v, ShippingEligibility::from_code))
                .transpose()?,
            self.site.0.map(SiteData::decode::<C>).transpose()?,
            self.arrangement
                .0
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
    fn encode<C: ShippingTermsReferenceCodec>(v: &CollectionPolicy) -> Result<Self, BoxError> {
        Ok(Self {
            applicability: ApplicabilityData::encode(v.applicability()),
            dependency: v.dependency().as_str().into(),
            eligibility: RequiredNullable(v.eligibility().map(|v| v.as_str().into())),
            site: RequiredNullable(v.site().map(SiteData::encode::<C>).transpose()?),
            arrangement: RequiredNullable(v.arrangement().map(|v| match v {
                CollectionArrangement::ByAppointment => ArrangementData::ByAppointment {},
                CollectionArrangement::ContactToArrange => ArrangementData::ContactToArrange {},
                CollectionArrangement::Window(v) => ArrangementData::Window {
                    duration: TimeData::encode(v),
                },
            })),
        })
    }
}
pub fn decode<C: ShippingTermsReferenceCodec>(
    v: serde_json::Value,
    s: ListingSourceId,
) -> Result<DeliveryTerms, BoxError> {
    let d: TermsData = serde_json::from_value(v)?;
    if d.schema != "SHIP_TERMS_V1" {
        return Err(bad());
    }
    Ok(DeliveryTerms::new(
        d.shipping.0.map(|v| v.decode::<C>(s)).transpose()?,
        d.collection.0.map(|v| v.decode::<C>(s)).transpose()?,
    ))
}
pub fn encode<C: ShippingTermsReferenceCodec>(
    v: &DeliveryTerms,
) -> Result<serde_json::Value, BoxError> {
    serde_json::to_value(TermsData {
        schema: "SHIP_TERMS_V1".into(),
        shipping: RequiredNullable(v.shipping().map(PolicyData::encode::<C>).transpose()?),
        collection: RequiredNullable(
            v.collection()
                .map(CollectionData::encode::<C>)
                .transpose()?,
        ),
    })
    .map_err(box_error)
}

pub mod listing_delivery;

/// Schema of the same terms DTOs used by the codec. Each public or storage adapter supplies
/// its reference representation, just as it does when encoding and decoding the values.
pub fn terms_schema(generator: &mut SchemaGenerator, party: Schema, location: Schema) -> Schema {
    let schema = generator.subschema_for::<TermsData>();
    schema::references(
        generator,
        &[("party_id", party), ("party_location_id", location)],
    );
    schema
}

mod schema;

#[cfg(test)]
mod tests {
    use super::TermsData;

    #[test]
    fn nullable_assertions_require_presence_and_preserve_explicit_null() {
        let explicit =
            serde_json::json!({"schema":"SHIP_TERMS_V1","shipping":null,"collection":null});
        let decoded: TermsData = serde_json::from_value(explicit.clone()).unwrap();
        assert_eq!(explicit, serde_json::to_value(decoded).unwrap());
        for field in ["shipping", "collection"] {
            let mut omitted = explicit.clone();
            omitted.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<TermsData>(omitted).is_err());
        }
    }
}
