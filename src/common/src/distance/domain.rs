use std::fmt;

use crate::distance::data::{DistanceData, DistanceUnitData, GeoDistanceQueryData};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Distance {
    pub amount: f64,
    pub unit: DistanceUnit,
}

impl Distance {
    pub fn opensearch_value(&self) -> String {
        format!("{}{}", self.amount, self.unit.as_str())
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoDistanceQuery {
    pub lat: f64,
    pub lon: f64,
    pub distance: Distance,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DistanceUnit {
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

impl DistanceUnit {
    pub fn as_str(&self) -> &'static str {
        match self {
            DistanceUnit::Miles => "mi",
            DistanceUnit::Yards => "yd",
            DistanceUnit::Feet => "ft",
            DistanceUnit::Inches => "in",
            DistanceUnit::Kilometers => "km",
            DistanceUnit::Meters => "m",
            DistanceUnit::Centimeters => "cm",
            DistanceUnit::Millimeters => "mm",
            DistanceUnit::NauticalMiles => "nmi",
        }
    }
}

impl fmt::Display for DistanceUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<DistanceData> for Distance {
    fn from(data: DistanceData) -> Self {
        Self {
            amount: data.amount,
            unit: data.unit.into(),
        }
    }
}

impl From<Distance> for DistanceData {
    fn from(domain: Distance) -> Self {
        Self {
            amount: domain.amount,
            unit: domain.unit.into(),
        }
    }
}

impl From<GeoDistanceQueryData> for GeoDistanceQuery {
    fn from(data: GeoDistanceQueryData) -> Self {
        Self {
            lat: data.lat,
            lon: data.lon,
            distance: data.distance.into(),
        }
    }
}

impl From<GeoDistanceQuery> for GeoDistanceQueryData {
    fn from(domain: GeoDistanceQuery) -> Self {
        Self {
            lat: domain.lat,
            lon: domain.lon,
            distance: domain.distance.into(),
        }
    }
}

impl From<DistanceUnitData> for DistanceUnit {
    fn from(data: DistanceUnitData) -> Self {
        match data {
            DistanceUnitData::Miles => DistanceUnit::Miles,
            DistanceUnitData::Yards => DistanceUnit::Yards,
            DistanceUnitData::Feet => DistanceUnit::Feet,
            DistanceUnitData::Inches => DistanceUnit::Inches,
            DistanceUnitData::Kilometers => DistanceUnit::Kilometers,
            DistanceUnitData::Meters => DistanceUnit::Meters,
            DistanceUnitData::Centimeters => DistanceUnit::Centimeters,
            DistanceUnitData::Millimeters => DistanceUnit::Millimeters,
            DistanceUnitData::NauticalMiles => DistanceUnit::NauticalMiles,
        }
    }
}

impl From<DistanceUnit> for DistanceUnitData {
    fn from(domain: DistanceUnit) -> Self {
        match domain {
            DistanceUnit::Miles => DistanceUnitData::Miles,
            DistanceUnit::Yards => DistanceUnitData::Yards,
            DistanceUnit::Feet => DistanceUnitData::Feet,
            DistanceUnit::Inches => DistanceUnitData::Inches,
            DistanceUnit::Kilometers => DistanceUnitData::Kilometers,
            DistanceUnit::Meters => DistanceUnitData::Meters,
            DistanceUnit::Centimeters => DistanceUnitData::Centimeters,
            DistanceUnit::Millimeters => DistanceUnitData::Millimeters,
            DistanceUnit::NauticalMiles => DistanceUnitData::NauticalMiles,
        }
    }
}
