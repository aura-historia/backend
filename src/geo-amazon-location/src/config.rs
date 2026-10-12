use aws_sdk_geoplaces::{
    Client,
    config::{retry::RetryConfig, timeout::TimeoutConfig},
};
use geo_service::geocoding::ResultRetention;
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
}

#[derive(Clone)]
pub struct AmazonLocationConfig {
    pub(crate) deadline: Duration,
    pub(crate) attempt_timeout: Duration,
    pub(crate) max_attempts: u32,
    pub(crate) retention: ResultRetention,
}

impl AmazonLocationConfig {
    pub fn new(
        deadline: Duration,
        attempt_timeout: Duration,
        max_attempts: u32,
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
            retention: ResultRetention::SingleUse,
        })
    }

    /// Configure provider retention at composition time. Storage requests use AWS's
    /// higher storage pricing tier and reject results with prohibited/unknown country.
    pub fn with_retention(mut self, retention: ResultRetention) -> Self {
        self.retention = retention;
        self
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
            retention: ResultRetention::SingleUse,
        }
    }
}
