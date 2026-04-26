use crate::core::address::{GeoAddress, StructuredAddress};
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum GeocodingError {
    #[error("Cannot geocode an empty structured address")]
    EmptyAddress,
    #[error("Missing Google Geocoding API key")]
    MissingApiKey,
    #[error("Geocoding is disabled")]
    GeocodingDisabled,
    #[error("Google Geocoding API request failed: {0}")]
    RequestFailed(#[from] reqwest::Error),
    #[error("Google Geocoding API returned no result for address")]
    NoResult,
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait GeocodingService {
    async fn geocode(&self, address: &StructuredAddress) -> Result<GeoAddress, GeocodingError>;
}

pub struct GoogleGeocodingService {
    client: reqwest::Client,
    api_key: Option<String>,
}

impl GoogleGeocodingService {
    pub fn from_env() -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: std::env::var("GOOGLE_GEOCODING_API_KEY").ok(),
        }
    }
}

#[async_trait::async_trait]
impl GeocodingService for GoogleGeocodingService {
    async fn geocode(&self, address: &StructuredAddress) -> Result<GeoAddress, GeocodingError> {
        let address = address
            .format_for_geocoding()
            .ok_or(GeocodingError::EmptyAddress)?;
        let api_key = self.api_key.as_ref().ok_or(GeocodingError::MissingApiKey)?;
        let response = self
            .client
            .get("https://maps.googleapis.com/maps/api/geocode/json")
            .query(&[("address", address.as_str()), ("key", api_key.as_str())])
            .send()
            .await?
            .error_for_status()?
            .json::<GoogleGeocodingResponse>()
            .await?;
        let location = response
            .results
            .into_iter()
            .next()
            .map(|result| result.geometry.location)
            .ok_or(GeocodingError::NoResult)?;
        Ok(GeoAddress {
            lat: location.lat,
            lon: location.lng,
        })
    }
}

pub struct NoopGeocodingService;

#[async_trait::async_trait]
impl GeocodingService for NoopGeocodingService {
    async fn geocode(&self, _address: &StructuredAddress) -> Result<GeoAddress, GeocodingError> {
        Err(GeocodingError::GeocodingDisabled)
    }
}

#[derive(Deserialize)]
struct GoogleGeocodingResponse {
    results: Vec<GoogleGeocodingResult>,
}

#[derive(Deserialize)]
struct GoogleGeocodingResult {
    geometry: GoogleGeocodingGeometry,
}

#[derive(Deserialize)]
struct GoogleGeocodingGeometry {
    location: GoogleGeocodingLocation,
}

#[derive(Deserialize)]
struct GoogleGeocodingLocation {
    lat: f64,
    lng: f64,
}
