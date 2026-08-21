#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Distance {
    pub amount: f64,
    pub unit: DistanceUnit,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_preserve_geo_distance_query_value() {
        let query = GeoDistanceQuery {
            lat: 52.52,
            lon: 13.405,
            distance: Distance {
                amount: 50.0,
                unit: DistanceUnit::Kilometers,
            },
        };

        assert_eq!(52.52, query.lat);
        assert_eq!(13.405, query.lon);
        assert_eq!(50.0, query.distance.amount);
        assert_eq!(DistanceUnit::Kilometers, query.distance.unit);
    }
}
