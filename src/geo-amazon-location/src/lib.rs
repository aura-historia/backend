//! Amazon Location Places v2 Geocode adapter. Credentials/SDK clients are injected.
mod config;
mod mapping;

pub use config::{
    AmazonLocationConfig, ENDPOINT, InvalidAmazonLocationConfig, REGION, TERMS_PROFILE,
};

use application::error::{box_error, static_error};
use aws_sdk_geoplaces::{
    Client,
    error::{ProvideErrorMetadata, SdkError},
    operation::geocode::{GeocodeError, builders::GeocodeFluentBuilder},
    types::{GeocodeFilter, GeocodeIntendedUse},
};
use geo::CountryCode;
use geo_service::geocoding::{
    GeocodingError, GeocodingFailure, GeocodingProvider, GeocodingRequest, GeocodingResult,
    ResultRetention,
};

pub struct AmazonLocationGeocoder {
    client: Client,
    config: AmazonLocationConfig,
    in_flight: tokio::sync::Semaphore,
}

const MAX_IN_FLIGHT: usize = 8;

impl AmazonLocationGeocoder {
    pub fn new(
        client: Client,
        config: AmazonLocationConfig,
    ) -> Result<Self, InvalidAmazonLocationConfig> {
        if client.config().region().map(|r| r.as_ref()) != Some(REGION) {
            return Err(InvalidAmazonLocationConfig::RegionOrEndpoint);
        }
        // Pin the reviewed endpoint and default resolver, overriding inherited endpoint
        // settings. A region/provider change needs a separate terms/quality review.
        Ok(Self {
            client: config.bounded_client(&client),
            config,
            in_flight: tokio::sync::Semaphore::new(MAX_IN_FLIGHT),
        })
    }

    fn request(&self, request: &GeocodingRequest) -> GeocodeFluentBuilder {
        let mut builder = self
            .client
            .geocode()
            .query_text(request.query().as_str())
            .max_results(request.candidate_limit() as i32)
            .intended_use(match self.config.retention {
                ResultRetention::Storage => GeocodeIntendedUse::Storage,
                ResultRetention::SingleUse => GeocodeIntendedUse::SingleUse,
            });
        if let Some(constraint) = request.constraint() {
            builder = builder.filter(
                GeocodeFilter::builder()
                    .include_countries(constraint.associated_country().alpha2())
                    .build(),
            );
        }
        if let Some(point) = request.bias_position() {
            // AWS uses [longitude, latitude]; G1 accessors explicitly name the other order.
            builder = builder.set_bias_position(Some(vec![
                point.longitude_degrees(),
                point.latitude_degrees(),
            ]));
        }
        if let Some(language) = request.language() {
            builder = builder.language(language.as_str());
        }
        // No submitted assertions are appended to or rewritten into QueryText. A caller
        // explicitly chooses hard constraints or a ranking hint independently of assertions.
        builder
    }
}

#[async_trait::async_trait]
impl GeocodingProvider for AmazonLocationGeocoder {
    async fn geocode(&self, request: &GeocodingRequest) -> Result<GeocodingResult, GeocodingError> {
        if request.query().as_str().chars().count() > 200 {
            return Err(GeocodingError {
                kind: GeocodingFailure::UnsupportedInput,
                source: static_error(
                    "Places v2 query text exceeds the provider's 200-character limit",
                ),
            });
        }
        if self.config.retention.permits_storage()
            && request
                .constraint()
                .is_some_and(|constraint| constraint.associated_country() == CountryCode::JPN)
        {
            return Err(GeocodingError {
                kind: GeocodingFailure::StorageNotPermitted,
                source: static_error("Places v2 results for Japan cannot be retained"),
            });
        }
        // Includes credential resolution, SDK backoff and response parsing. The SDK timeout
        // alone need not bound time spent resolving credentials outside operation execution.
        let response = tokio::time::timeout(self.config.deadline, async {
            let _permit = self
                .in_flight
                .acquire()
                .await
                .map_err(|source| GeocodingError {
                    kind: GeocodingFailure::Configuration,
                    source: box_error(source),
                })?;
            self.request(request).send().await.map_err(map_sdk_error)
        })
        .await
        .map_err(|source| GeocodingError {
            kind: GeocodingFailure::Timeout,
            source: box_error(source),
        })??;
        mapping::map_response(response, request, self.config.retention)
    }
}

fn map_sdk_error(source: SdkError<GeocodeError>) -> GeocodingError {
    let kind = match &source {
        SdkError::TimeoutError(_) => GeocodingFailure::Timeout,
        SdkError::DispatchFailure(error) if error.is_timeout() => GeocodingFailure::Timeout,
        SdkError::DispatchFailure(error) if error.is_io() => GeocodingFailure::Unavailable,
        SdkError::ConstructionFailure(_) | SdkError::DispatchFailure(_) => {
            GeocodingFailure::Configuration
        }
        SdkError::ServiceError(error) => {
            let status = error.raw().status().as_u16();
            match (status, error.err().code()) {
                (200..=299, _) => GeocodingFailure::InvalidResponse,
                (429, _) | (_, Some("ThrottlingException")) => GeocodingFailure::Throttled,
                (401 | 403, _)
                | (
                    _,
                    Some(
                        "AccessDeniedException"
                        | "UnrecognizedClientException"
                        | "InvalidSignatureException"
                        | "ExpiredTokenException",
                    ),
                ) => GeocodingFailure::Authentication,
                (408 | 504, _) => GeocodingFailure::Timeout,
                (500..=599, _) => GeocodingFailure::Unavailable,
                _ => GeocodingFailure::Configuration,
            }
        }
        SdkError::ResponseError(error) => match error.raw().status().as_u16() {
            401 | 403 => GeocodingFailure::Authentication,
            408 | 504 => GeocodingFailure::Timeout,
            429 => GeocodingFailure::Throttled,
            500..=599 => GeocodingFailure::Unavailable,
            400..=499 => GeocodingFailure::Configuration,
            _ => GeocodingFailure::InvalidResponse,
        },
        _ => GeocodingFailure::Unavailable,
    };
    // The original SDK error (including retry exhaustion) remains in the source chain.
    // Do not log its Display/Debug, raw response, or arbitrary provider message/code.
    GeocodingError {
        kind,
        source: box_error(source),
    }
}

#[cfg(test)]
mod tests;
