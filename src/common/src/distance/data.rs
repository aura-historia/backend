use geo::core::distance::{Distance, DistanceUnit, GeoDistanceQuery};
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DistanceData {
    pub amount: f64,
    pub unit: DistanceUnitData,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeoDistanceQueryData {
    pub lat: f64,
    pub lon: f64,
    pub distance: DistanceData,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DistanceUnitData {
    Miles,
    Yards,
    Feet,
    Inches,
    Kilometers,
    Meters,
    Centimeters,
    Millimeters,
    NauticalMiles,
}

impl From<DistanceData> for Distance {
    fn from(value: DistanceData) -> Self {
        Self {
            amount: value.amount,
            unit: value.unit.into(),
        }
    }
}

impl From<Distance> for DistanceData {
    fn from(value: Distance) -> Self {
        Self {
            amount: value.amount,
            unit: value.unit.into(),
        }
    }
}

impl From<GeoDistanceQueryData> for GeoDistanceQuery {
    fn from(value: GeoDistanceQueryData) -> Self {
        Self {
            lat: value.lat,
            lon: value.lon,
            distance: value.distance.into(),
        }
    }
}

impl From<GeoDistanceQuery> for GeoDistanceQueryData {
    fn from(value: GeoDistanceQuery) -> Self {
        Self {
            lat: value.lat,
            lon: value.lon,
            distance: value.distance.into(),
        }
    }
}

impl From<DistanceUnitData> for DistanceUnit {
    fn from(value: DistanceUnitData) -> Self {
        match value {
            DistanceUnitData::Miles => Self::Miles,
            DistanceUnitData::Yards => Self::Yards,
            DistanceUnitData::Feet => Self::Feet,
            DistanceUnitData::Inches => Self::Inches,
            DistanceUnitData::Kilometers => Self::Kilometers,
            DistanceUnitData::Meters => Self::Meters,
            DistanceUnitData::Centimeters => Self::Centimeters,
            DistanceUnitData::Millimeters => Self::Millimeters,
            DistanceUnitData::NauticalMiles => Self::NauticalMiles,
        }
    }
}

impl From<DistanceUnit> for DistanceUnitData {
    fn from(value: DistanceUnit) -> Self {
        match value {
            DistanceUnit::Miles => Self::Miles,
            DistanceUnit::Yards => Self::Yards,
            DistanceUnit::Feet => Self::Feet,
            DistanceUnit::Inches => Self::Inches,
            DistanceUnit::Kilometers => Self::Kilometers,
            DistanceUnit::Meters => Self::Meters,
            DistanceUnit::Centimeters => Self::Centimeters,
            DistanceUnit::Millimeters => Self::Millimeters,
            DistanceUnit::NauticalMiles => Self::NauticalMiles,
        }
    }
}
