use crate::{
    coverage::{
        CoverageCompleteness, DeliveryArea, PostalForm, ShippingCoverage, german_postal_shape,
    },
    inputs::{Applicability, AssessmentInput, FactCompleteness, ListingRateInputs},
    pricing::{ChargeBasis, PriceRange, QualifyingScope, ShippingPrice},
    profile::{
        ShippingProfile, ShippingProfileId, ShippingProfileLifecycle, ShippingProfileRevision,
    },
    selection::{
        ComponentOverride, ListingDeliveryConfiguration, ProfileSelection, SourceDefaultProfile,
    },
    terms::{
        CollectionDependency, CollectionPolicy, DeliverySite, DeliveryTerms, ShippingEligibility,
        ShippingPolicy, ShippingService,
    },
};
use geo::{CountryCode, GeographicDescription, reference::ReferenceRelease};
use listing_source_core::ListingSourceId;
use money::Price;
use party_core::{party_location::PartyLocation, party_location_id::PartyLocationId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingInput {
    Destination,
    DestinationCountry,
    DestinationSubdivision,
    PostalCode,
    OriginCountry,
    Quantity,
    Lots,
    Shipments,
    SellerOrders,
    Variant,
    ClassMembership,
    QualifyingAmount(QualifyingScope),
}
impl MissingInput {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Destination => "DESTINATION",
            Self::DestinationCountry => "DESTINATION_COUNTRY",
            Self::DestinationSubdivision => "DESTINATION_SUBDIVISION",
            Self::PostalCode => "POSTAL_CODE",
            Self::OriginCountry => "ORIGIN_COUNTRY",
            Self::Quantity => "QUANTITY",
            Self::Lots => "LOTS",
            Self::Shipments => "SHIPMENTS",
            Self::SellerOrders => "SELLER_ORDERS",
            Self::Variant => "VARIANT",
            Self::ClassMembership => "CLASS_MEMBERSHIP",
            Self::QualifyingAmount(_) => "QUALIFYING_AMOUNT",
        }
    }
}

/// Reasons are deterministic and contain no buyer geography or arbitrary provider text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssessmentReason {
    MissingInput(MissingInput),
    NoSourceDefault,
    ProfileAbsent(ShippingProfileId),
    ProfileRetired(ShippingProfileId),
    SourceMismatch,
    AmbiguousDependency,
    LocationAbsent(PartyLocationId),
    LocationRetired(PartyLocationId),
    ReferenceReleaseMismatch,
    UnresolvedArea,
    UnsupportedPostalShape,
    CountryMismatch,
    SubdivisionMismatch,
    PostalMismatch,
    ClassMismatch,
    IncompleteClassMembership,
    VariantMismatch,
    VariantUnavailable,
    MixedVariantApplicability,
    QuantityOutsideRange,
    QualifyingThresholdNotMet,
    CurrencyMismatch,
    Excluded,
    EarlierZoneUndetermined { index: usize },
    IncompleteCoverage,
    OutsideCompleteCoverage,
    NoApplicableService,
    NoShippingAssertion,
    NoCollectionAssertion,
    NoEligibilityAssertion,
    SuppressedInheritance,
    AssertionCleared,
    ShippingProhibited,
    ShippingDependencyReplaced,
    ShippingDependencyUndetermined,
    UnknownChargeBasis,
    NoCurrencyAssertion,
    PriceOnRequest,
    ExternallyCalculatedPrice,
    ArithmeticOverflow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PredicateResult {
    Match,
    NoMatch(Vec<AssessmentReason>),
    Undetermined(Vec<AssessmentReason>),
}
impl PredicateResult {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Match => "MATCH",
            Self::NoMatch(_) => "NO_MATCH",
            Self::Undetermined(_) => "UNDETERMINED",
        }
    }
    pub fn reasons(&self) -> &[AssessmentReason] {
        match self {
            Self::Match => &[],
            Self::NoMatch(v) | Self::Undetermined(v) => v,
        }
    }
}
fn unknown(reason: AssessmentReason) -> PredicateResult {
    PredicateResult::Undetermined(vec![reason])
}
fn missing(input: MissingInput) -> PredicateResult {
    unknown(AssessmentReason::MissingInput(input))
}
fn no(reason: AssessmentReason) -> PredicateResult {
    PredicateResult::NoMatch(vec![reason])
}
fn extend_unique(
    target: &mut Vec<AssessmentReason>,
    reasons: impl IntoIterator<Item = AssessmentReason>,
) {
    for reason in reasons {
        if !target.contains(&reason) {
            target.push(reason);
        }
    }
}
fn all(results: impl IntoIterator<Item = PredicateResult>) -> PredicateResult {
    let mut negative = Vec::new();
    let mut uncertain = Vec::new();
    for result in results {
        match result {
            PredicateResult::Match => {}
            PredicateResult::NoMatch(r) => extend_unique(&mut negative, r),
            PredicateResult::Undetermined(r) => extend_unique(&mut uncertain, r),
        }
    }
    if !negative.is_empty() {
        PredicateResult::NoMatch(negative)
    } else if !uncertain.is_empty() {
        PredicateResult::Undetermined(uncertain)
    } else {
        PredicateResult::Match
    }
}
fn any(results: impl IntoIterator<Item = PredicateResult>) -> PredicateResult {
    let mut negative = Vec::new();
    let mut uncertain = Vec::new();
    for result in results {
        match result {
            PredicateResult::Match => return PredicateResult::Match,
            PredicateResult::NoMatch(r) => extend_unique(&mut negative, r),
            PredicateResult::Undetermined(r) => extend_unique(&mut uncertain, r),
        }
    }
    if uncertain.is_empty() {
        PredicateResult::NoMatch(negative)
    } else {
        PredicateResult::Undetermined(uncertain)
    }
}

/// Pure conjunction over the bounded rate facts. Absent facts never turn into matches.
pub fn evaluate_applicability(
    rule: &Applicability,
    facts: &ListingRateInputs,
    input: &AssessmentInput,
) -> PredicateResult {
    let source_id = rule.source_id().or(facts.source_id());
    if let Some(source_id) = source_id
        && (rule.validate_source(source_id).is_err() || facts.validate_source(source_id).is_err())
    {
        return unknown(AssessmentReason::SourceMismatch);
    }
    if let Some(reason) = selected_variant_reason(source_id, facts, input) {
        return unknown(reason);
    }
    let mut results = Vec::new();
    if let Some(class) = rule.class() {
        results.push(match facts.classes() {
            None => missing(MissingInput::ClassMembership),
            Some(membership) if membership.classes().contains(class) => PredicateResult::Match,
            Some(membership) if membership.completeness() == FactCompleteness::Complete => {
                no(AssessmentReason::ClassMismatch)
            }
            Some(_) => unknown(AssessmentReason::IncompleteClassMembership),
        });
    }
    if let Some(allowed) = rule.variants() {
        results.push(if let Some(selected) = &input.selected_variant {
            if !allowed.contains(selected) {
                no(AssessmentReason::VariantMismatch)
            } else {
                PredicateResult::Match
            }
        } else {
            match facts.variants() {
                None => missing(MissingInput::Variant),
                Some(available) => {
                    let count = available.iter().filter(|v| allowed.contains(v)).count();
                    if count == available.len() {
                        PredicateResult::Match
                    } else if count == 0 {
                        no(AssessmentReason::VariantMismatch)
                    } else {
                        unknown(AssessmentReason::MixedVariantApplicability)
                    }
                }
            }
        });
    }
    if let Some(range) = rule.quantity() {
        results.push(match input.quantity {
            None => missing(MissingInput::Quantity),
            Some(q) if q >= range.minimum() && q <= range.maximum() => PredicateResult::Match,
            Some(_) => no(AssessmentReason::QuantityOutsideRange),
        });
    }
    if let Some(threshold) = rule.threshold() {
        results.push(match input.qualifying_amount(threshold.scope) {
            None => missing(MissingInput::QualifyingAmount(threshold.scope)),
            Some(value) if value.currency != threshold.minimum.currency => {
                unknown(AssessmentReason::CurrencyMismatch)
            }
            Some(value) if value.monetary_amount >= threshold.minimum.monetary_amount => {
                PredicateResult::Match
            }
            Some(_) => no(AssessmentReason::QualifyingThresholdNotMet),
        });
    }
    all(results)
}

fn selected_variant_reason(
    source_id: Option<ListingSourceId>,
    facts: &ListingRateInputs,
    input: &AssessmentInput,
) -> Option<AssessmentReason> {
    let selected = input.selected_variant.as_ref()?;
    if source_id.is_some_and(|source| source != selected.source_id()) {
        return Some(AssessmentReason::SourceMismatch);
    }
    if facts
        .variants()
        .is_some_and(|available| !available.contains(selected))
    {
        return Some(AssessmentReason::VariantUnavailable);
    }
    None
}

fn usable_country(description: &GeographicDescription) -> Option<CountryCode> {
    description
        .country()
        .or_else(|| description.subdivision().map(|s| s.country()))
}
fn site_geography<'a>(
    site: &'a DeliverySite,
    locations: &'a [PartyLocation],
) -> Result<Option<&'a GeographicDescription>, AssessmentReason> {
    match site {
        DeliverySite::Asserted(assertions) => Ok(assertions.geography()),
        DeliverySite::PartyLocation(id) => {
            let mut found = locations.iter().filter(|location| location.id() == *id);
            let location = found.next().ok_or(AssessmentReason::LocationAbsent(*id))?;
            if found.next().is_some() {
                return Err(AssessmentReason::AmbiguousDependency);
            }
            if !location.eligible_for_assignment() {
                return Err(AssessmentReason::LocationRetired(*id));
            }
            Ok(location.content().geography.as_ref())
        }
    }
}

/// No ancestry, geocoding, continent grouping or postcode deliverability is inferred.
pub fn evaluate_area(
    area: &DeliveryArea,
    release: ReferenceRelease,
    input: &AssessmentInput,
    dispatch_origin: Option<&DeliverySite>,
    locations: &[PartyLocation],
) -> PredicateResult {
    if let DeliveryArea::Anywhere = area {
        return PredicateResult::Match;
    }
    if let DeliveryArea::Unresolved(_) = area {
        return unknown(AssessmentReason::UnresolvedArea);
    }
    if area.validate_release(release).is_err() {
        return unknown(AssessmentReason::ReferenceReleaseMismatch);
    }
    let Some(destination) = &input.destination else {
        return missing(MissingInput::Destination);
    };
    if destination.reference_release() != release {
        return unknown(AssessmentReason::ReferenceReleaseMismatch);
    }
    let Some(country) = usable_country(destination) else {
        return missing(MissingInput::DestinationCountry);
    };
    match area {
        DeliveryArea::Country(expected) => {
            if country == expected.country() {
                PredicateResult::Match
            } else {
                no(AssessmentReason::CountryMismatch)
            }
        }
        DeliveryArea::Subdivision(expected) => {
            if country != expected.country() {
                return no(AssessmentReason::CountryMismatch);
            }
            match destination.subdivision() {
                None => missing(MissingInput::DestinationSubdivision),
                Some(value) if value == *expected => PredicateResult::Match,
                Some(_) => no(AssessmentReason::SubdivisionMismatch),
            }
        }
        DeliveryArea::Domestic => {
            let Some(origin) = dispatch_origin else {
                return missing(MissingInput::OriginCountry);
            };
            let origin = match site_geography(origin, locations) {
                Err(reason) => return unknown(reason),
                Ok(None) => return missing(MissingInput::OriginCountry),
                Ok(Some(origin)) => origin,
            };
            if origin.reference_release() != release {
                return unknown(AssessmentReason::ReferenceReleaseMismatch);
            }
            match usable_country(origin) {
                None => missing(MissingInput::OriginCountry),
                Some(origin) if origin == country => PredicateResult::Match,
                Some(_) => no(AssessmentReason::CountryMismatch),
            }
        }
        DeliveryArea::Postal(rule) => {
            if country != rule.country().country() {
                return no(AssessmentReason::CountryMismatch);
            }
            let Some(postal) = destination.postal_code() else {
                return missing(MissingInput::PostalCode);
            };
            let matched = match rule.form() {
                PostalForm::Exact(expected) => postal.equivalent_in(expected, country),
                PostalForm::GermanPrefix(prefix) => {
                    if !german_postal_shape(postal.as_str()) {
                        return unknown(AssessmentReason::UnsupportedPostalShape);
                    }
                    postal.as_str().starts_with(prefix)
                }
                PostalForm::GermanRange { first, last } => {
                    if !german_postal_shape(postal.as_str()) {
                        return unknown(AssessmentReason::UnsupportedPostalShape);
                    }
                    postal.as_str() >= first.as_str() && postal.as_str() <= last.as_str()
                }
            };
            if matched {
                PredicateResult::Match
            } else {
                no(AssessmentReason::PostalMismatch)
            }
        }
        DeliveryArea::Anywhere | DeliveryArea::Unresolved(_) => {
            unreachable!("handled before destination evaluation")
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileBasis {
    pub profile_id: ShippingProfileId,
    pub revision: ShippingProfileRevision,
}

/// A derived snapshot. Services resolve fresh dependencies when authoritative assessment
/// is needed; this value does not acquire current state or perform cross-context reads.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveDeliveryTerms {
    source_id: ListingSourceId,
    terms: DeliveryTerms,
    rate_inputs: ListingRateInputs,
    profile_basis: Option<ProfileBasis>,
    shipping_reasons: Vec<AssessmentReason>,
    collection_reasons: Vec<AssessmentReason>,
}
impl EffectiveDeliveryTerms {
    pub fn resolve(
        configuration: &ListingDeliveryConfiguration,
        source_default: Option<&SourceDefaultProfile>,
        profiles: &[ShippingProfile],
    ) -> Self {
        let mut basis = None;
        let selected = match configuration.selection() {
            ProfileSelection::Inline(terms) => Ok(terms.as_ref().clone()),
            ProfileSelection::SuppressInheritance => Err(AssessmentReason::SuppressedInheritance),
            selection => {
                let id = match selection {
                    ProfileSelection::Explicit(id) => Ok(*id),
                    ProfileSelection::InheritSourceDefault => match source_default {
                        Some(default) if default.source_id() != configuration.source_id() => {
                            Err(AssessmentReason::SourceMismatch)
                        }
                        Some(default) => default
                            .profile_id()
                            .ok_or(AssessmentReason::NoSourceDefault),
                        None => Err(AssessmentReason::NoSourceDefault),
                    },
                    _ => unreachable!("handled above"),
                };
                id.and_then(|id| {
                    let mut found = profiles.iter().filter(|p| p.id() == id);
                    let profile = found.next().ok_or(AssessmentReason::ProfileAbsent(id))?;
                    if found.next().is_some() {
                        return Err(AssessmentReason::AmbiguousDependency);
                    }
                    if profile.source_id() != configuration.source_id() {
                        return Err(AssessmentReason::SourceMismatch);
                    }
                    if profile.lifecycle() == ShippingProfileLifecycle::Retired {
                        return Err(AssessmentReason::ProfileRetired(id));
                    }
                    basis = Some(ProfileBasis {
                        profile_id: id,
                        revision: profile.revision(),
                    });
                    Ok(profile.content().terms.clone())
                })
            }
        };
        let (base, base_reasons) = match selected {
            Ok(terms) => (terms, Vec::new()),
            Err(reason) => (DeliveryTerms::default(), vec![reason]),
        };
        let mut shipping_reasons = base_reasons.clone();
        let mut collection_reasons = base_reasons;
        let shipping = match &configuration.overrides().shipping {
            ComponentOverride::Inherit => base.shipping().cloned(),
            ComponentOverride::Replace(policy) => {
                shipping_reasons.clear();
                Some(policy.clone())
            }
            ComponentOverride::Clear => {
                shipping_reasons = vec![AssessmentReason::AssertionCleared];
                None
            }
        };
        let collection = match &configuration.overrides().collection {
            ComponentOverride::Replace(policy) => {
                collection_reasons.clear();
                Some(policy.clone())
            }
            ComponentOverride::Clear => {
                collection_reasons = vec![AssessmentReason::AssertionCleared];
                None
            }
            ComponentOverride::Inherit => {
                if !matches!(
                    configuration.overrides().shipping,
                    ComponentOverride::Inherit
                ) && base
                    .collection()
                    .is_some_and(|c| c.dependency() == CollectionDependency::Shipping)
                {
                    collection_reasons = vec![AssessmentReason::ShippingDependencyReplaced];
                    None
                } else {
                    base.collection().cloned()
                }
            }
        };
        Self {
            source_id: configuration.source_id(),
            terms: DeliveryTerms::new(shipping, collection),
            rate_inputs: configuration.rate_inputs().clone(),
            profile_basis: basis,
            shipping_reasons,
            collection_reasons,
        }
    }
    pub fn terms(&self) -> &DeliveryTerms {
        &self.terms
    }
    pub fn rate_inputs(&self) -> &ListingRateInputs {
        &self.rate_inputs
    }
    pub fn profile_basis(&self) -> Option<ProfileBasis> {
        self.profile_basis
    }
    pub fn shipping_reasons(&self) -> &[AssessmentReason] {
        &self.shipping_reasons
    }
    pub fn collection_reasons(&self) -> &[AssessmentReason] {
        &self.collection_reasons
    }
    pub fn assess(
        &self,
        input: &AssessmentInput,
        locations: &[PartyLocation],
    ) -> ShippingAssessment {
        assess(self, input, locations)
    }
}

/// An arithmetic projection of advertised charges, never an authorized quote or total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssessedCharge {
    Fixed(Price),
    From(Price),
    EstimatedRange(PriceRange),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServiceAssessment {
    pub service: ShippingService,
    pub applicability: PredicateResult,
    pub eligibility: Option<ShippingEligibility>,
    pub charge: Option<AssessedCharge>,
    pub reasons: Vec<AssessmentReason>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct CollectionAssessment {
    pub policy: Option<CollectionPolicy>,
    pub applicability: PredicateResult,
    pub eligibility: Option<ShippingEligibility>,
    pub reasons: Vec<AssessmentReason>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ShippingAssessment {
    pub profile_basis: Option<ProfileBasis>,
    pub eligibility: Option<ShippingEligibility>,
    pub selected_zone: Option<usize>,
    pub services: Vec<ServiceAssessment>,
    pub reasons: Vec<AssessmentReason>,
    pub collection: CollectionAssessment,
}

pub fn assess(
    effective: &EffectiveDeliveryTerms,
    input: &AssessmentInput,
    locations: &[PartyLocation],
) -> ShippingAssessment {
    if let Some(reason) =
        selected_variant_reason(Some(effective.source_id), &effective.rate_inputs, input)
    {
        return ShippingAssessment {
            profile_basis: effective.profile_basis,
            eligibility: None,
            selected_zone: None,
            services: Vec::new(),
            reasons: vec![reason.clone()],
            collection: CollectionAssessment {
                policy: effective.terms.collection().cloned(),
                applicability: unknown(reason.clone()),
                eligibility: None,
                reasons: vec![reason],
            },
        };
    }
    let (eligibility, selected_zone, services, reasons) = match effective.terms.shipping() {
        None => {
            let mut reasons = effective.shipping_reasons.clone();
            if reasons.is_empty() {
                reasons.push(AssessmentReason::NoShippingAssertion);
            }
            (None, None, Vec::new(), reasons)
        }
        Some(ShippingPolicy::Prohibited) => (
            Some(ShippingEligibility::NotOffered),
            None,
            Vec::new(),
            vec![AssessmentReason::ShippingProhibited],
        ),
        Some(ShippingPolicy::Coverage(coverage)) => {
            assess_coverage(coverage, &effective.rate_inputs, input, locations)
        }
    };
    let collection = assess_collection(effective, eligibility, input, locations);
    ShippingAssessment {
        profile_basis: effective.profile_basis,
        eligibility,
        selected_zone,
        services,
        reasons,
        collection,
    }
}

type CoverageAssessment = (
    Option<ShippingEligibility>,
    Option<usize>,
    Vec<ServiceAssessment>,
    Vec<AssessmentReason>,
);
fn assess_coverage(
    coverage: &ShippingCoverage,
    facts: &ListingRateInputs,
    input: &AssessmentInput,
    locations: &[PartyLocation],
) -> CoverageAssessment {
    match evaluate_applicability(coverage.applicability(), facts, input) {
        PredicateResult::Match => {}
        result => return (None, None, Vec::new(), result.reasons().to_vec()),
    }
    let mut uncertain_exclusions = Vec::new();
    for exclusion in coverage.exclusions() {
        match evaluate_area(
            exclusion,
            coverage.release(),
            input,
            coverage.dispatch_origin(),
            locations,
        ) {
            PredicateResult::Match => {
                return (
                    Some(ShippingEligibility::NotOffered),
                    None,
                    Vec::new(),
                    vec![AssessmentReason::Excluded],
                );
            }
            PredicateResult::Undetermined(reasons) => {
                extend_unique(&mut uncertain_exclusions, reasons)
            }
            PredicateResult::NoMatch(_) => {}
        }
    }
    for (index, zone) in coverage.zones().iter().enumerate() {
        let geographic = any(zone.areas().iter().map(|area| {
            evaluate_area(
                area,
                coverage.release(),
                input,
                coverage.dispatch_origin(),
                locations,
            )
        }));
        let applies = all([
            geographic,
            evaluate_applicability(zone.applicability(), facts, input),
        ]);
        match applies {
            PredicateResult::NoMatch(_) => continue,
            PredicateResult::Undetermined(reasons) => {
                extend_unique(&mut uncertain_exclusions, reasons);
                uncertain_exclusions.push(AssessmentReason::EarlierZoneUndetermined { index });
                return (None, None, Vec::new(), uncertain_exclusions);
            }
            PredicateResult::Match => {
                if !uncertain_exclusions.is_empty() {
                    return (None, None, Vec::new(), uncertain_exclusions);
                }
                let services: Vec<_> = zone
                    .services()
                    .iter()
                    .map(|service| assess_service(service, facts, input))
                    .collect();
                let mut reasons = Vec::new();
                for service in &services {
                    extend_unique(&mut reasons, service.reasons.clone());
                }
                let eligibility =
                    summarize_services(&services, coverage.completeness(), &mut reasons);
                return (eligibility, Some(index), services, reasons);
            }
        }
    }
    if coverage.completeness() == CoverageCompleteness::Complete {
        (
            Some(ShippingEligibility::NotOffered),
            None,
            Vec::new(),
            vec![AssessmentReason::OutsideCompleteCoverage],
        )
    } else {
        extend_unique(
            &mut uncertain_exclusions,
            [AssessmentReason::IncompleteCoverage],
        );
        (None, None, Vec::new(), uncertain_exclusions)
    }
}
fn summarize_services(
    services: &[ServiceAssessment],
    completeness: CoverageCompleteness,
    reasons: &mut Vec<AssessmentReason>,
) -> Option<ShippingEligibility> {
    if services
        .iter()
        .any(|s| s.eligibility == Some(ShippingEligibility::Offered))
    {
        return Some(ShippingEligibility::Offered);
    }
    if services
        .iter()
        .any(|s| s.eligibility == Some(ShippingEligibility::SubjectToConfirmation))
    {
        return Some(ShippingEligibility::SubjectToConfirmation);
    }
    if services.iter().any(|s| {
        matches!(s.applicability, PredicateResult::Undetermined(_))
            || (s.applicability == PredicateResult::Match && s.eligibility.is_none())
    }) {
        return None;
    }
    if !services
        .iter()
        .any(|s| s.eligibility == Some(ShippingEligibility::NotOffered))
    {
        reasons.push(AssessmentReason::NoApplicableService);
    }
    if completeness == CoverageCompleteness::Complete {
        Some(ShippingEligibility::NotOffered)
    } else {
        reasons.push(AssessmentReason::IncompleteCoverage);
        None
    }
}
fn assess_service(
    service: &ShippingService,
    facts: &ListingRateInputs,
    input: &AssessmentInput,
) -> ServiceAssessment {
    let applicability = evaluate_applicability(service.applicability(), facts, input);
    let mut reasons = applicability.reasons().to_vec();
    let eligibility = (applicability == PredicateResult::Match)
        .then(|| service.eligibility())
        .flatten();
    let charge = if matches!(
        eligibility,
        Some(ShippingEligibility::Offered | ShippingEligibility::SubjectToConfirmation)
    ) {
        assess_charge(service, input, &mut reasons)
    } else {
        None
    };
    if applicability == PredicateResult::Match && eligibility.is_none() {
        reasons.push(AssessmentReason::NoEligibilityAssertion);
    }
    ServiceAssessment {
        service: service.clone(),
        applicability,
        eligibility,
        charge,
        reasons,
    }
}
fn scaled(price: Price, count: u64) -> Option<Price> {
    u64::from(price.monetary_amount)
        .checked_mul(count)
        .map(|amount| Price::new(amount.into(), price.currency))
}
fn assess_charge(
    service: &ShippingService,
    input: &AssessmentInput,
    reasons: &mut Vec<AssessmentReason>,
) -> Option<AssessedCharge> {
    let rate = service.rate()?;
    match rate.price() {
        ShippingPrice::OnRequest(_) => {
            reasons.push(AssessmentReason::PriceOnRequest);
            return None;
        }
        ShippingPrice::ExternallyCalculated(_) => {
            reasons.push(AssessmentReason::ExternallyCalculatedPrice);
            return None;
        }
        _ => {}
    }
    let Some(basis) = rate.basis() else {
        reasons.push(AssessmentReason::UnknownChargeBasis);
        return None;
    };
    let (count, missing) = match basis {
        ChargeBasis::PerItem => (input.quantity, MissingInput::Quantity),
        ChargeBasis::PerLot => (input.lots, MissingInput::Lots),
        ChargeBasis::PerShipment => (input.shipments, MissingInput::Shipments),
        ChargeBasis::PerSellerOrder => (input.seller_orders, MissingInput::SellerOrders),
    };
    let Some(count) = count else {
        reasons.push(AssessmentReason::MissingInput(missing));
        return None;
    };
    let count = count.get();
    let charge = match rate.price() {
        ShippingPrice::Free(Some(currency)) => {
            Some(AssessedCharge::Fixed(Price::new(0_u64.into(), currency)))
        }
        ShippingPrice::Free(None) => {
            reasons.push(AssessmentReason::NoCurrencyAssertion);
            return None;
        }
        ShippingPrice::Fixed(price) => scaled(price, count).map(AssessedCharge::Fixed),
        ShippingPrice::From(price) => scaled(price, count).map(AssessedCharge::From),
        ShippingPrice::EstimatedRange(range) => scaled(range.minimum(), count)
            .zip(scaled(range.maximum(), count))
            .and_then(|(min, max)| PriceRange::new(min, max).ok())
            .map(AssessedCharge::EstimatedRange),
        ShippingPrice::OnRequest(_) | ShippingPrice::ExternallyCalculated(_) => {
            unreachable!("handled above")
        }
    };
    if charge.is_none() {
        reasons.push(AssessmentReason::ArithmeticOverflow);
    }
    charge
}

fn assess_collection(
    effective: &EffectiveDeliveryTerms,
    shipping_eligibility: Option<ShippingEligibility>,
    input: &AssessmentInput,
    locations: &[PartyLocation],
) -> CollectionAssessment {
    let Some(policy) = effective.terms.collection() else {
        let mut reasons = effective.collection_reasons.clone();
        if reasons.is_empty() {
            reasons.push(AssessmentReason::NoCollectionAssertion);
        }
        return CollectionAssessment {
            policy: None,
            applicability: PredicateResult::Undetermined(reasons.clone()),
            eligibility: None,
            reasons,
        };
    };
    let mut predicates = vec![evaluate_applicability(
        policy.applicability(),
        &effective.rate_inputs,
        input,
    )];
    if policy.dependency() == CollectionDependency::Shipping {
        predicates.push(match shipping_eligibility {
            Some(ShippingEligibility::Offered) => PredicateResult::Match,
            Some(ShippingEligibility::NotOffered) => no(AssessmentReason::ShippingProhibited),
            Some(ShippingEligibility::SubjectToConfirmation) | None => {
                unknown(AssessmentReason::ShippingDependencyUndetermined)
            }
        });
    }
    // An explicit prohibition needs no usable site. Otherwise referenced sites must be active.
    if policy.eligibility() != Some(ShippingEligibility::NotOffered)
        && let Some(site) = policy.site()
    {
        predicates.push(match site_geography(site, locations) {
            Ok(_) => PredicateResult::Match,
            Err(reason) => unknown(reason),
        });
    }
    let applicability = all(predicates);
    let eligibility = (applicability == PredicateResult::Match)
        .then(|| policy.eligibility())
        .flatten();
    let mut reasons = applicability.reasons().to_vec();
    if applicability == PredicateResult::Match && eligibility.is_none() {
        reasons.push(AssessmentReason::NoEligibilityAssertion);
    }
    CollectionAssessment {
        policy: Some(policy.clone()),
        applicability,
        eligibility,
        reasons,
    }
}
