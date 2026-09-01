use crate::core::address::StructuredAddress;
use serde::Deserialize;

const GOOGLE_GEOCODING_V4_URL: &str = "https://geocode.googleapis.com/v4/geocode/address";

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
    async fn geocode(&self, address: &StructuredAddress) -> Result<String, GeocodingError>;
}

pub struct GoogleGeocodingService {
    client: reqwest::Client,
    api_key: String,
    endpoint: String,
}

impl GoogleGeocodingService {
    pub fn from_env() -> Result<Self, GeocodingError> {
        Ok(Self {
            client: reqwest::Client::new(),
            api_key: std::env::var("GOOGLE_GEOCODING_API_KEY")
                .map_err(|_| GeocodingError::MissingApiKey)?,
            endpoint: GOOGLE_GEOCODING_V4_URL.to_owned(),
        })
    }
}

#[async_trait::async_trait]
impl GeocodingService for GoogleGeocodingService {
    async fn geocode(&self, address: &StructuredAddress) -> Result<String, GeocodingError> {
        let address = address
            .format_for_geocoding()
            .ok_or(GeocodingError::EmptyAddress)?;
        let response = self
            .client
            .get(format!("{}/{address}", self.endpoint))
            .header("X-Goog-Api-Key", &self.api_key)
            .send()
            .await?
            .error_for_status()?
            .json::<GoogleGeocodingResponse>()
            .await?;
        response
            .into_formatted_address()
            .ok_or(GeocodingError::NoResult)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn address() -> StructuredAddress {
        StructuredAddress {
            addressline: Some("10 Downing Street".to_owned()),
            locality: Some("London".to_owned()),
            ..Default::default()
        }
    }

    fn service(endpoint: String) -> GoogleGeocodingService {
        GoogleGeocodingService {
            client: reqwest::Client::new(),
            api_key: "test-key".to_owned(),
            endpoint,
        }
    }

    #[tokio::test]
    async fn should_return_formatted_address_from_google() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header("X-Goog-Api-Key", "test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"results":[{"formattedAddress":"10 Downing Street, London"}]}"#,
            ))
            .mount(&server)
            .await;

        let result = service(server.uri()).geocode(&address()).await;

        assert!(matches!(
            result,
            Ok(formatted_address) if formatted_address == "10 Downing Street, London"
        ));
    }

    #[tokio::test]
    async fn should_return_no_result_for_blank_google_address() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"results":[{"formattedAddress":"  "}]}"#),
            )
            .mount(&server)
            .await;

        let result = service(server.uri()).geocode(&address()).await;

        assert!(matches!(result, Err(GeocodingError::NoResult)));
    }
}

pub struct NoopGeocodingService;

#[async_trait::async_trait]
impl GeocodingService for NoopGeocodingService {
    async fn geocode(&self, _address: &StructuredAddress) -> Result<String, GeocodingError> {
        Err(GeocodingError::GeocodingDisabled)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleGeocodingResponse {
    #[serde(default)]
    results: Vec<GoogleGeocodingV4Result>,
}

impl GoogleGeocodingResponse {
    fn into_formatted_address(self) -> Option<String> {
        self.results
            .into_iter()
            .find_map(GoogleGeocodingV4Result::into_formatted_address)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleGeocodingV4Result {
    formatted_address: Option<String>,
}

impl GoogleGeocodingV4Result {
    fn into_formatted_address(self) -> Option<String> {
        self.formatted_address
            .filter(|address| !address.trim().is_empty())
    }
}
