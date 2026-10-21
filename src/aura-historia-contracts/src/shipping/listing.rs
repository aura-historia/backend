use application::error::{BoxError, box_error};
use geo::GeographicDescription;
use money::Price;
use product_listing_service::shipping::{AdvertisedShipping, PersonalizedShipping};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Serialize, Serializer};
use serde_json::{Value, json};
use shipping_core::{
    assessment::{AssessedCharge, AssessmentReason},
    terms::{CollectionPolicy, DeliveryTerms, ShippingPolicy},
};
use shipping_service::assessment::AssertionEvidence;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug)]
pub struct AdvertisedShippingData(pub AdvertisedShipping);
impl Serialize for AdvertisedShippingData {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        advertised_data(&self.0)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}
#[derive(Debug)]
pub struct PersonalizedShippingData(pub PersonalizedShipping);
impl Serialize for PersonalizedShippingData {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        personalized_data(&self.0)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}
fn timestamp(t: OffsetDateTime) -> Result<String, BoxError> {
    t.format(&Rfc3339).map_err(box_error)
}
fn geography(g: Option<&GeographicDescription>) -> Value {
    g.map_or(Value::Null, |g| json!({"referenceRelease":g.reference_release().as_str(),"country":g.country().map(|c|c.alpha2()),"subdivision":g.subdivision().map(|s|s.as_str()),"postalCode":g.postal_code().map(|p|p.as_str())}))
}
fn evidence_data(e: AssertionEvidence) -> Result<Value, BoxError> {
    Ok(json!({"verifiedAt":timestamp(e.verified_at)?}))
}
fn money(p: Price) -> super::schema::MoneySchema {
    super::schema::MoneySchema {
        amount_minor: p.monetary_amount.into(),
        currency: p.currency.as_str().into(),
    }
}
fn charge(c: Option<AssessedCharge>) -> Option<super::schema::ChargeSchema> {
    use super::schema::ChargeSchema;
    c.map(|c| match c {
        AssessedCharge::Fixed(p) => ChargeSchema::Fixed { amount: money(p) },
        AssessedCharge::From(p) => ChargeSchema::From { amount: money(p) },
        AssessedCharge::EstimatedRange(r) => ChargeSchema::EstimatedRange {
            minimum: money(r.minimum()),
            maximum: money(r.maximum()),
        },
    })
}
fn display_price(
    price: shipping_core::pricing::ShippingPrice,
) -> Option<super::schema::PriceSchema> {
    use super::schema::PriceSchema;
    use shipping_core::pricing::ShippingPrice;
    Some(match price {
        ShippingPrice::Free(currency) => PriceSchema::Free {
            currency: currency.map(|c| c.as_str().into()),
        },
        ShippingPrice::Fixed(p) => PriceSchema::Fixed { amount: money(p) },
        ShippingPrice::From(p) => PriceSchema::From { amount: money(p) },
        ShippingPrice::EstimatedRange(r) => PriceSchema::EstimatedRange {
            minimum: money(r.minimum()),
            maximum: money(r.maximum()),
        },
        _ => return None,
    })
}
fn reason(r: &AssessmentReason) -> Value {
    use AssessmentReason::*;
    let code = match r {
        MissingInput(i) => {
            return match i {
                shipping_core::assessment::MissingInput::QualifyingAmount(s) => {
                    json!({"code":"MISSING_INPUT","input":i.as_str(),"scope":s.as_str()})
                }
                _ => json!({"code":"MISSING_INPUT","input":i.as_str()}),
            };
        }
        ProfileAbsent(_) => "PROFILE_ABSENT",
        ProfileRetired(_) => "PROFILE_RETIRED",
        LocationAbsent(_) => "LOCATION_ABSENT",
        LocationRetired(_) => "LOCATION_RETIRED",
        EarlierZoneUndetermined { index } => {
            return json!({"code":"EARLIER_ZONE_UNDETERMINED","index":index});
        }
        NoSourceDefault => "NO_SOURCE_DEFAULT",
        SourceMismatch => "SOURCE_MISMATCH",
        AmbiguousDependency => "AMBIGUOUS_DEPENDENCY",
        ReferenceReleaseMismatch => "REFERENCE_RELEASE_MISMATCH",
        UnresolvedArea => "UNRESOLVED_AREA",
        UnsupportedPostalShape => "UNSUPPORTED_POSTAL_SHAPE",
        CountryMismatch => "COUNTRY_MISMATCH",
        SubdivisionMismatch => "SUBDIVISION_MISMATCH",
        PostalMismatch => "POSTAL_MISMATCH",
        ClassMismatch => "CLASS_MISMATCH",
        IncompleteClassMembership => "INCOMPLETE_CLASS_MEMBERSHIP",
        VariantMismatch => "VARIANT_MISMATCH",
        VariantUnavailable => "VARIANT_UNAVAILABLE",
        MixedVariantApplicability => "MIXED_VARIANT_APPLICABILITY",
        QuantityOutsideRange => "QUANTITY_OUTSIDE_RANGE",
        QualifyingThresholdNotMet => "QUALIFYING_THRESHOLD_NOT_MET",
        CurrencyMismatch => "CURRENCY_MISMATCH",
        Excluded => "EXCLUDED",
        IncompleteCoverage => "INCOMPLETE_COVERAGE",
        OutsideCompleteCoverage => "OUTSIDE_COMPLETE_COVERAGE",
        NoApplicableService => "NO_APPLICABLE_SERVICE",
        NoShippingAssertion => "NO_SHIPPING_ASSERTION",
        NoCollectionAssertion => "NO_COLLECTION_ASSERTION",
        NoEligibilityAssertion => "NO_ELIGIBILITY_ASSERTION",
        SuppressedInheritance => "SUPPRESSED_INHERITANCE",
        AssertionCleared => "ASSERTION_CLEARED",
        ShippingProhibited => "SHIPPING_PROHIBITED",
        ShippingDependencyReplaced => "SHIPPING_DEPENDENCY_REPLACED",
        ShippingDependencyUndetermined => "SHIPPING_DEPENDENCY_UNDETERMINED",
        UnknownChargeBasis => "UNKNOWN_CHARGE_BASIS",
        NoCurrencyAssertion => "NO_CURRENCY_ASSERTION",
        PriceOnRequest => "PRICE_ON_REQUEST",
        ExternallyCalculatedPrice => "EXTERNALLY_CALCULATED_PRICE",
        ArithmeticOverflow => "ARITHMETIC_OVERFLOW",
    };
    json!({"code":code})
}
fn reasons(v: &[AssessmentReason]) -> Vec<Value> {
    v.iter().map(reason).collect()
}
/// Exclude site disclosure from public terms. Referencing
/// a managed location does not grant disclosure, including for anonymous requests.
fn public_terms(terms: &DeliveryTerms) -> Result<Value, BoxError> {
    let collection = terms.collection().map(|c| {
        CollectionPolicy::new(
            c.applicability().clone(),
            c.dependency(),
            c.eligibility(),
            None,
            c.arrangement(),
        )
    });
    let mut value =
        super::terms::encode(&DeliveryTerms::new(terms.shipping().cloned(), collection))?;
    if matches!(terms.shipping(), Some(ShippingPolicy::Coverage(_))) {
        value["shipping"]["coverage"]["dispatch_origin"] = Value::Null;
    }
    Ok(value)
}

fn advertised_data(item: &AdvertisedShipping) -> Result<Value, BoxError> {
    let profile = item
        .profile
        .as_ref()
        .map(|p| {
            Ok::<_, BoxError>(json!({
                "shippingProfileId":p.id,"termsRevision":p.terms_revision,
                "managementRevision":p.management_revision,"lifecycle":p.lifecycle.as_str(),
                "evidence":evidence_data(p.evidence)?,
            }))
        })
        .transpose()?;
    Ok(json!({
        "terms":public_terms(item.effective.terms())?,
        "display":item.display.as_ref().map(|d| Ok::<_, BoxError>(json!({
            "currency":d.currency.as_str(),"fxRateId":d.fx_rate_id,"capturedAt":timestamp(d.captured_at)?,
            "rates":d.rates.iter().map(|r| json!({"zoneIndex":r.zone_index,"serviceIndex":r.service_index,"price":display_price(r.price),"basis":r.basis.map(|b| b.as_str())})).collect::<Vec<_>>()
        }))).transpose()?,
        "listingRevision":item.listing_revision,"sourceRevision":item.source_revision,
        "defaultRevision":item.default_revision,"profile":profile,
        "listingEvidence":item.listing_evidence.map(evidence_data).transpose()?,
        "evaluatedAt":timestamp(item.evaluated_at)?,
        "shippingReasons":reasons(&item.shipping_reasons),
        "collectionReasons":reasons(&item.collection_reasons),
        "notices":item.notices,
    }))
}
fn personalized_data(item: &PersonalizedShipping) -> Result<Value, BoxError> {
    let a = &item.assessment;
    Ok(json!({
        "destination":geography(Some(&item.destination)),
        "attribution":item.attribution.as_ref().map(|a|json!({"provider":a.provider,"attributionUrl":a.attribution_url})),
        "evaluatedAt":timestamp(item.evaluated_at)?,
        "eligibility":a.eligibility.map(|e|e.as_str()),"selectedZone":a.selected_zone,
        "options":a.services.iter().enumerate().map(|(index,s)|json!({
            "serviceIndex":index,"applicability":s.applicability.as_str(),
            "eligibility":s.eligibility.map(|e|e.as_str()),"charge":charge(s.charge),"reasons":reasons(&s.reasons),
        })).collect::<Vec<_>>(),
        "reasons":reasons(&a.reasons),
        "collection":{"eligibility":a.collection.eligibility.map(|e|e.as_str()),"applicability":a.collection.applicability.as_str(),"reasons":reasons(&a.collection.reasons)},
        "bindingQuote":false,"completeLandedCost":false,
    }))
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShippingMatchValuationData {
    interpretation: &'static str,
    #[schemars(schema_with = "crate::wire::schema::fx_rate_id")]
    fx_rate_id: fxrate_core::FxRateId,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(with = "String")]
    captured_at: OffsetDateTime,
    maximum_per_item: ShippingMatchMaximumData,
}
#[derive(Debug, Serialize, JsonSchema)]
struct ShippingMatchMaximumData {
    #[schemars(range(max = u64::MAX))]
    amount: u64,
    #[serde(with = "crate::wire::currency")]
    #[schemars(schema_with = "crate::wire::currency::schema")]
    currency: money::Currency,
}
impl From<search_filter_core::ShippingMatchValuation> for ShippingMatchValuationData {
    fn from(v: search_filter_core::ShippingMatchValuation) -> Self {
        Self {
            interpretation:
                product_listing_core::shipping_search::ShippingSearch::FX_INTERPRETATION,
            fx_rate_id: v.fx_rate_id,
            captured_at: v.captured_at,
            maximum_per_item: ShippingMatchMaximumData {
                amount: v.maximum_per_item.monetary_amount.into(),
                currency: v.maximum_per_item.currency,
            },
        }
    }
}

impl JsonSchema for AdvertisedShippingData {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "AdvertisedShippingData".into()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        super::schema::AdvertisedShippingSchema::json_schema(generator)
    }
}
impl JsonSchema for PersonalizedShippingData {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "PersonalizedShippingData".into()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        super::schema::PersonalizedShippingSchema::json_schema(generator)
    }
}
