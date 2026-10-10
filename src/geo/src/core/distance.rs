#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Distance {
    amount: f64,
    unit: DistanceUnit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("distance must be finite and nonnegative")]
pub struct InvalidDistance;

impl Distance {
    pub fn new(amount: f64, unit: DistanceUnit) -> Result<Self, InvalidDistance> {
        if !amount.is_finite() || amount < 0.0 {
            return Err(InvalidDistance);
        }
        Ok(Self { amount, unit })
    }

    pub fn amount(self) -> f64 {
        self.amount
    }
    pub fn unit(self) -> DistanceUnit {
        self.unit
    }
}

#[cfg(feature = "test-data")]
impl fake::Dummy<fake::Faker> for Distance {
    fn dummy_with_rng<R: fake::RngExt + ?Sized>(_config: &fake::Faker, rng: &mut R) -> Self {
        Self::new(rng.random_range(0.0..10000.0), DistanceUnit::Meters).unwrap()
    }
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
