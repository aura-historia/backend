use crate::core::{
    address::{GeoAddress, StructuredAddress},
    continent::Continent,
};
use isocountry::CountryCode;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct StructuredAddressDocumentFields<TContinent> {
    pub addressline: Option<String>,
    pub addressline_extra: Option<String>,
    pub locality: Option<String>,
    pub region: Option<String>,
    pub postal_code: Option<String>,
    pub country: Option<CountryCode>,
    pub continent: Option<TContinent>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GeoAddressDocumentFields {
    pub geo_point: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

pub fn structured_address_to_document_fields<TContinent>(
    address: Option<&StructuredAddress>,
    continent_from_country: impl Fn(CountryCode) -> TContinent,
) -> StructuredAddressDocumentFields<TContinent> {
    let country = address.and_then(|a| a.country);
    StructuredAddressDocumentFields {
        addressline: address.and_then(|a| a.addressline.clone()),
        addressline_extra: address.and_then(|a| a.addressline_extra.clone()),
        locality: address.and_then(|a| a.locality.clone()),
        region: address.and_then(|a| a.region.clone()),
        postal_code: address.and_then(|a| a.postal_code.clone()),
        country,
        continent: country.map(continent_from_country),
    }
}

pub fn structured_address_from_document_fields<TContinent>(
    fields: StructuredAddressDocumentFields<TContinent>,
) -> Option<StructuredAddress> {
    let structured_address = StructuredAddress {
        addressline: fields.addressline,
        addressline_extra: fields.addressline_extra,
        locality: fields.locality,
        region: fields.region,
        postal_code: fields.postal_code,
        country: fields.country,
        continent: fields.country.map(Continent::from),
    };
    (!structured_address.is_empty()).then_some(structured_address)
}

pub fn geo_address_to_document_fields(address: Option<GeoAddress>) -> GeoAddressDocumentFields {
    GeoAddressDocumentFields {
        geo_point: address.map(GeoAddress::to_opensearch_geo_point),
        lat: address.map(|address| address.lat),
        lon: address.map(|address| address.lon),
    }
}

pub fn geo_address_from_geo_point(geo_point: Option<&str>) -> Option<GeoAddress> {
    geo_point.and_then(GeoAddress::from_opensearch_geo_point)
}

pub fn continent_from_country<TContinent>(country: CountryCode) -> TContinent
where
    TContinent: From<Continent>,
{
    TContinent::from(Continent::from(country))
}
