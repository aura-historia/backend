pub mod core;
#[cfg(feature = "google")]
pub mod geocoder;
pub mod reference;

#[cfg(feature = "google")]
pub use geocoder::{
    Geocoder, GeocodingError, GeocodingErrorSource, GoogleGeocoder, GoogleGeocoderConfig,
};

#[cfg(feature = "opensearch")]
pub mod opensearch;

pub use core::{
    continent::Continent,
    country::{CountryCode, InvalidCountryCode, country_from_code},
    description::{
        DerivedGeography, GeographicDescription, InvalidGeographicDescription, SubmittedGeography,
    },
    distance::{Distance, DistanceUnit, InvalidDistance},
    position::{GeoPoint, InvalidPosition, SpatialPosition, SpatialPrecision},
    subdivision::{InvalidSubdivisionCode, SubdivisionCode},
    text::{AddressText, InvalidGeoText, PostalCode},
};

#[cfg(feature = "data")]
pub mod data;

#[cfg(feature = "service")]
pub mod service;
