use crate::{
    InvalidShippingValue,
    inputs::Applicability,
    terms::{DeliverySite, ShippingService},
    validate_text,
};
use geo::{
    CountryCode, PostalCode, SubdivisionCode, country_from_code, reference::ReferenceRelease,
};
use listing_source_core::ListingSourceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceCountry {
    country: CountryCode,
    release: ReferenceRelease,
}
impl ReferenceCountry {
    pub fn new(code: &str, release: ReferenceRelease) -> Result<Self, geo::InvalidCountryCode> {
        Ok(Self {
            country: country_from_code(code, release)?,
            release,
        })
    }
    pub fn country(self) -> CountryCode {
        self.country
    }
    pub fn release(self) -> ReferenceRelease {
        self.release
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostalRuleVersion {
    V1,
}
codes!(PostalRuleVersion, V1 => "POSTAL_RULE_V1");

/// V1 permits exact text in any country and reviewed German five-digit forms.
/// Bounds stay fixed-width text; no country-independent numeric interpretation exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostalForm {
    Exact(PostalCode),
    GermanPrefix(String),
    GermanRange { first: String, last: String },
}
impl PostalForm {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Exact(_) => "EXACT",
            Self::GermanPrefix(_) => "GERMAN_PREFIX",
            Self::GermanRange { .. } => "GERMAN_RANGE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostalRule {
    version: PostalRuleVersion,
    country: ReferenceCountry,
    form: PostalForm,
}
impl PostalRule {
    pub fn new(
        version: PostalRuleVersion,
        country: ReferenceCountry,
        form: PostalForm,
    ) -> Result<Self, InvalidShippingValue> {
        let valid = match &form {
            PostalForm::Exact(_) => true,
            PostalForm::GermanPrefix(prefix) => {
                country.country == CountryCode::DEU
                    && (1..=4).contains(&prefix.len())
                    && prefix.bytes().all(|b| b.is_ascii_digit())
            }
            PostalForm::GermanRange { first, last } => {
                country.country == CountryCode::DEU
                    && german_postal_shape(first)
                    && german_postal_shape(last)
                    && first <= last
            }
        };
        if !valid {
            return Err(InvalidShippingValue::PostalRule);
        }
        Ok(Self {
            version,
            country,
            form,
        })
    }
    pub fn version(&self) -> PostalRuleVersion {
        self.version
    }
    pub fn country(&self) -> ReferenceCountry {
        self.country
    }
    pub fn form(&self) -> &PostalForm {
        &self.form
    }
}
pub(crate) fn german_postal_shape(value: &str) -> bool {
    value.len() == 5 && value.bytes().all(|b| b.is_ascii_digit())
}

/// An advertised area whose geographic interpretation has not been established.
/// This records an unresolved assertion and can never positively match a destination.
#[derive(Clone, PartialEq, Eq)]
pub struct UnresolvedArea(String);
impl UnresolvedArea {
    pub fn new(value: String) -> Result<Self, InvalidShippingValue> {
        validate_text(&value, 255)?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for UnresolvedArea {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UnresolvedArea(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryArea {
    Anywhere,
    Country(ReferenceCountry),
    /// Exact reference equality only; subdivision ancestry is never guessed.
    Subdivision(SubdivisionCode),
    Domestic,
    Postal(PostalRule),
    Unresolved(UnresolvedArea),
}
impl DeliveryArea {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Anywhere => "ANYWHERE",
            Self::Country(_) => "COUNTRY",
            Self::Subdivision(_) => "SUBDIVISION",
            Self::Domestic => "DOMESTIC",
            Self::Postal(_) => "POSTAL",
            Self::Unresolved(_) => "UNRESOLVED",
        }
    }
    pub(crate) fn validate_release(
        &self,
        release: ReferenceRelease,
    ) -> Result<(), InvalidShippingValue> {
        let own = match self {
            Self::Country(v) => Some(v.release()),
            Self::Subdivision(v) => Some(v.reference_release()),
            Self::Postal(v) => Some(v.country.release()),
            _ => None,
        };
        if own.is_some_and(|v| v != release) {
            return Err(InvalidShippingValue::ReferenceReleaseMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageCompleteness {
    Partial,
    Complete,
}
codes!(CoverageCompleteness, Partial => "PARTIAL", Complete => "COMPLETE");

#[derive(Debug, Clone, PartialEq)]
pub struct ShippingZone {
    areas: Vec<DeliveryArea>,
    applicability: Applicability,
    services: Vec<ShippingService>,
}
impl ShippingZone {
    pub fn new(
        areas: Vec<DeliveryArea>,
        applicability: Applicability,
        services: Vec<ShippingService>,
    ) -> Result<Self, InvalidShippingValue> {
        if areas.is_empty()
            || areas.len() > 64
            || services.is_empty()
            || services.len() > 64
            || services
                .iter()
                .enumerate()
                .any(|(i, service)| services[..i].iter().any(|s| s.code() == service.code()))
        {
            return Err(InvalidShippingValue::RuleCollection);
        }
        Ok(Self {
            areas,
            applicability,
            services,
        })
    }
    pub fn areas(&self) -> &[DeliveryArea] {
        &self.areas
    }
    pub fn applicability(&self) -> &Applicability {
        &self.applicability
    }
    pub fn services(&self) -> &[ShippingService] {
        &self.services
    }
}

/// Zones are ordered; all services in the first matching zone remain alternatives.
/// Completeness applies only inside the independently evaluated applicability scope.
#[derive(Debug, Clone, PartialEq)]
pub struct ShippingCoverage {
    release: ReferenceRelease,
    completeness: CoverageCompleteness,
    applicability: Applicability,
    exclusions: Vec<DeliveryArea>,
    zones: Vec<ShippingZone>,
    dispatch_origin: Option<DeliverySite>,
}
impl ShippingCoverage {
    pub fn new(
        release: ReferenceRelease,
        completeness: CoverageCompleteness,
        applicability: Applicability,
        exclusions: Vec<DeliveryArea>,
        zones: Vec<ShippingZone>,
        dispatch_origin: Option<DeliverySite>,
    ) -> Result<Self, InvalidShippingValue> {
        if exclusions.len() > 64 || zones.len() > 64 {
            return Err(InvalidShippingValue::RuleCollection);
        }
        for area in exclusions.iter().chain(zones.iter().flat_map(|z| &z.areas)) {
            area.validate_release(release)?;
        }
        Ok(Self {
            release,
            completeness,
            applicability,
            exclusions,
            zones,
            dispatch_origin,
        })
    }
    pub fn release(&self) -> ReferenceRelease {
        self.release
    }
    pub fn completeness(&self) -> CoverageCompleteness {
        self.completeness
    }
    pub fn applicability(&self) -> &Applicability {
        &self.applicability
    }
    pub fn exclusions(&self) -> &[DeliveryArea] {
        &self.exclusions
    }
    pub fn zones(&self) -> &[ShippingZone] {
        &self.zones
    }
    pub fn dispatch_origin(&self) -> Option<&DeliverySite> {
        self.dispatch_origin.as_ref()
    }
    pub(crate) fn validate_source(
        &self,
        source_id: ListingSourceId,
    ) -> Result<(), InvalidShippingValue> {
        self.applicability.validate_source(source_id)?;
        for zone in &self.zones {
            zone.applicability.validate_source(source_id)?;
            for service in &zone.services {
                service.applicability().validate_source(source_id)?;
            }
        }
        Ok(())
    }
}
