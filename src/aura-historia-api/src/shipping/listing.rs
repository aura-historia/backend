//! Public advertised terms and private assessment fields on existing listing responses.
use crate::error::{ApiError, FORBIDDEN, SHIPPING_STORAGE_ERROR, SHIPPING_TEMPORARILY_UNAVAILABLE};
use application::error::{BoxError, box_error};
use geo::GeographicDescription;
use money::Price;
use product_listing_service::shipping::{AdvertisedShipping, PersonalizedShipping};
use serde::{Serialize, Serializer};
use serde_json::{Value, json};
use shipping_core::{
    assessment::{AssessedCharge, AssessmentReason},
    terms::{CollectionPolicy, DeliveryTerms, ShippingPolicy},
};
use shipping_service::assessment::{AssertionEvidence, AssessmentError};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

impl From<AssessmentError> for ApiError {
    fn from(error: AssessmentError) -> Self {
        match error {
            AssessmentError::InvalidInput | AssessmentError::InvalidState(_) => {
                Self::internal_server_error(SHIPPING_STORAGE_ERROR)
            }
            AssessmentError::Forbidden => Self::forbidden(FORBIDDEN),
            AssessmentError::Destination(error) => error.into(),
            AssessmentError::Storage(_) | AssessmentError::Transaction(_) => {
                Self::service_unavailable(SHIPPING_TEMPORARILY_UNAVAILABLE)
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct AdvertisedShippingData(pub AdvertisedShipping);
impl Serialize for AdvertisedShippingData {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        advertised_data(&self.0)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}
#[derive(Debug)]
pub(crate) struct PersonalizedShippingData(pub PersonalizedShipping);
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
fn money(p: Price) -> Value {
    json!({"amount_minor":u64::from(p.monetary_amount),"currency":p.currency.as_str()})
}
fn charge(c: Option<AssessedCharge>) -> Value {
    match c {
        None => Value::Null,
        Some(AssessedCharge::Fixed(p)) => json!({"kind":"FIXED","amount":money(p)}),
        Some(AssessedCharge::From(p)) => json!({"kind":"FROM","amount":money(p)}),
        Some(AssessedCharge::EstimatedRange(r)) => {
            json!({"kind":"ESTIMATED_RANGE","minimum":money(r.minimum()),"maximum":money(r.maximum())})
        }
    }
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

/// Cap shared caching at geographic provider evidence retention deadlines.
pub(crate) fn cache_ttl<'a>(
    items: impl IntoIterator<Item = Option<&'a AdvertisedShipping>>,
    maximum: u32,
) -> u32 {
    let now = OffsetDateTime::now_utc();
    items.into_iter().flatten().fold(maximum, |ttl, item| {
        item.geographic_valid_until
            .into_iter()
            .filter(|deadline| *deadline > item.evaluated_at)
            .fold(ttl, |ttl, deadline| {
                ttl.min(u32::try_from((deadline - now).whole_seconds().max(0)).unwrap_or(u32::MAX))
            })
    })
}
