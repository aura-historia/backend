use crate::CrawlerDomainId;
use crate::network::policy::{DomainFailureKind, NetworkErrorKind, domain_failure_kind};
use crate::scraper::scraper_service::domain::errors::ScraperError;
use listing_source_core::ListingSourceId;
use money::Currency;
use product_listing_normalization::{
    ListingAvailabilityQuickCheck, ProductListingNormalizationInput,
};
use std::cell::RefCell;
use std::future::Future;
use url::Url;

/// Result of a successful scrape — the raw normalization input together with
/// metadata needed to mark the URL as scraped after durable raw capture.
#[derive(Debug)]
pub struct ScrapedProduct {
    /// Complete source-neutral normalization input. The worker later performs canonical writes.
    pub raw_input: ProductListingNormalizationInput,
    /// Pure crawler quick-check used only for local disposition.
    pub availability: ListingAvailabilityQuickCheck,
    /// SHA-256 of the page's `<main>` fragment (or full HTML) that was used to
    /// detect whether the page had changed.
    pub hash: String,
    /// Deterministic fingerprint of the effective ordered schema set.
    pub schema_fingerprint: String,
    /// Shared provider-neutral raw-input hash used for local change detection.
    pub raw_input_sha256: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrapeMode {
    Normal,
    DomainProbe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchFailureSource {
    Primary,
    SchemaSeed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchFailureContext {
    pub url: Url,
    pub kind: NetworkErrorKind,
    pub status_code: Option<u16>,
    pub retry_after: Option<std::time::Duration>,
    pub source: FetchFailureSource,
    pub expected_last_captured_raw_input_sha256: Option<Vec<u8>>,
}

/// All inputs required to execute one scraper request. Keeping the request as
/// one value also makes the probe boundary explicit and prevents the mode
/// call from growing another positional argument.
#[derive(Debug, Clone)]
pub struct ScrapeRequest {
    pub listing_source_id: ListingSourceId,
    pub url: Url,
    pub product_url_pattern: Option<String>,
    pub last_scraped_hash: Option<String>,
    pub last_scraped_schema_fingerprint: Option<String>,
    pub expected_last_captured_raw_input_sha256: Option<Vec<u8>>,
    pub fallback_currency: Option<Currency>,
    pub mode: ScrapeMode,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DomainFetchHealth {
    #[default]
    NotObserved,
    Responsive,
    CircuitFailure {
        kind: DomainFailureKind,
        status_code: Option<u16>,
        retry_after: Option<std::time::Duration>,
    },
}

pub struct ScrapeOutcome {
    pub result: Result<Option<ScrapedProduct>, ScraperError>,
    pub domain_health: DomainFetchHealth,
    pub fetch_failure: Option<FetchFailureContext>,
}

struct ScrapeObservationState {
    domain_id: CrawlerDomainId,
    transport_health: Option<DomainFetchHealth>,
    fetch_failure: Option<FetchFailureContext>,
}

tokio::task_local! {
    static SCRAPE_OBSERVATION: RefCell<ScrapeObservationState>;
}

/// Runs a scrape with the persisted domain identity and returns the transport
/// observation collected by the primary/seed fetches. Test doubles that do not
/// participate in this context return `None` and retain the legacy fallback at
/// the caller boundary.
pub(crate) async fn with_scrape_observation<F, T>(
    domain_id: CrawlerDomainId,
    future: F,
) -> (T, Option<DomainFetchHealth>)
where
    F: Future<Output = T>,
{
    SCRAPE_OBSERVATION
        .scope(
            RefCell::new(ScrapeObservationState {
                domain_id,
                transport_health: None,
                fetch_failure: None,
            }),
            async {
                let result = future.await;
                let observation =
                    SCRAPE_OBSERVATION.with(|state| state.borrow().transport_health.clone());
                (result, observation)
            },
        )
        .await
}

pub(crate) fn current_scrape_domain_id() -> Option<CrawlerDomainId> {
    SCRAPE_OBSERVATION
        .try_with(|state| state.borrow().domain_id)
        .ok()
}

pub(crate) fn begin_transport_observation() {
    let _ = SCRAPE_OBSERVATION.try_with(|state| {
        state.borrow_mut().transport_health = Some(DomainFetchHealth::NotObserved);
    });
}

pub(crate) fn current_transport_observation() -> Option<DomainFetchHealth> {
    SCRAPE_OBSERVATION
        .try_with(|state| state.borrow().transport_health.clone())
        .ok()
        .flatten()
}

pub(crate) fn current_fetch_failure() -> Option<FetchFailureContext> {
    SCRAPE_OBSERVATION
        .try_with(|state| state.borrow().fetch_failure.clone())
        .ok()
        .flatten()
}

pub(crate) fn record_transport_observation(health: DomainFetchHealth) {
    let _ = SCRAPE_OBSERVATION.try_with(|state| {
        state.borrow_mut().transport_health = Some(health);
    });
}

pub(crate) fn record_transport_success() {
    record_transport_observation(DomainFetchHealth::Responsive);
}

pub(crate) fn record_transport_fetch_error(
    kind: NetworkErrorKind,
    status_code: Option<u16>,
    retry_after: Option<std::time::Duration>,
) {
    record_transport_observation(domain_health_for_fetch_error(
        kind,
        status_code,
        retry_after,
    ));
}

pub(crate) fn record_transport_failure(
    url: &Url,
    source: FetchFailureSource,
    expected_last_captured_raw_input_sha256: Option<&[u8]>,
    kind: NetworkErrorKind,
    status_code: Option<u16>,
    retry_after: Option<std::time::Duration>,
) {
    record_transport_fetch_error(kind, status_code, retry_after);
    let _ = SCRAPE_OBSERVATION.try_with(|state| {
        state.borrow_mut().fetch_failure = Some(FetchFailureContext {
            url: url.clone(),
            kind,
            status_code: status_code.or(match kind {
                NetworkErrorKind::HttpStatus(status) => Some(status),
                _ => None,
            }),
            retry_after,
            source,
            expected_last_captured_raw_input_sha256: expected_last_captured_raw_input_sha256
                .map(ToOwned::to_owned),
        });
    });
}

/// Converts one fetch result into an independent transport observation.
///
/// HTTP responses outside the circuit-opening policy are responsive even when
/// they are non-2xx. Connection, DNS, timeout, unsafe-target, and unknown
/// failures do not prove that the remote domain responded.
pub(crate) fn domain_health_for_fetch_error(
    network_kind: NetworkErrorKind,
    status_code: Option<u16>,
    retry_after: Option<std::time::Duration>,
) -> DomainFetchHealth {
    let status_code = status_code.or(match network_kind {
        NetworkErrorKind::HttpStatus(status) => Some(status),
        _ => None,
    });
    match domain_failure_kind(network_kind) {
        Some(kind) => DomainFetchHealth::CircuitFailure {
            kind,
            status_code,
            retry_after,
        },
        None if matches!(network_kind, NetworkErrorKind::HttpStatus(_)) => {
            DomainFetchHealth::Responsive
        }
        None => DomainFetchHealth::NotObserved,
    }
}

pub(crate) fn domain_health_for_scraper_error(error: &ScraperError) -> DomainFetchHealth {
    match error {
        ScraperError::HttpError { kind, .. } => match domain_failure_kind(*kind) {
            Some(kind) => DomainFetchHealth::CircuitFailure {
                kind,
                status_code: match kind {
                    DomainFailureKind::Http408 => Some(408),
                    DomainFailureKind::Http429 => Some(429),
                    DomainFailureKind::Http503 => Some(503),
                    DomainFailureKind::Http504 => Some(504),
                    _ => None,
                },
                retry_after: None,
            },
            None => DomainFetchHealth::Responsive,
        },
        ScraperError::HttpErrorWithMetadata(metadata) => match domain_failure_kind(metadata.kind) {
            Some(kind) => DomainFetchHealth::CircuitFailure {
                kind,
                status_code: metadata.status_code,
                retry_after: metadata.retry_after,
            },
            None => DomainFetchHealth::Responsive,
        },
        ScraperError::ProductListingRemoved { .. }
        | ScraperError::NotProductPage { .. }
        | ScraperError::SchemaClassificationRejected { .. }
        | ScraperError::SchemaServiceError(_)
        | ScraperError::RemovedPageSchemaDatabaseError(_)
        | ScraperError::SchemaRegenerationExhausted { .. }
        | ScraperError::FreshSchemaNormalizationFailed { .. }
        | ScraperError::NormalizationError(_)
        | ScraperError::RawNormalizationInput(_)
        | ScraperError::SchemaFingerprint(_)
        | ScraperError::LlmBudgetExceeded { .. } => DomainFetchHealth::Responsive,
        ScraperError::NoHost { .. } | ScraperError::PendingSchemaReview { .. } => {
            DomainFetchHealth::NotObserved
        }
    }
}

// ---------------------------------------------------------------------------
// ScraperService trait
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
#[mockall::automock]
pub trait ScraperService: Send + Sync {
    /// Fetch the product page at `url`, extract structured data using the CSS
    /// selector schema for `listing_source_id`, validate a plausible extraction, and return a
    /// [`ScrapedProduct`]. The caller captures it before calling
    /// [`crate::scraper::candidate_service::ScraperCandidateService::mark_as_scraped`].
    async fn scrape(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        product_url_pattern: Option<&str>,
        last_scraped_hash: Option<&str>,
        last_scraped_schema_fingerprint: Option<&str>,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<Option<ScrapedProduct>, ScraperError>;

    #[allow(clippy::too_many_arguments)]
    async fn scrape_with_fallback_currency(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        product_url_pattern: Option<&str>,
        last_scraped_hash: Option<&str>,
        last_scraped_schema_fingerprint: Option<&str>,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
        fallback_currency: Option<Currency>,
    ) -> Result<Option<ScrapedProduct>, ScraperError> {
        let _ = fallback_currency;
        self.scrape(
            listing_source_id,
            url,
            product_url_pattern,
            last_scraped_hash,
            last_scraped_schema_fingerprint,
            expected_last_captured_raw_input_sha256,
        )
        .await
    }

    /// Executes one scrape while reporting the independent transport health
    /// observation used by the domain circuit. The default keeps existing
    /// implementations and test doubles compatible.
    async fn scrape_with_mode(&self, request: ScrapeRequest) -> ScrapeOutcome {
        let result = self
            .scrape_with_fallback_currency(
                &request.listing_source_id,
                &request.url,
                request.product_url_pattern.as_deref(),
                request.last_scraped_hash.as_deref(),
                request.last_scraped_schema_fingerprint.as_deref(),
                request.expected_last_captured_raw_input_sha256.as_deref(),
                request.fallback_currency,
            )
            .await;
        let domain_health = match &result {
            Ok(_) => DomainFetchHealth::Responsive,
            Err(error) => domain_health_for_scraper_error(error),
        };
        ScrapeOutcome {
            result,
            domain_health,
            fetch_failure: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responsive_http_statuses_prove_domain_health() {
        for status in [404, 410, 403, 425, 500, 502] {
            assert_eq!(
                domain_health_for_fetch_error(
                    NetworkErrorKind::HttpStatus(status),
                    Some(status),
                    None,
                ),
                DomainFetchHealth::Responsive,
                "HTTP {status} should remain responsive"
            );
        }
    }

    #[test]
    fn only_policy_network_failures_open_domain_circuit() {
        assert!(matches!(
            domain_health_for_fetch_error(NetworkErrorKind::HttpStatus(429), Some(429), None),
            DomainFetchHealth::CircuitFailure {
                kind: DomainFailureKind::Http429,
                ..
            }
        ));
        assert_eq!(
            domain_health_for_fetch_error(NetworkErrorKind::Request, None, None),
            DomainFetchHealth::NotObserved
        );
    }

    #[tokio::test]
    async fn pre_fetch_application_error_stays_not_observed() {
        let (_, observation) = with_scrape_observation(CrawlerDomainId::new(), async {
            begin_transport_observation();
            Err::<(), _>("schema lookup failed")
        })
        .await;

        assert_eq!(observation, Some(DomainFetchHealth::NotObserved));
    }

    #[tokio::test]
    async fn downstream_error_cannot_erase_responsive_primary_fetch() {
        let (_, observation) = with_scrape_observation(CrawlerDomainId::new(), async {
            begin_transport_observation();
            record_transport_success();
            Err::<(), _>("pending schema review")
        })
        .await;

        assert_eq!(observation, Some(DomainFetchHealth::Responsive));
    }

    #[tokio::test]
    async fn probe_transport_failure_is_recorded_independently_of_final_error() {
        let (_, observation) = with_scrape_observation(CrawlerDomainId::new(), async {
            begin_transport_observation();
            record_transport_fetch_error(NetworkErrorKind::Timeout, None, None);
            Err::<(), _>("timeout")
        })
        .await;

        assert!(matches!(
            observation,
            Some(DomainFetchHealth::CircuitFailure {
                kind: DomainFailureKind::Timeout,
                ..
            })
        ));
    }
}
