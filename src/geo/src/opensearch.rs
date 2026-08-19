use crate::core::{
    address::{GeoAddress, StructuredAddress},
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

pub fn geo_address_to_opensearch_point(address: GeoAddress) -> String {
    format!("{},{}", address.lat, address.lon)
}

pub fn geo_address_from_opensearch_point(value: &str) -> Option<GeoAddress> {
    let (lat, lon) = value.split_once(',')?;
    Some(GeoAddress {
        lat: lat.trim().parse().ok()?,
        lon: lon.trim().parse().ok()?,
    })
}

pub fn geo_address_to_document(address: Option<GeoAddress>) -> Option<String> {
    address.map(geo_address_to_opensearch_point)
}

pub fn geo_address_from_document(geo_point: Option<&str>) -> Option<GeoAddress> {
    geo_point.and_then(geo_address_from_opensearch_point)
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
