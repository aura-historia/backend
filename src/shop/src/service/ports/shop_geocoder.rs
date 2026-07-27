#![allow(dead_code)]

use crate::core::address::{GeoAddress, StructuredAddress};

#[derive(Debug, thiserror::Error)]
pub enum ShopGeocoderError {
    #[error("address not found")]
    NotFound,
    #[error("temporary geocoding failure")]
    TemporarilyUnavailable,
    #[error("internal geocoding failure")]
    Internal,
}

#[async_trait::async_trait]
pub(crate) trait ShopGeocoder: Send + Sync {
    async fn geocode(&self, address: &StructuredAddress) -> Result<GeoAddress, ShopGeocoderError>;
}
