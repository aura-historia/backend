use super::distance::{Distance, DistanceUnit};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidPosition {
    #[error("latitude must be finite and between -90 and 90 degrees")]
    Latitude,
    #[error("longitude must be finite and between -180 and 180 degrees")]
    Longitude,
    #[error("accuracy must be a finite nonnegative distance in metres")]
    Accuracy,
}

/// WGS 84 degrees, explicitly latitude then longitude. (0, 0) is a valid point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoPoint {
    latitude_degrees: f64,
    longitude_degrees: f64,
}

impl GeoPoint {
    pub fn new(latitude_degrees: f64, longitude_degrees: f64) -> Result<Self, InvalidPosition> {
        if !latitude_degrees.is_finite() || !(-90.0..=90.0).contains(&latitude_degrees) {
            return Err(InvalidPosition::Latitude);
        }
        if !longitude_degrees.is_finite() || !(-180.0..=180.0).contains(&longitude_degrees) {
            return Err(InvalidPosition::Longitude);
        }
        Ok(Self {
            latitude_degrees,
            longitude_degrees,
        })
    }
    pub fn latitude_degrees(self) -> f64 {
        self.latitude_degrees
    }
    pub fn longitude_degrees(self) -> f64 {
        self.longitude_degrees
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpatialPrecision {
    Premises,
    Street,
    Locality,
    Representative,
}

impl SpatialPrecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Premises => "PREMISES",
            Self::Street => "STREET",
            Self::Locality => "LOCALITY",
            Self::Representative => "REPRESENTATIVE",
        }
    }
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "PREMISES" => Some(Self::Premises),
            "STREET" => Some(Self::Street),
            "LOCALITY" => Some(Self::Locality),
            "REPRESENTATIVE" => Some(Self::Representative),
            _ => None,
        }
    }
}

/// Precision is explicit evidence, never inferred from decimal places or accuracy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpatialPosition {
    point: GeoPoint,
    precision: SpatialPrecision,
    accuracy_metres: Option<Distance>,
}

impl SpatialPosition {
    pub fn new(
        point: GeoPoint,
        precision: SpatialPrecision,
        accuracy_metres: Option<f64>,
    ) -> Result<Self, InvalidPosition> {
        let accuracy_metres = accuracy_metres
            .map(|amount| {
                Distance::new(amount, DistanceUnit::Meters).map_err(|_| InvalidPosition::Accuracy)
            })
            .transpose()?;
        Ok(Self {
            point,
            precision,
            accuracy_metres,
        })
    }
    pub fn point(self) -> GeoPoint {
        self.point
    }
    pub fn precision(self) -> SpatialPrecision {
        self.precision
    }
    pub fn accuracy_metres(self) -> Option<f64> {
        self.accuracy_metres.map(Distance::amount)
    }
}
