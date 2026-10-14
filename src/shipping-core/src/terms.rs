use crate::{
    InvalidShippingValue,
    coverage::ShippingCoverage,
    inputs::Applicability,
    pricing::ShippingRate,
    time::{CollectionArrangement, LeadTimes},
    validate_text,
};
use geo::{GeographicDescription, SpatialPosition};
use listing_source_core::ListingSourceId;
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShippingEligibility {
    Offered,
    NotOffered,
    SubjectToConfirmation,
}
codes!(ShippingEligibility, Offered => "OFFERED", NotOffered => "NOT_OFFERED", SubjectToConfirmation => "SUBJECT_TO_CONFIRMATION");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ServiceFeature {
    Tracking,
    Insured,
    WhiteGlove,
    Installation,
    Packing,
}
codes!(ServiceFeature, Tracking => "TRACKING", Insured => "INSURED", WhiteGlove => "WHITE_GLOVE", Installation => "INSTALLATION", Packing => "PACKING");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryActor {
    ListingSource,
    Buyer,
    Party(PartyId),
}
impl DeliveryActor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ListingSource => "LISTING_SOURCE",
            Self::Buyer => "BUYER",
            Self::Party(_) => "PARTY",
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct SiteAssertions {
    geography: Option<GeographicDescription>,
    position: Option<SpatialPosition>,
}
impl SiteAssertions {
    pub fn new(
        geography: Option<GeographicDescription>,
        position: Option<SpatialPosition>,
    ) -> Result<Self, InvalidShippingValue> {
        if geography.is_none() && position.is_none() {
            return Err(InvalidShippingValue::EmptySite);
        }
        Ok(Self {
            geography,
            position,
        })
    }
    pub fn geography(&self) -> Option<&GeographicDescription> {
        self.geography.as_ref()
    }
    pub fn position(&self) -> Option<SpatialPosition> {
        self.position
    }
}
impl std::fmt::Debug for SiteAssertions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SiteAssertions(<redacted>)")
    }
}

/// The reference itself explicitly asserts shipping/collection use. Business roles do not.
#[derive(Debug, Clone, PartialEq)]
pub enum DeliverySite {
    PartyLocation(PartyLocationId),
    Asserted(SiteAssertions),
}
impl DeliverySite {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PartyLocation(_) => "PARTY_LOCATION",
            Self::Asserted(_) => "ASSERTED",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShippingServiceCode(String);
impl ShippingServiceCode {
    pub fn new(value: String) -> Result<Self, InvalidShippingValue> {
        validate_text(&value, 128)?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShippingService {
    code: ShippingServiceCode,
    applicability: Applicability,
    eligibility: Option<ShippingEligibility>,
    rate: Option<ShippingRate>,
    lead_times: LeadTimes,
    features: BTreeSet<ServiceFeature>,
    arranger: Option<DeliveryActor>,
    performer: Option<DeliveryActor>,
}
/// Independent source assertions used by construction and adapter rehydration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShippingServiceTerms {
    pub applicability: Applicability,
    pub eligibility: Option<ShippingEligibility>,
    pub rate: Option<ShippingRate>,
    pub lead_times: LeadTimes,
    pub features: BTreeSet<ServiceFeature>,
    pub arranger: Option<DeliveryActor>,
    pub performer: Option<DeliveryActor>,
}
impl ShippingService {
    pub fn new(code: ShippingServiceCode, terms: ShippingServiceTerms) -> Self {
        Self {
            code,
            applicability: terms.applicability,
            eligibility: terms.eligibility,
            rate: terms.rate,
            lead_times: terms.lead_times,
            features: terms.features,
            arranger: terms.arranger,
            performer: terms.performer,
        }
    }
    pub fn code(&self) -> &ShippingServiceCode {
        &self.code
    }
    pub fn applicability(&self) -> &Applicability {
        &self.applicability
    }
    pub fn eligibility(&self) -> Option<ShippingEligibility> {
        self.eligibility
    }
    pub fn rate(&self) -> Option<&ShippingRate> {
        self.rate.as_ref()
    }
    pub fn lead_times(&self) -> LeadTimes {
        self.lead_times
    }
    pub fn features(&self) -> &BTreeSet<ServiceFeature> {
        &self.features
    }
    pub fn arranger(&self) -> Option<DeliveryActor> {
        self.arranger
    }
    pub fn performer(&self) -> Option<DeliveryActor> {
        self.performer
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShippingPolicy {
    Prohibited,
    Coverage(Box<ShippingCoverage>),
}
impl ShippingPolicy {
    pub fn coverage(coverage: ShippingCoverage) -> Self {
        Self::Coverage(Box::new(coverage))
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Prohibited => "PROHIBITED",
            Self::Coverage(_) => "COVERAGE",
        }
    }
    pub(crate) fn validate_source(
        &self,
        source_id: ListingSourceId,
    ) -> Result<(), InvalidShippingValue> {
        match self {
            Self::Prohibited => Ok(()),
            Self::Coverage(coverage) => coverage.validate_source(source_id),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionDependency {
    Independent,
    Shipping,
}
codes!(CollectionDependency, Independent => "INDEPENDENT", Shipping => "SHIPPING");

#[derive(Debug, Clone, PartialEq)]
pub struct CollectionPolicy {
    applicability: Applicability,
    dependency: CollectionDependency,
    eligibility: Option<ShippingEligibility>,
    site: Option<DeliverySite>,
    arrangement: Option<CollectionArrangement>,
}
impl CollectionPolicy {
    pub fn new(
        applicability: Applicability,
        dependency: CollectionDependency,
        eligibility: Option<ShippingEligibility>,
        site: Option<DeliverySite>,
        arrangement: Option<CollectionArrangement>,
    ) -> Self {
        Self {
            applicability,
            dependency,
            eligibility,
            site,
            arrangement,
        }
    }
    pub fn applicability(&self) -> &Applicability {
        &self.applicability
    }
    pub fn dependency(&self) -> CollectionDependency {
        self.dependency
    }
    pub fn eligibility(&self) -> Option<ShippingEligibility> {
        self.eligibility
    }
    pub fn site(&self) -> Option<&DeliverySite> {
        self.site.as_ref()
    }
    pub fn arrangement(&self) -> Option<CollectionArrangement> {
        self.arrangement
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeliveryTerms {
    shipping: Option<ShippingPolicy>,
    collection: Option<CollectionPolicy>,
}
impl DeliveryTerms {
    pub fn new(shipping: Option<ShippingPolicy>, collection: Option<CollectionPolicy>) -> Self {
        Self {
            shipping,
            collection,
        }
    }
    pub fn shipping(&self) -> Option<&ShippingPolicy> {
        self.shipping.as_ref()
    }
    pub fn collection(&self) -> Option<&CollectionPolicy> {
        self.collection.as_ref()
    }
    pub(crate) fn validate_source(
        &self,
        source_id: ListingSourceId,
    ) -> Result<(), InvalidShippingValue> {
        if let Some(shipping) = &self.shipping {
            shipping.validate_source(source_id)?;
        }
        if let Some(collection) = &self.collection {
            collection.applicability.validate_source(source_id)?;
        }
        Ok(())
    }
}
