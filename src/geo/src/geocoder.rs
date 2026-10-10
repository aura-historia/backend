use crate::AddressText;
use std::error::Error;

const GOOGLE_GEOCODING_V4_URL: &str = "https://geocode.googleapis.com/v4/geocode/address";

pub type GeocodingErrorSource = Box<dyn Error + Send + Sync + 'static>;

#[derive(Debug, thiserror::Error)]
pub enum GeocodingError {
    #[error("address not found")]
    NotFound,
    #[error("temporary geocoding failure")]
    TemporarilyUnavailable {
        #[source]
        source: GeocodingErrorSource,
    },
    #[error("internal geocoding failure")]
    Internal {
        #[source]
        source: GeocodingErrorSource,
    },
}

impl GeocodingError {
    pub fn temporarily_unavailable(source: impl Error + Send + Sync + 'static) -> Self {
        Self::TemporarilyUnavailable {
            source: Box::new(source),
        }
    }

    pub fn internal(source: impl Error + Send + Sync + 'static) -> Self {
        Self::Internal {
            source: Box::new(source),
        }
    }
}

#[async_trait::async_trait]
pub trait Geocoder: Send + Sync {
    async fn geocode(&self, address: &AddressText) -> Result<String, GeocodingError>;
}

#[async_trait::async_trait]
impl<G> Geocoder for std::sync::Arc<G>
where
    G: Geocoder + ?Sized,
{
    async fn geocode(&self, address: &AddressText) -> Result<String, GeocodingError> {
        self.as_ref().geocode(address).await
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct GoogleGeocoderConfig {
    api_key: String,
}

impl GoogleGeocoderConfig {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
        }
    }
}

/// Google Maps implementation of [`Geocoder`].
///
/// This adapter owns Google request and response types. Composition roots provide its API key;
/// this adapter never reads environment variables.
pub struct GoogleGeocoder {
    client: reqwest::Client,
    config: GoogleGeocoderConfig,
}

impl GoogleGeocoder {
    pub fn new(config: GoogleGeocoderConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }

    fn request(&self, address: &AddressText) -> reqwest::RequestBuilder {
        self.client
            .get(GOOGLE_GEOCODING_V4_URL)
            .query(&[("addressQuery", address.as_str())])
            .header("X-Goog-Api-Key", &self.config.api_key)
    }
}

#[async_trait::async_trait]
impl Geocoder for GoogleGeocoder {
    async fn geocode(&self, address: &AddressText) -> Result<String, GeocodingError> {
        let response = self
            .request(address)
            .send()
            .await
            .map_err(|source| GeocodingError::from(GoogleGeocoderRequestError(source)))?;

        if !response.status().is_success() {
            return Err(GoogleGeocoderResponseError {
                status: response.status(),
            }
            .into());
        }

        response
            .json::<GoogleGeocodingResponse>()
            .await
            .map_err(|source| GeocodingError::from(GoogleGeocoderDecodeError(source)))?
            .into_formatted_address()
            .ok_or(GeocodingError::NotFound)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Google geocoding request failed")]
struct GoogleGeocoderRequestError(#[source] reqwest::Error);

impl From<GoogleGeocoderRequestError> for GeocodingError {
    fn from(error: GoogleGeocoderRequestError) -> Self {
        if error.0.is_timeout() || error.0.is_connect() {
            Self::temporarily_unavailable(error)
        } else {
            Self::internal(error)
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Google geocoding response could not be decoded")]
struct GoogleGeocoderDecodeError(#[source] reqwest::Error);

impl From<GoogleGeocoderDecodeError> for GeocodingError {
    fn from(error: GoogleGeocoderDecodeError) -> Self {
        Self::internal(error)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Google geocoding returned HTTP {status}")]
struct GoogleGeocoderResponseError {
    status: reqwest::StatusCode,
}

impl From<GoogleGeocoderResponseError> for GeocodingError {
    fn from(error: GoogleGeocoderResponseError) -> Self {
        if error.status == reqwest::StatusCode::REQUEST_TIMEOUT
            || error.status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || error.status.is_server_error()
        {
            Self::temporarily_unavailable(error)
        } else {
            Self::internal(error)
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleGeocodingResponse {
    #[serde(default)]
    results: Vec<GoogleGeocodingResult>,
}

impl GoogleGeocodingResponse {
    fn into_formatted_address(self) -> Option<String> {
        self.results
            .into_iter()
            .find_map(GoogleGeocodingResult::into_formatted_address)
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleGeocodingResult {
    formatted_address: Option<String>,
}

impl GoogleGeocodingResult {
    fn into_formatted_address(self) -> Option<String> {
        self.formatted_address
            .filter(|address| !address.trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct StubGeocoder;

    #[async_trait::async_trait]
    impl Geocoder for StubGeocoder {
        async fn geocode(&self, address: &AddressText) -> Result<String, GeocodingError> {
            Ok(address.as_str().to_owned())
        }
    }

    #[tokio::test]
    async fn should_delegate_arc_geocoder() {
        let geocoder = Arc::new(StubGeocoder);
        let address = AddressText::new(" 10 Downing Street\nLondon ").unwrap();
        let result = <Arc<StubGeocoder> as Geocoder>::geocode(&geocoder, &address).await;

        assert!(matches!(
            result,
            Ok(formatted_address) if formatted_address == address.as_str()
        ));
    }

    #[test]
    fn should_encode_exact_address_text_in_google_request() {
        let geocoder = GoogleGeocoder::new(GoogleGeocoderConfig::new("test-key"));
        for source in [
            "10 Downing Street, London",
            "  東京都千代田区\r\n丸の内１丁目９−１  ",
            "شارع المتنبي\nبغداد / #1?x=y&regionCode=GB + %20",
        ] {
            let address = AddressText::new(source).unwrap();
            let request = geocoder.request(&address).build().unwrap();
            assert_eq!("geocode.googleapis.com", request.url().host_str().unwrap());
            assert_eq!("/v4/geocode/address", request.url().path());
            assert_eq!(None, request.url().fragment());
            assert_eq!(
                vec![("addressQuery".to_owned(), source.to_owned())],
                request.url().query_pairs().into_owned().collect::<Vec<_>>()
            );
            assert_eq!("test-key", request.headers()["X-Goog-Api-Key"]);
            assert_eq!(source, address.as_str());
        }
    }

    #[test]
    fn should_map_first_complete_google_result() {
        let response = GoogleGeocodingResponse {
            results: vec![
                GoogleGeocodingResult {
                    formatted_address: None,
                },
                GoogleGeocodingResult {
                    formatted_address: Some("10 Downing Street, London".to_owned()),
                },
            ],
        };

        assert_eq!(
            Some("10 Downing Street, London".to_owned()),
            response.into_formatted_address()
        );
    }

    #[test]
    fn should_ignore_blank_google_result() {
        let response = GoogleGeocodingResponse {
            results: vec![GoogleGeocodingResult {
                formatted_address: Some("  ".to_owned()),
            }],
        };

        assert_eq!(None, response.into_formatted_address());
    }

    #[test]
    fn should_map_transient_google_response_to_temporary_failure() {
        let error = GeocodingError::from(GoogleGeocoderResponseError {
            status: reqwest::StatusCode::TOO_MANY_REQUESTS,
        });

        assert!(matches!(
            error,
            GeocodingError::TemporarilyUnavailable { .. }
        ));
    }

    #[test]
    fn should_map_non_retryable_google_response_to_internal_failure() {
        let error = GeocodingError::from(GoogleGeocoderResponseError {
            status: reqwest::StatusCode::UNAUTHORIZED,
        });

        assert!(matches!(error, GeocodingError::Internal { .. }));
    }
}
