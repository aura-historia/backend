use crate::core::distance::{Distance, DistanceUnit};

pub fn distance_to_opensearch_value(distance: Distance) -> String {
    format!(
        "{}{}",
        distance.amount(),
        distance_unit_suffix(distance.unit())
    )
}

fn distance_unit_suffix(unit: DistanceUnit) -> &'static str {
    match unit {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_format_distance_for_opensearch() {
        assert_eq!(
            "50km",
            distance_to_opensearch_value(Distance::new(50.0, DistanceUnit::Kilometers).unwrap())
        );
        assert_eq!(
            "1.5nmi",
            distance_to_opensearch_value(Distance::new(1.5, DistanceUnit::NauticalMiles).unwrap())
        );
    }
}
