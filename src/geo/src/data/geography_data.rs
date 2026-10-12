//! Boundary codecs. All conversions back into core values call validated constructors.
use crate::{
    AddressText, Distance, DistanceUnit, GeoPoint, GeographicDescription, InvalidDistance,
    InvalidGeoText, InvalidPosition, InvalidSubdivisionCode, PostalCode, SpatialPosition,
    SpatialPrecision, SubdivisionCode,
    core::{
        country::{InvalidCountryCode, country_from_code},
        description::InvalidGeographicDescription,
    },
    reference::{ReferenceRelease, UnsupportedReferenceRelease},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum InvalidGeographyData {
    #[error("unsupported reference release")]
    Release(
        #[from]
        #[source]
        UnsupportedReferenceRelease,
    ),
    #[error("invalid country")]
    Country(
        #[from]
        #[source]
        InvalidCountryCode,
    ),
    #[error("invalid subdivision")]
    Subdivision(
        #[from]
        #[source]
        InvalidSubdivisionCode,
    ),
    #[error("invalid geographic text")]
    Text(
        #[from]
        #[source]
        InvalidGeoText,
    ),
    #[error("incompatible geographic assertions")]
    Compatibility(
        #[from]
        #[source]
        InvalidGeographicDescription,
    ),
    #[error("invalid position")]
    Position(
        #[from]
        #[source]
        InvalidPosition,
    ),
    #[error("invalid distance")]
    Distance(
        #[from]
        #[source]
        InvalidDistance,
    ),
    #[error("unsupported canonical spatial precision")]
    Precision,
    #[error("unsupported canonical distance unit")]
    DistanceUnit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeographicDescriptionData {
    reference_release: String,
    address_text: Option<String>,
    country: Option<String>,
    subdivision: Option<String>,
    postal_code: Option<String>,
}

impl TryFrom<GeographicDescriptionData> for Option<GeographicDescription> {
    type Error = InvalidGeographyData;
    fn try_from(data: GeographicDescriptionData) -> Result<Self, Self::Error> {
        let release = ReferenceRelease::from_code(&data.reference_release)?;
        let country = data
            .country
            .as_deref()
            .map(|code| country_from_code(code, release))
            .transpose()?;
        let subdivision = data
            .subdivision
            .as_deref()
            .map(|code| SubdivisionCode::from_code(code, release))
            .transpose()?;
        let address_text = data.address_text.map(AddressText::new).transpose()?;
        let postal_code = data.postal_code.map(PostalCode::new).transpose()?;
        GeographicDescription::new_in_release(
            release,
            address_text,
            country,
            subdivision,
            postal_code,
        )
        .map_err(Into::into)
    }
}

impl From<&GeographicDescription> for GeographicDescriptionData {
    fn from(value: &GeographicDescription) -> Self {
        Self {
            reference_release: value.reference_release().as_str().to_owned(),
            address_text: value.address_text().map(|text| text.as_str().to_owned()),
            country: value.country().map(|country| country.alpha2().to_owned()),
            subdivision: value.subdivision().map(|code| code.as_str().to_owned()),
            postal_code: value.postal_code().map(|code| code.as_str().to_owned()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeoPointData {
    latitude_degrees: f64,
    longitude_degrees: f64,
}

impl TryFrom<GeoPointData> for GeoPoint {
    type Error = InvalidPosition;
    fn try_from(value: GeoPointData) -> Result<Self, Self::Error> {
        Self::new(value.latitude_degrees, value.longitude_degrees)
    }
}

impl From<GeoPoint> for GeoPointData {
    fn from(point: GeoPoint) -> Self {
        Self {
            latitude_degrees: point.latitude_degrees(),
            longitude_degrees: point.longitude_degrees(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistanceData {
    amount: f64,
    unit: String,
}

impl TryFrom<DistanceData> for Distance {
    type Error = InvalidGeographyData;
    fn try_from(value: DistanceData) -> Result<Self, Self::Error> {
        let unit =
            DistanceUnit::from_code(&value.unit).ok_or(InvalidGeographyData::DistanceUnit)?;
        Self::new(value.amount, unit).map_err(Into::into)
    }
}

impl From<Distance> for DistanceData {
    fn from(distance: Distance) -> Self {
        Self {
            amount: distance.amount(),
            unit: distance.unit().as_str().to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpatialPositionData {
    point: GeoPointData,
    precision: String,
    accuracy_metres: Option<f64>,
}

impl TryFrom<SpatialPositionData> for SpatialPosition {
    type Error = InvalidGeographyData;
    fn try_from(value: SpatialPositionData) -> Result<Self, Self::Error> {
        let point = value.point.try_into()?;
        let precision =
            SpatialPrecision::from_code(&value.precision).ok_or(InvalidGeographyData::Precision)?;
        Self::new(point, precision, value.accuracy_metres).map_err(Into::into)
    }
}

impl From<SpatialPosition> for SpatialPositionData {
    fn from(value: SpatialPosition) -> Self {
        Self {
            point: value.point().into(),
            precision: value.precision().as_str().to_owned(),
            accuracy_metres: value.accuracy_metres(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_validate_numeric_values_during_boundary_rehydration() {
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                GeoPoint::try_from(GeoPointData {
                    latitude_degrees: invalid,
                    longitude_degrees: 0.0
                })
                .is_err()
            );
            assert!(
                GeoPoint::try_from(GeoPointData {
                    latitude_degrees: 0.0,
                    longitude_degrees: invalid
                })
                .is_err()
            );
            assert!(
                Distance::try_from(DistanceData {
                    amount: invalid,
                    unit: "METERS".to_owned()
                })
                .is_err()
            );
            assert!(
                SpatialPosition::try_from(SpatialPositionData {
                    point: GeoPointData {
                        latitude_degrees: 0.0,
                        longitude_degrees: 0.0
                    },
                    precision: "PREMISES".to_owned(),
                    accuracy_metres: Some(invalid),
                })
                .is_err()
            );
        }
    }
}
