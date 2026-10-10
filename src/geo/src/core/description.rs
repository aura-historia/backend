use super::{
    country::{CountryCode, InvalidCountryCode, country_from_code},
    position::SpatialPosition,
    subdivision::SubdivisionCode,
    text::{AddressText, PostalCode},
};
use crate::reference::ReferenceRelease;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidGeographicDescription {
    #[error("country is not recognized in the asserted reference release")]
    Country(
        #[from]
        #[source]
        InvalidCountryCode,
    ),
    #[error("subdivision does not belong to the asserted country")]
    IncompatibleCountrySubdivision,
    #[error("subdivision and description use different reference releases")]
    IncompatibleReferenceRelease,
}

/// Partial assertions. An empty description is absence, never patch intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeographicDescription {
    reference_release: ReferenceRelease,
    address_text: Option<AddressText>,
    country: Option<CountryCode>,
    subdivision: Option<SubdivisionCode>,
    postal_code: Option<PostalCode>,
}

impl GeographicDescription {
    pub fn new(
        address_text: Option<AddressText>,
        country: Option<CountryCode>,
        subdivision: Option<SubdivisionCode>,
        postal_code: Option<PostalCode>,
    ) -> Result<Option<Self>, InvalidGeographicDescription> {
        Self::new_in_release(
            ReferenceRelease::CURRENT,
            address_text,
            country,
            subdivision,
            postal_code,
        )
    }

    /// Reconstruct assertions using their original release, including country-only evidence.
    pub fn new_in_release(
        reference_release: ReferenceRelease,
        address_text: Option<AddressText>,
        country: Option<CountryCode>,
        subdivision: Option<SubdivisionCode>,
        postal_code: Option<PostalCode>,
    ) -> Result<Option<Self>, InvalidGeographicDescription> {
        if let Some(country) = country {
            country_from_code(country.alpha2(), reference_release)?;
        }
        if let Some(subdivision) = subdivision
            && subdivision.reference_release() != reference_release
        {
            return Err(InvalidGeographicDescription::IncompatibleReferenceRelease);
        }
        if let (Some(country), Some(subdivision)) = (country, subdivision)
            && country != subdivision.country()
        {
            return Err(InvalidGeographicDescription::IncompatibleCountrySubdivision);
        }
        if address_text.is_none()
            && country.is_none()
            && subdivision.is_none()
            && postal_code.is_none()
        {
            return Ok(None);
        }
        Ok(Some(Self {
            reference_release,
            address_text,
            country,
            subdivision,
            postal_code,
        }))
    }
    pub fn reference_release(&self) -> ReferenceRelease {
        self.reference_release
    }
    pub fn address_text(&self) -> Option<&AddressText> {
        self.address_text.as_ref()
    }
    pub fn country(&self) -> Option<CountryCode> {
        self.country
    }
    pub fn subdivision(&self) -> Option<SubdivisionCode> {
        self.subdivision
    }
    pub fn postal_code(&self) -> Option<&PostalCode> {
        self.postal_code.as_ref()
    }
}

/// Caller/source assertions; retain the original raw observation at the owning boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmittedGeography(GeographicDescription);

impl SubmittedGeography {
    pub fn new(description: GeographicDescription) -> Self {
        Self(description)
    }
    pub fn description(&self) -> &GeographicDescription {
        &self.0
    }
}

/// Derived evidence has a distinct type and never overwrites submitted assertions implicitly.
#[derive(Debug, Clone, PartialEq)]
pub struct DerivedGeography {
    description: Option<GeographicDescription>,
    position: Option<SpatialPosition>,
}

impl DerivedGeography {
    pub fn new(
        description: Option<GeographicDescription>,
        position: Option<SpatialPosition>,
    ) -> Option<Self> {
        (description.is_some() || position.is_some()).then_some(Self {
            description,
            position,
        })
    }
    pub fn description(&self) -> Option<&GeographicDescription> {
        self.description.as_ref()
    }
    pub fn position(&self) -> Option<SpatialPosition> {
        self.position
    }
}
