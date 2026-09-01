#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Distance {
    pub amount: f64,
    pub unit: DistanceUnit,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum_macros::EnumIter)]
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
    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;

        Self::iter().find(|unit| unit.as_str() == value)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Miles => "MILES",
            Self::Yards => "YARDS",
            Self::Feet => "FEET",
            Self::Inches => "INCHES",
            Self::Kilometers => "KILOMETERS",
            Self::Meters => "METERS",
            Self::Centimeters => "CENTIMETERS",
            Self::Millimeters => "MILLIMETERS",
            Self::NauticalMiles => "NAUTICAL_MILES",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    #[test]
    fn should_round_trip_all_canonical_distance_unit_codes() {
        for unit in DistanceUnit::iter() {
            assert_eq!(Some(unit), DistanceUnit::from_code(unit.as_str()));
        }
        assert_eq!(None, DistanceUnit::from_code("kilometers"));
    }
}
