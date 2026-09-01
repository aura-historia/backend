use crate::core::{
    address::StructuredAddress,
    continent::Continent,
    distance::{Distance, DistanceUnit},
};
use isocountry::CountryCode;

pub fn distance_to_opensearch_value(distance: Distance) -> String {
    format!("{}{}", distance.amount, distance_unit_suffix(distance.unit))
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

pub fn structured_address_from_document(
    addressline: Option<String>,
    addressline_extra: Option<String>,
    locality: Option<String>,
    region: Option<String>,
    postal_code: Option<String>,
    country: Option<CountryCode>,
) -> Option<StructuredAddress> {
    let structured_address = StructuredAddress {
        addressline,
        addressline_extra,
        locality,
        region,
        postal_code,
        country,
        continent: country.map(Continent::from),
    };
    (!structured_address.is_empty()).then_some(structured_address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_format_distance_for_opensearch() {
        assert_eq!(
            "50km",
            distance_to_opensearch_value(Distance {
                amount: 50.0,
                unit: DistanceUnit::Kilometers,
            })
        );
        assert_eq!(
            "1.5nmi",
            distance_to_opensearch_value(Distance {
                amount: 1.5,
                unit: DistanceUnit::NauticalMiles,
            })
        );
    }
}
