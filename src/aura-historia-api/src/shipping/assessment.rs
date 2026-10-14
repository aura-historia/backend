use crate::{
    auth::{OptionalAuthExtractor, TokenAuthenticator, request_metadata},
    error::{
        ApiError, BAD_BODY_VALUE, FORBIDDEN, PRODUCT_LISTING_NOT_FOUND, SHIPPING_STORAGE_ERROR,
        SHIPPING_TEMPORARILY_UNAVAILABLE,
    },
    wire::parse_path_object_id,
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use geo::{
    GeographicDescription, PostalCode, SubdivisionCode, country_from_code,
    reference::ReferenceRelease,
};
use money::{Currency, Price};
use product_listing_core::product_listing_id::ProductListingId;
use serde::Deserialize;
use serde_json::{Value, json};
use shipping_core::{
    assessment::{AssessedCharge, AssessmentReason},
    inputs::{AssessmentInput, SourceVariant},
    terms::{CollectionPolicy, DeliveryTerms, ShippingPolicy},
};
use shipping_service::assessment::*;
use std::{num::NonZeroU64, sync::Arc};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use user_core::user_address::UserAddressId;

#[derive(Clone)]
pub struct AssessmentState {
    pub assess: Arc<dyn AssessDeliveryUseCase>,
    pub authenticator: Arc<dyn TokenAuthenticator>,
}
fn bad() -> ApiError {
    ApiError::bad_request(BAD_BODY_VALUE).with_detail("Invalid delivery assessment input.")
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum DestinationData {
    None,
    Geographic {
        country: Option<String>,
        subdivision: Option<String>,
        #[serde(rename = "postalCode")]
        postal_code: Option<String>,
    },
    Address {
        #[serde(rename = "addressId")]
        address_id: UserAddressId,
    },
    Preference,
}
impl DestinationData {
    fn domain(self) -> Result<AssessmentDestination, ApiError> {
        Ok(match self {
            Self::None => AssessmentDestination::None,
            Self::Preference => AssessmentDestination::Preference,
            Self::Address { address_id } => AssessmentDestination::Address(address_id),
            Self::Geographic {
                country,
                subdivision,
                postal_code,
            } => AssessmentDestination::Geographic(
                GeographicDescription::new(
                    None,
                    country
                        .as_deref()
                        .map(|c| country_from_code(c, ReferenceRelease::CURRENT))
                        .transpose()
                        .map_err(|_| bad())?,
                    subdivision
                        .as_deref()
                        .map(|s| SubdivisionCode::from_code(s, ReferenceRelease::CURRENT))
                        .transpose()
                        .map_err(|_| bad())?,
                    postal_code
                        .map(PostalCode::new)
                        .transpose()
                        .map_err(|_| bad())?,
                )
                .map_err(|_| bad())?
                .ok_or_else(bad)?,
            ),
        })
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MoneyData {
    amount_minor: u64,
    currency: String,
}
impl MoneyData {
    fn domain(self) -> Result<Price, ApiError> {
        Ok(Price::new(
            self.amount_minor.into(),
            Currency::from_code(&self.currency).ok_or_else(bad)?,
        ))
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VariantData {
    listing_source_id: listing_source_core::ListingSourceId,
    code: String,
}
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InputData {
    quantity: Option<NonZeroU64>,
    lots: Option<NonZeroU64>,
    shipments: Option<NonZeroU64>,
    seller_orders: Option<NonZeroU64>,
    selected_variant: Option<VariantData>,
    item_value: Option<MoneyData>,
    lot_value: Option<MoneyData>,
    shipment_value: Option<MoneyData>,
    seller_order_value: Option<MoneyData>,
}
impl InputData {
    fn domain(self) -> Result<AssessmentInput, ApiError> {
        Ok(AssessmentInput {
            destination: None,
            quantity: self.quantity,
            lots: self.lots,
            shipments: self.shipments,
            seller_orders: self.seller_orders,
            selected_variant: self
                .selected_variant
                .map(|v| SourceVariant::new(v.listing_source_id, v.code))
                .transpose()
                .map_err(|_| bad())?,
            item_value: self.item_value.map(MoneyData::domain).transpose()?,
            lot_value: self.lot_value.map(MoneyData::domain).transpose()?,
            shipment_value: self.shipment_value.map(MoneyData::domain).transpose()?,
            seller_order_value: self.seller_order_value.map(MoneyData::domain).transpose()?,
        })
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListingData {
    product_listing_id: ProductListingId,
    #[serde(default)]
    input: InputData,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchData {
    destination: Option<DestinationData>,
    listings: Vec<ListingData>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DetailData {
    destination: Option<DestinationData>,
    #[serde(default)]
    input: InputData,
}
impl From<AssessmentError> for ApiError {
    fn from(e: AssessmentError) -> Self {
        match e {
            AssessmentError::InvalidInput => bad(),
            AssessmentError::Forbidden => Self::forbidden(FORBIDDEN),
            AssessmentError::NotFound => Self::not_found(PRODUCT_LISTING_NOT_FOUND),
            AssessmentError::Destination(e) => e.into(),
            AssessmentError::Storage(_) | AssessmentError::Transaction(_) => {
                Self::service_unavailable(SHIPPING_TEMPORARILY_UNAVAILABLE)
            }
            AssessmentError::InvalidState(_) => Self::internal_server_error(SHIPPING_STORAGE_ERROR),
        }
    }
}
pub async fn batch(State(s): State<AssessmentState>, headers: HeaderMap, body: String) -> Response {
    respond(
        s,
        headers,
        (|| {
            let d: BatchData = serde_json::from_str(&body).map_err(|_| bad())?;
            Ok(AssessDeliveryRequest {
                destination: d.destination.map(DestinationData::domain).transpose()?,
                listings: d
                    .listings
                    .into_iter()
                    .map(|l| {
                        Ok(AssessListingRequest {
                            listing_id: l.product_listing_id,
                            input: l.input.domain()?,
                        })
                    })
                    .collect::<Result<_, ApiError>>()?,
            })
        })(),
    )
    .await
}
pub async fn detail(
    State(s): State<AssessmentState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    respond(
        s,
        headers,
        (|| {
            let listing_id = parse_path_object_id(&id, "productListingId", "ProductListing")?;
            let d: DetailData = serde_json::from_str(&body).map_err(|_| bad())?;
            Ok(AssessDeliveryRequest {
                destination: d.destination.map(DestinationData::domain).transpose()?,
                listings: vec![AssessListingRequest {
                    listing_id,
                    input: d.input.domain()?,
                }],
            })
        })(),
    )
    .await
}
async fn respond(
    s: AssessmentState,
    headers: HeaderMap,
    request: Result<AssessDeliveryRequest, ApiError>,
) -> Response {
    let response = async {
        let metadata = request_metadata(&headers);
        let principal = OptionalAuthExtractor::new(s.authenticator.as_ref())
            .extract(&headers, &metadata)
            .await
            .map_err(ApiError::from)?;
        let view = s
            .assess
            .execute(&principal.operation_context(metadata), request?)
            .await
            .map_err(ApiError::from)?;
        response_data(view).map(Json)
    }
    .await;
    crate::transport::cache::private_no_store(match response {
        Ok(v) => v.into_response(),
        Err(e) => e.into_response(),
    })
}
fn timestamp(t: OffsetDateTime) -> Result<String, ApiError> {
    t.format(&Rfc3339)
        .map_err(|_| ApiError::internal_server_error(SHIPPING_STORAGE_ERROR))
}
fn geography(g: Option<&GeographicDescription>) -> Value {
    g.map_or(Value::Null, |g| json!({"referenceRelease":g.reference_release().as_str(),"country":g.country().map(|c|c.alpha2()),"subdivision":g.subdivision().map(|s|s.as_str()),"postalCode":g.postal_code().map(|p|p.as_str())}))
}
fn evidence_data(e: AssertionEvidence, at: OffsetDateTime) -> Result<Value, ApiError> {
    Ok(
        json!({"status": e.status(at).as_str(), "verifiedAt":timestamp(e.verified_at)?,"freshUntil":e.fresh_until.map(timestamp).transpose()?,"expiresAt":e.expires_at.map(timestamp).transpose()?}),
    )
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
fn public_terms(terms: &DeliveryTerms) -> Result<Value, ApiError> {
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
        super::terms::encode(&DeliveryTerms::new(terms.shipping().cloned(), collection))
            .map_err(|_| ApiError::internal_server_error(SHIPPING_STORAGE_ERROR))?;
    if matches!(terms.shipping(), Some(ShippingPolicy::Coverage(_))) {
        value["shipping"]["coverage"]["dispatch_origin"] = Value::Null;
    }
    Ok(value)
}
fn response_data(view: DeliveryAssessmentView) -> Result<Value, ApiError> {
    let mut items = Vec::new();
    for item in view.items {
        let terms = public_terms(item.effective.terms())?;
        let options: Vec<_> = item.assessment.services.iter().enumerate().map(|(i,s)| json!({
            "service":item.assessment.selected_zone.and_then(|z| terms.pointer(&format!("/shipping/coverage/zones/{z}/services/{i}"))).cloned(),
            "applicability":s.applicability.as_str(),"eligibility":s.eligibility.map(|e|e.as_str()),"charge":charge(s.charge),"reasons":reasons(&s.reasons),
        })).collect();
        let p = item.profile.as_ref().map(|p| Ok::<_,ApiError>(json!({"shippingProfileId":p.id,"termsRevision":p.terms_revision,"managementRevision":p.management_revision,"lifecycle":p.lifecycle.as_str(),"evidence":evidence_data(p.evidence,item.evaluated_at)?}))).transpose()?;
        items.push(json!({"productListingId":item.listing_id,"listingSourceId":item.source_id,"listingRevision":item.listing_revision,"sourceRevision":item.source_revision,
            "defaultRevision":item.default_revision,"profile":p,"listingEvidence":item.listing_evidence.map(|e|evidence_data(e,item.evaluated_at)).transpose()?,
            "evaluatedAt":timestamp(item.evaluated_at)?,"eligibility":item.assessment.eligibility.map(|e|e.as_str()),"selectedZone":item.assessment.selected_zone,
            "options":options,"reasons":reasons(&item.assessment.reasons),"notices":item.notices,"terms":terms,
            "collection":{"eligibility":item.assessment.collection.eligibility.map(|e|e.as_str()),"applicability":item.assessment.collection.applicability.as_str(),"reasons":reasons(&item.assessment.collection.reasons)},
            "locationRevisions":item.location_revisions.iter().map(|basis|json!({"partyLocationId":basis.id,"revision":basis.revision,"resolutionGeneration":basis.resolution_generation})).collect::<Vec<_>>(),
            "input":{"quantity":item.input.quantity,"lots":item.input.lots,"shipments":item.input.shipments,"sellerOrders":item.input.seller_orders,
                "selectedVariant":item.input.selected_variant.as_ref().map(|v|json!({"listingSourceId":v.source_id(),"code":v.code()})),
                "itemValue":item.input.item_value.map(money),"lotValue":item.input.lot_value.map(money),"shipmentValue":item.input.shipment_value.map(money),"sellerOrderValue":item.input.seller_order_value.map(money)},
            "bindingQuote":false,"completeLandedCost":false}));
    }
    Ok(
        json!({"destination":{"submitted":geography(view.destination.submitted.as_ref()),"derived":geography(view.destination.derived.as_ref()),"effective":geography(view.destination.effective.as_ref()),"attribution":view.destination.attribution.as_ref().map(|a|json!({"provider":a.provider,"attributionUrl":a.attribution_url}))},"items":items}),
    )
}
