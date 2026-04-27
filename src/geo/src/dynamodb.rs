use crate::core::{
    address::{GeoAddress, StructuredAddress},
    continent::Continent,
};
use isocountry::CountryCode;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct StructuredAddressFlat {
    pub addressline: Option<String>,
    pub addressline_extra: Option<String>,
    pub locality: Option<String>,
    pub region: Option<String>,
    pub postal_code: Option<String>,
    pub country: Option<CountryCode>,
}

#[derive(Debug, Copy, Clone, PartialEq, Default)]
pub struct GeoAddressFlat {
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

pub fn structured_address_to_flat(address: Option<&StructuredAddress>) -> StructuredAddressFlat {
    StructuredAddressFlat {
        addressline: address.and_then(|a| a.addressline.clone()),
        addressline_extra: address.and_then(|a| a.addressline_extra.clone()),
        locality: address.and_then(|a| a.locality.clone()),
        region: address.and_then(|a| a.region.clone()),
        postal_code: address.and_then(|a| a.postal_code.clone()),
        country: address.and_then(|a| a.country),
    }
}

pub fn structured_address_from_flat(flat: StructuredAddressFlat) -> Option<StructuredAddress> {
    let structured_address = StructuredAddress {
        addressline: flat.addressline,
        addressline_extra: flat.addressline_extra,
        locality: flat.locality,
        region: flat.region,
        postal_code: flat.postal_code,
        country: flat.country,
        continent: flat.country.map(Continent::from),
    };
    (!structured_address.is_empty()).then_some(structured_address)
}

pub fn geo_address_to_flat(address: Option<GeoAddress>) -> GeoAddressFlat {
    GeoAddressFlat {
        lat: address.map(|address| address.lat),
        lon: address.map(|address| address.lon),
    }
}

pub fn geo_address_from_flat(flat: GeoAddressFlat) -> Option<GeoAddress> {
    Some(GeoAddress {
        lat: flat.lat?,
        lon: flat.lon?,
    })
}
