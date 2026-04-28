use crate::core::{
    address::{GeoAddress, StructuredAddress},
    continent::Continent,
};
use isocountry::CountryCode;

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

pub fn geo_address_to_document(address: Option<GeoAddress>) -> Option<String> {
    address.map(GeoAddress::to_opensearch_geo_point)
}

pub fn geo_address_from_document(geo_point: Option<&str>) -> Option<GeoAddress> {
    geo_point.and_then(GeoAddress::from_opensearch_geo_point)
}
