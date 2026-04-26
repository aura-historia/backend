use crate::core::address::{GeoAddress, StructuredAddress};
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuredAddressData {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub address_lines: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub locality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub postal_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub country: Option<String>,
}

impl From<StructuredAddress> for StructuredAddressData {
    fn from(address: StructuredAddress) -> Self {
        Self {
            address_lines: address.address_lines,
            locality: address.locality,
            region: address.region,
            postal_code: address.postal_code,
            country: address.country,
        }
    }
}

impl From<StructuredAddressData> for StructuredAddress {
    fn from(address: StructuredAddressData) -> Self {
        Self {
            address_lines: address.address_lines,
            locality: address.locality,
            region: address.region,
            postal_code: address.postal_code,
            country: address.country,
        }
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeoAddressData {
    pub lat: f64,
    pub lon: f64,
}

impl From<GeoAddress> for GeoAddressData {
    fn from(address: GeoAddress) -> Self {
        Self {
            lat: address.lat,
            lon: address.lon,
        }
    }
}

impl From<GeoAddressData> for GeoAddress {
    fn from(address: GeoAddressData) -> Self {
        Self {
            lat: address.lat,
            lon: address.lon,
        }
    }
}
