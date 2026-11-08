use crate::network::policy::{DomainFailureKind, NetworkErrorKind, domain_failure_kind};
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::html_fetcher::DomainFetchStats;
use crate::spider::classification::product_url_pattern::ProductUrlPattern;
use listing_source_core::ListingSourceId;
use product_listing_normalization::ProductListingNormalizationInput;
use std::cell::RefCell;
use std::future::Future;
use url::Url;

/// Result of a successful scrape — the raw normalization input together with
/// metadata needed to mark the URL as scraped after durable raw capture.
#[derive(Debug)]
pub struct ScrapedProduct {
    /// Successful page acquisition, before schema evaluation or normalization.
    pub acquired_at: time::OffsetDateTime,
    /// Complete source-neutral normalization input. The worker later performs canonical writes.
    pub raw_input: ProductListingNormalizationInput,
    pub effective_url: Url,
    pub source_listing_id: String,
    /// The title the raw input captures: the extracted one, or the one carried forward from the
    /// last capture when the page left its title blank.
    pub title: String,
    /// SHA-256 of what the effective ordered schema set extracts from the page, which the
    /// next scrape compares to skip an unchanged page.
    pub hash: String,
    /// Deterministic fingerprint of the extractor version, the effective ordered schema set and
    /// the fallback currency the extraction ran with.
    pub schema_fingerprint: String,
    /// Shared provider-neutral raw-input hash used for local change detection.
    pub raw_input_sha256: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrapeMode {
    Normal,
    CircuitProbe,
    /// Explicitly keeps the legacy direct API on the primary page only. This
    /// mode never samples persisted sibling URLs for schema generation.
    PrimaryOnly,
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
    /// The fetcher's view of the host when the failure may open the scrape circuit.
    pub fetch_stats: Option<DomainFetchStats>,
}

/// All inputs required to execute one scraper request. Keeping the request as
/// one value also makes the probe boundary explicit and prevents the mode
/// call from growing another positional argument.
#[derive(Debug, Clone)]
pub struct ScrapeRequest {
    pub listing_source_id: ListingSourceId,
    pub url: Url,
    pub source_record_key: String,
    /// Source listing id captured by the last successful scrape of this URL, when
    /// known. The removal guard only accepts product evidence under this identity.
    pub last_source_listing_id: Option<String>,
    /// Title captured by the last successful scrape of this URL, when known. It stands in for
    /// a title the page later leaves blank, so the change the page still shows is captured.
    pub last_captured_title: Option<String>,
    pub product_url_pattern: Option<ProductUrlPattern>,
    pub last_scraped_hash: Option<String>,
    pub last_scraped_schema_fingerprint: Option<String>,
    pub expected_last_captured_raw_input_sha256: Option<Vec<u8>>,
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
    pub scrape_health: DomainFetchHealth,
    pub fetch_failure: Option<FetchFailureContext>,
    pub telemetry: ScrapeTelemetry,
}

/// What one scrape did, for the URL's outcome log: the product page's answer and timings, the
/// schema that was captured and the work the scrape spent on LLM calls and image probes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScrapeTelemetry {
    /// The status the shop answered the product page with, when it answered.
    pub http_status: Option<u16>,
    pub fetch_ms: Option<u64>,
    /// Everything of the scrape that is no product-page fetch.
    pub process_ms: Option<u64>,
    /// The index of the captured schema in the ListingSource's ordered schema set; a fresh
    /// schema takes the next index.
    pub schema_index: Option<usize>,
    /// The ranking score of a cached schema's extraction; absent for a fresh schema.
    pub schema_score: Option<usize>,
    pub fresh_schema: bool,
    /// How many of the product's extracted fields the captured extraction filled.
    pub fields_present: Option<usize>,
    pub image_probes: u32,
    pub llm_calls: u32,
}

impl ScrapeOutcome {
    #[cfg(test)]
    pub(crate) fn unwrap(self) -> Option<ScrapedProduct> {
        self.result.unwrap()
    }

    #[cfg(test)]
    pub(crate) fn unwrap_err(self) -> ScraperError {
        self.result.unwrap_err()
    }

    #[cfg(test)]
    pub(crate) fn expect_err(self, message: &str) -> ScraperError {
        self.result.expect_err(message)
    }

    #[cfg(test)]
    pub(crate) fn is_ok(&self) -> bool {
        self.result.is_ok()
    }
}

struct ScrapeObservationState {
    transport_health: Option<DomainFetchHealth>,
    fetch_failure: Option<FetchFailureContext>,
    telemetry: ScrapeTelemetry,
}

tokio::task_local! {
    static SCRAPE_OBSERVATION: RefCell<ScrapeObservationState>;
}

/// Runs a scrape and returns the transport observation collected by the
/// primary/seed fetches. Execution configuration lives in `ScrapeRequest`; this
/// scoped state is only for nested transport observations.
pub(crate) async fn with_scrape_observation<F, T>(future: F) -> (T, Option<DomainFetchHealth>)
where
    F: Future<Output = T>,
{
    SCRAPE_OBSERVATION
        .scope(
            RefCell::new(ScrapeObservationState {
                transport_health: None,
                fetch_failure: None,
                telemetry: ScrapeTelemetry::default(),
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

/// The telemetry the running scrape collected so far; empty outside a scrape.
pub(crate) fn current_scrape_telemetry() -> ScrapeTelemetry {
    SCRAPE_OBSERVATION
        .try_with(|state| state.borrow().telemetry.clone())
        .unwrap_or_default()
}

fn update_scrape_telemetry(update: impl FnOnce(&mut ScrapeTelemetry)) {
    let _ = SCRAPE_OBSERVATION.try_with(|state| update(&mut state.borrow_mut().telemetry));
}

pub(crate) fn record_product_page_fetch(status: Option<u16>, elapsed: std::time::Duration) {
    update_scrape_telemetry(|telemetry| {
        telemetry.http_status = status;
        telemetry.fetch_ms = Some(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX));
    });
}

pub(crate) fn record_captured_schema(
    schema_index: usize,
    schema_score: Option<usize>,
    fresh_schema: bool,
    fields_present: usize,
) {
    update_scrape_telemetry(|telemetry| {
        telemetry.schema_index = Some(schema_index);
        telemetry.schema_score = schema_score;
        telemetry.fresh_schema = fresh_schema;
        telemetry.fields_present = Some(fields_present);
    });
}

pub(crate) fn count_image_probe() {
    update_scrape_telemetry(|telemetry| telemetry.image_probes += 1);
}

pub(crate) fn count_llm_call() {
    update_scrape_telemetry(|telemetry| telemetry.llm_calls += 1);
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
    record_transport_observation(scrape_health_for_fetch_error(
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
    fetch_stats: Option<DomainFetchStats>,
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
            fetch_stats,
        });
    });
}

/// Converts one fetch result into an independent transport observation.
///
/// HTTP responses outside the circuit-opening policy are responsive even when
/// they are non-2xx, and so is a bot challenge the shop answered with.
/// Connection, DNS, timeout, unsafe-target, and unknown failures do not prove
/// that the remote domain responded.
pub(crate) fn scrape_health_for_fetch_error(
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
        None if matches!(
            network_kind,
            NetworkErrorKind::HttpStatus(_) | NetworkErrorKind::BotChallenge
        ) =>
        {
            DomainFetchHealth::Responsive
        }
        None => DomainFetchHealth::NotObserved,
    }
}

pub(crate) fn scrape_health_for_scraper_error(error: &ScraperError) -> DomainFetchHealth {
    match error {
        // A robots.txt refusal sent nothing to the shop.
        error
            if error
                .network_error_kind()
                .is_some_and(NetworkErrorKind::is_robots_refusal) =>
        {
            DomainFetchHealth::NotObserved
        }
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
        ScraperError::ProductListingRemoved(_)
        | ScraperError::NotProductPage { .. }
        | ScraperError::RedirectToKnownProduct { .. }
        | ScraperError::RedirectDestinationLookup(_)
        | ScraperError::RedirectDestinationUnsettled { .. }
        | ScraperError::RedirectTargetUnverified { .. }
        | ScraperError::SchemaClassificationRejected { .. }
        | ScraperError::SchemaServiceError(_)
        | ScraperError::RemovedPageSchemaDatabaseError(_)
        | ScraperError::SchemaRegenerationExhausted { .. }
        | ScraperError::FreshSchemaImplausible { .. }
        | ScraperError::MissingRequiredValue { .. }
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
    /// Executes one scrape request and returns both the product result and the
    /// transport observation used by the scrape circuit.
    async fn scrape(&self, request: ScrapeRequest) -> ScrapeOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responsive_http_statuses_prove_domain_health() {
        for status in [404, 410, 403, 425, 500, 502] {
            assert_eq!(
                scrape_health_for_fetch_error(
                    NetworkErrorKind::HttpStatus(status),
                    Some(status),
                    None,
                ),
                DomainFetchHealth::Responsive,
                "HTTP {status} should remain responsive"
            );
        }
    }

    /// A challenge is the shop answering, so it closes a probed circuit and counts towards the
    /// status burst like any answer that rejects the crawler.
    #[test]
    fn a_bot_challenge_proves_domain_health() {
        for status in [200, 202, 403] {
            assert_eq!(
                scrape_health_for_fetch_error(NetworkErrorKind::BotChallenge, Some(status), None),
                DomainFetchHealth::Responsive,
                "a challenge answered with {status} should be responsive"
            );
        }
    }

    #[test]
    fn only_policy_network_failures_open_scrape_circuit() {
        assert!(matches!(
            scrape_health_for_fetch_error(NetworkErrorKind::HttpStatus(429), Some(429), None),
            DomainFetchHealth::CircuitFailure {
                kind: DomainFailureKind::Http429,
                ..
            }
        ));
        assert_eq!(
            scrape_health_for_fetch_error(NetworkErrorKind::Request, None, None),
            DomainFetchHealth::NotObserved
        );
    }

    #[tokio::test]
    async fn pre_fetch_application_error_stays_not_observed() {
        let (_, observation) = with_scrape_observation(async {
            begin_transport_observation();
            Err::<(), _>("schema lookup failed")
        })
        .await;

        assert_eq!(observation, Some(DomainFetchHealth::NotObserved));
    }

    #[tokio::test]
    async fn downstream_error_cannot_erase_responsive_primary_fetch() {
        let (_, observation) = with_scrape_observation(async {
            begin_transport_observation();
            record_transport_success();
            Err::<(), _>("pending schema review")
        })
        .await;

        assert_eq!(observation, Some(DomainFetchHealth::Responsive));
    }

    #[tokio::test]
    async fn probe_transport_failure_is_recorded_independently_of_final_error() {
        let (_, observation) = with_scrape_observation(async {
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
