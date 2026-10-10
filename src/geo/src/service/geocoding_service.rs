//! Legacy opt-in Google compatibility. New consumers use `geo-service::geocoding`;
//! formatted strings from this interface are not a fallback for durable evidence.
use crate::AddressText;
use serde::Deserialize;

const GOOGLE_GEOCODING_V4_URL: &str = "https://geocode.googleapis.com/v4/geocode/address";

#[derive(Debug, thiserror::Error)]
pub enum GeocodingError {
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
    async fn geocode(&self, address: &AddressText) -> Result<String, GeocodingError>;
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
    async fn geocode(&self, address: &AddressText) -> Result<String, GeocodingError> {
        let response = self
            .client
            .get(&self.endpoint)
            .query(&[("addressQuery", address.as_str())])
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
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn address() -> AddressText {
        AddressText::new("10 Downing Street, London").unwrap()
    }

    fn service(endpoint: String) -> GoogleGeocodingService {
        GoogleGeocodingService {
            client: reqwest::Client::new(),
            api_key: "test-key".to_owned(),
            endpoint: format!("{endpoint}/v4/geocode/address"),
        }
    }

    #[tokio::test]
    async fn should_return_formatted_address_from_google() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header("X-Goog-Api-Key", "test-key"))
            .and(path("/v4/geocode/address"))
            .and(query_param("addressQuery", address().as_str()))
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
    async fn should_preserve_multiline_unicode_and_reserved_characters_in_google_request() {
        let server = MockServer::start().await;
        let source = "  東京都\r\n丸の内 / #1?x=y&regionCode=GB + %20  ";
        let address = AddressText::new(source).unwrap();
        Mock::given(method("GET"))
            .and(path("/v4/geocode/address"))
            .and(header("X-Goog-Api-Key", "test-key"))
            .and(query_param("addressQuery", source))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(
                    r#"{"results":[{"formattedAddress":"東京都千代田区丸の内"}]}"#,
                ),
            )
            .expect(1)
            .mount(&server)
            .await;

        assert_eq!(
            "東京都千代田区丸の内",
            service(server.uri()).geocode(&address).await.unwrap()
        );
        assert_eq!(source, address.as_str());
        let requests = server.received_requests().await.unwrap();
        assert_eq!(1, requests.len());
        assert_eq!(None, requests[0].url.fragment());
        assert_eq!(
            vec![("addressQuery".to_owned(), source.to_owned())],
            requests[0]
                .url
                .query_pairs()
                .into_owned()
                .collect::<Vec<_>>()
        );
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
    async fn geocode(&self, _address: &AddressText) -> Result<String, GeocodingError> {
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
