use crate::core::{
    address::{GeoAddress, StructuredAddress},
    continent::Continent,
};
use isocountry::CountryCode;

pub fn structured_address_from_record(
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

pub fn geo_address_from_record(lat: Option<f64>, lon: Option<f64>) -> Option<GeoAddress> {
    Some(GeoAddress {
        lat: lat?,
        lon: lon?,
    })
}
