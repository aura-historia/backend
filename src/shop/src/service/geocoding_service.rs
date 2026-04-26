use crate::core::address::{GeoAddress, StructuredAddress};
use serde::{Deserialize, Serialize};

const GOOGLE_GEOCODING_V4_URL: &str = "https://geocoding.googleapis.com/v4beta/geocode:address";

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
    api_key: String,
}

impl GoogleGeocodingService {
    pub fn from_env() -> Result<Self, GeocodingError> {
        Ok(Self {
            client: reqwest::Client::new(),
            api_key: std::env::var("GOOGLE_GEOCODING_API_KEY")
                .map_err(|_| GeocodingError::MissingApiKey)?,
        })
    }
}

#[async_trait::async_trait]
impl GeocodingService for GoogleGeocodingService {
    async fn geocode(&self, address: &StructuredAddress) -> Result<GeoAddress, GeocodingError> {
        let address = address
            .format_for_geocoding()
            .ok_or(GeocodingError::EmptyAddress)?;
        let response = self
            .client
            .post(GOOGLE_GEOCODING_V4_URL)
            .header("X-Goog-Api-Key", &self.api_key)
            .header(
                "X-Goog-FieldMask",
                "geocodingResults.geocoding.location,geocodingResults.geocode.location",
            )
            .json(&GoogleGeocodingRequest { address })
            .send()
            .await?
            .error_for_status()?
            .json::<GoogleGeocodingResponse>()
            .await?;
        response.into_geo_address().ok_or(GeocodingError::NoResult)
    }
}

pub struct NoopGeocodingService;

#[async_trait::async_trait]
impl GeocodingService for NoopGeocodingService {
    async fn geocode(&self, _address: &StructuredAddress) -> Result<GeoAddress, GeocodingError> {
        Err(GeocodingError::GeocodingDisabled)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GoogleGeocodingRequest {
    address: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleGeocodingResponse {
    #[serde(default)]
    geocoding_results: Vec<GoogleGeocodingV4Result>,
}

impl GoogleGeocodingResponse {
    fn into_geo_address(self) -> Option<GeoAddress> {
        self.geocoding_results
            .into_iter()
            .find_map(GoogleGeocodingV4Result::into_geo_address)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleGeocodingV4Result {
    geocoding: Option<GoogleGeocodingV4Geocode>,
    geocode: Option<GoogleGeocodingV4Geocode>,
}

impl GoogleGeocodingV4Result {
    fn into_geo_address(self) -> Option<GeoAddress> {
        self.geocoding
            .or(self.geocode)?
            .location?
            .into_geo_address()
    }
}

#[derive(Deserialize)]
struct GoogleGeocodingV4Geocode {
    location: Option<GoogleGeocodingV4Location>,
}

#[derive(Deserialize)]
struct GoogleGeocodingV4Location {
    latitude: Option<f64>,
    longitude: Option<f64>,
}

impl GoogleGeocodingV4Location {
    fn into_geo_address(self) -> Option<GeoAddress> {
        Some(GeoAddress {
            lat: self.latitude?,
            lon: self.longitude?,
        })
    }
}
