use crate::core::address::GeoAddress;
use serde::{Deserialize, Serialize};

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
