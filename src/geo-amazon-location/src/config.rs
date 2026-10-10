use aws_sdk_geoplaces::{
    Client,
    config::{retry::RetryConfig, timeout::TimeoutConfig},
};
use geo::CountryCode;
use geo_service::geocoding::{GeocodingPurpose, GeocodingRequest};
use std::time::Duration;

pub const REGION: &str = "eu-central-1";
pub const ENDPOINT: &str = "https://places.geo.eu-central-1.amazonaws.com";
pub const TERMS_PROFILE: &str = "AMAZON_LOCATION_PLACES_V2_DEFAULT_FRANKFURT_2026_10";

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidAmazonLocationConfig {
    #[error("only the reviewed Places v2 Frankfurt region/endpoint is supported")]
    RegionOrEndpoint,
    #[error(
        "deadline must be 1..=30 seconds, attempt timeout positive and no larger, attempts 1..=3"
    )]
    RequestBounds,
    #[error(
        "storage requires a nonblank review reference, countries excluding Japan and an enabled purpose"
    )]
    StorageReview,
}

/// Attestation to the deployment-specific review described in docs/geography.md.
/// Constructing this value records a review; it does not create provider rights.
#[derive(Clone)]
pub struct ReviewedStoragePolicy {
    review_reference: String,
    countries: Vec<CountryCode>,
    dealer_shared: bool,
    private_user: bool,
}

impl ReviewedStoragePolicy {
    pub fn new(
        review_reference: String,
        countries: Vec<CountryCode>,
        dealer_shared: bool,
        private_user: bool,
    ) -> Result<Self, InvalidAmazonLocationConfig> {
        if review_reference.trim().is_empty()
            || review_reference.len() > 256
            || review_reference.chars().any(char::is_control)
            || countries.is_empty()
            || countries.len() > 249
            || countries.contains(&CountryCode::JPN)
            || !(dealer_shared || private_user)
        {
            return Err(InvalidAmazonLocationConfig::StorageReview);
        }
        Ok(Self {
            review_reference,
            countries,
            dealer_shared,
            private_user,
        })
    }
    pub(crate) fn permits(&self, request: &GeocodingRequest) -> bool {
        let purpose_allowed = match request.purpose() {
            GeocodingPurpose::DealerReusable => self.dealer_shared,
            GeocodingPurpose::PrivateRetained { .. } => self.private_user,
            _ => false,
        };
        // Required hard country constraint bounds the returned data to the reviewed market.
        purpose_allowed
            && request
                .constraint()
                .is_some_and(|c| self.countries.contains(&c.associated_country()))
    }
    pub(crate) fn review_reference(&self) -> &str {
        &self.review_reference
    }
}

#[derive(Clone, Default)]
pub enum StoragePolicy {
    #[default]
    Disabled,
    Reviewed(ReviewedStoragePolicy),
}

#[derive(Clone)]
pub struct AmazonLocationConfig {
    pub(crate) deadline: Duration,
    pub(crate) attempt_timeout: Duration,
    pub(crate) max_attempts: u32,
    pub(crate) storage: StoragePolicy,
}

impl AmazonLocationConfig {
    pub fn new(
        deadline: Duration,
        attempt_timeout: Duration,
        max_attempts: u32,
        storage: StoragePolicy,
    ) -> Result<Self, InvalidAmazonLocationConfig> {
        if !(Duration::from_secs(1)..=Duration::from_secs(30)).contains(&deadline)
            || attempt_timeout.is_zero()
            || attempt_timeout > deadline
            || !(1..=3).contains(&max_attempts)
        {
            return Err(InvalidAmazonLocationConfig::RequestBounds);
        }
        Ok(Self {
            deadline,
            attempt_timeout,
            max_attempts,
            storage,
        })
    }

    pub(crate) fn bounded_client(&self, client: &Client) -> Client {
        // Override inherited unbounded retry/timeout settings. Geocode is a read, so the
        // standard SDK transient/throttling retry classifier and jitter are appropriate.
        Client::from_conf(
            client
                .config()
                .to_builder()
                .endpoint_url(ENDPOINT)
                .endpoint_resolver(aws_sdk_geoplaces::config::endpoint::DefaultResolver::new())
                .retry_config(RetryConfig::standard().with_max_attempts(self.max_attempts))
                .timeout_config(
                    TimeoutConfig::builder()
                        .operation_timeout(self.deadline)
                        .operation_attempt_timeout(self.attempt_timeout)
                        .connect_timeout(self.attempt_timeout.min(Duration::from_secs(1)))
                        .read_timeout(self.attempt_timeout)
                        .build(),
                )
                .build(),
        )
    }
}

impl Default for AmazonLocationConfig {
    fn default() -> Self {
        Self {
            deadline: Duration::from_secs(6),
            attempt_timeout: Duration::from_secs(2),
            max_attempts: 3,
            storage: StoragePolicy::Disabled,
        }
    }
}
