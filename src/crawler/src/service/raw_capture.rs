//! Crawler raw ProductListing capture boundary.
//!
//! The crawler retains source-specific extraction mapping, but only writes changed immutable raw
//! revisions. Canonical ProductListing normalization and events are worker-owned.

use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use listing_source_core::ListingSourceId;
use product_listing_ingestion_sqs::codec::{self, MAX_MESSAGE_BYTES};
use product_listing_normalization::{
    ProductListingNormalizationInput, RawProductListingProvenance,
};
use product_listing_service::ports::ProductListingRawIngestionMethod;
use product_listing_service::use_cases::{
    CaptureProductListingRawObservationCommand, IndexedProductListingIngestionIntent,
    ProductListingIngestionActor, ProductListingIngestionCommandId, ProductListingIngestionIntent,
    ProductListingIngestionItemOutcome, ProductListingIngestionMessage,
    ProductListingIngestionMetadata, ProductListingIngestionOperation,
    ProductListingIngestionOutcome, ProductListingIngestionRejectionReason,
    ProductListingIngestionSubmission, ProductListingIngestionSubmissionId,
    SubmitInternalProductListingIngestionUseCase,
};
use std::{collections::HashMap, sync::Arc};
use tracing::{debug, warn};

/// One crawler observation prepared for durable raw capture.
#[derive(Debug, Clone)]
pub struct ProductListingRawCaptureItem {
    pub command: CaptureProductListingRawObservationCommand,
}

impl ProductListingRawCaptureItem {
    pub fn acquired_at(mut self, acquired_at: time::OffsetDateTime) -> Self {
        self.command.source_occurred_at = Some(acquired_at);
        self
    }
    pub fn crawler(
        listing_source_id: ListingSourceId,
        source_record_key: &str,
        input: ProductListingNormalizationInput,
        provenance: RawProductListingProvenance,
    ) -> Self {
        Self {
            command: CaptureProductListingRawObservationCommand {
                listing_source_id,
                ingestion_method: ProductListingRawIngestionMethod::WebCrawl,
                // The logical crawler source identity is stable across URL moves. The input
                // itself still carries the effective URL used for extraction and canonical state.
                source_record_key: source_record_key.to_owned(),
                input,
                provenance,
                source_event_id: None,
                source_occurred_at: None,
                provider_receipt: None,
            },
        }
    }
}

/// Crawler-local disposition of one capture attempt.
///
/// `Persisted` means the ingestion queue accepted the observation; the consumer captures it
/// transactionally afterwards. `DiscardedUnsendable` is terminal: the observation cannot be
/// encoded within the queue's message limit, so retrying the same page can never succeed.
/// `DiscardedMissingSource` is terminal for an observation whose ListingSource no longer exists.
/// Neither discard claims that raw business data was persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProductListingRawCaptureOutcome {
    Persisted,
    DiscardedMissingSource,
    DiscardedUnsendable,
    #[default]
    RetryableFailure,
}

/// Captures input positions independently and reports accepted, terminal, and retryable outcomes.
/// Results retain input order. Observations of one ListingSource are submitted in input order, so
/// each raw stream keeps its enqueue order; only independent ListingSources run concurrently.
#[async_trait]
#[mockall::automock]
pub trait ProductListingRawCaptureService: Send + Sync {
    async fn capture(
        &self,
        observations: Vec<ProductListingRawCaptureItem>,
    ) -> Vec<ProductListingRawCaptureOutcome>;
}

/// Largest number of observations in one source-scoped ingestion submission.
const MAX_SUBMISSION_ITEMS: usize = 100;

/// Submits raw observations to the ingestion queue through the internal submission use case.
pub struct ProductListingRawCaptureServiceImpl {
    submit: Arc<dyn SubmitInternalProductListingIngestionUseCase>,
    max_concurrent_sources: usize,
}

impl ProductListingRawCaptureServiceImpl {
    pub fn new(
        submit: Arc<dyn SubmitInternalProductListingIngestionUseCase>,
        max_concurrent_sources: usize,
    ) -> Self {
        Self {
            submit,
            max_concurrent_sources: max_concurrent_sources.max(1),
        }
    }

    /// Submits one ListingSource's observations chunk by chunk, so no two chunks of a raw stream
    /// are in flight together.
    async fn submit_source(
        &self,
        listing_source_id: ListingSourceId,
        items: Vec<(usize, ProductListingRawCaptureItem)>,
    ) -> Vec<(usize, ProductListingRawCaptureOutcome)> {
        let mut outcomes = Vec::with_capacity(items.len());
        let mut items = items.into_iter().peekable();
        while items.peek().is_some() {
            let chunk: Vec<_> = items.by_ref().take(MAX_SUBMISSION_ITEMS).collect();
            outcomes.extend(self.submit_chunk(listing_source_id, chunk).await);
        }
        outcomes
    }

    async fn submit_chunk(
        &self,
        listing_source_id: ListingSourceId,
        chunk: Vec<(usize, ProductListingRawCaptureItem)>,
    ) -> Vec<(usize, ProductListingRawCaptureOutcome)> {
        let context = crawler_operation_context(listing_source_id);
        let input_indices: Vec<usize> = chunk.iter().map(|(input_index, _)| *input_index).collect();
        let submission = ProductListingIngestionSubmission {
            listing_source_id,
            original_input_count: chunk.len(),
            idempotency_key: None,
            items: chunk
                .into_iter()
                .enumerate()
                .map(|(index, (_, item))| IndexedProductListingIngestionIntent {
                    index,
                    intent: ProductListingIngestionIntent::CaptureRaw(item.command),
                })
                .collect(),
        };

        let mut by_index = match self.submit.execute(&context, submission).await {
            Ok(result) => outcomes_by_index(result.items),
            Err(error) => {
                warn!(
                    error = %error,
                    listing_source_id = %listing_source_id,
                    request_id = %context.request_id,
                    correlation_id = %context.correlation_id,
                    "Crawler raw ProductListing submission failed; observations remain retryable"
                );
                HashMap::new()
            }
        };
        input_indices
            .into_iter()
            .enumerate()
            .map(|(index, input_index)| {
                let outcome = by_index
                    .remove(&index)
                    .flatten()
                    .map(|outcome| map_item_outcome(&outcome))
                    .unwrap_or_default();
                (input_index, outcome)
            })
            .collect()
    }
}

/// Indexes item outcomes by submission position. A position reported twice is contradictory and
/// maps to `None`, which callers treat as retryable.
fn outcomes_by_index(
    items: Vec<ProductListingIngestionItemOutcome>,
) -> HashMap<usize, Option<ProductListingIngestionOutcome>> {
    let mut by_index = HashMap::with_capacity(items.len());
    for item in items {
        by_index
            .entry(item.index)
            .and_modify(|outcome| *outcome = None)
            .or_insert(Some(item.outcome));
    }
    by_index
}

/// Only confirmed queue acceptance counts as persisted. A publisher that could not send the
/// message at all because of its size or shape is terminal; everything else may succeed later.
fn map_item_outcome(outcome: &ProductListingIngestionOutcome) -> ProductListingRawCaptureOutcome {
    match outcome {
        ProductListingIngestionOutcome::Accepted => ProductListingRawCaptureOutcome::Persisted,
        ProductListingIngestionOutcome::Rejected {
            reason: ProductListingIngestionRejectionReason::Publisher { code },
            ..
        } if UNSENDABLE_PUBLISHER_CODES.contains(&code.as_str()) => {
            ProductListingRawCaptureOutcome::DiscardedUnsendable
        }
        ProductListingIngestionOutcome::Rejected { .. }
        | ProductListingIngestionOutcome::Unconfirmed
        | ProductListingIngestionOutcome::NotAttempted { .. } => {
            ProductListingRawCaptureOutcome::RetryableFailure
        }
    }
}

/// Publisher rejection codes for a message that cannot be encoded or does not fit the queue.
const UNSENDABLE_PUBLISHER_CODES: [&str; 2] = ["INVALID_MESSAGE_SIZE", "ENCODING_FAILED"];

/// Encoded queue-message size of an observation, measured as the publisher will encode it.
/// Placeholder identities have the fixed length of the real ones and the index fields are sized
/// for the largest submission position, so the result never undercounts the real message.
fn measure_encoded_bytes(
    command: CaptureProductListingRawObservationCommand,
    context: &OperationContext,
) -> (
    CaptureProductListingRawObservationCommand,
    Result<usize, codec::CodecError>,
) {
    let placeholder_digest = "0".repeat(64);
    let metadata = ProductListingIngestionMetadata {
        submission_id: ProductListingIngestionSubmissionId::from_wire(&format!(
            "plis1_{placeholder_digest}"
        ))
        .expect("placeholder submission id is canonical"),
        command_id: ProductListingIngestionCommandId::from_wire(&format!(
            "plic1_{placeholder_digest}"
        ))
        .expect("placeholder command id is canonical"),
        index: MAX_SUBMISSION_ITEMS - 1,
        input_count: MAX_SUBMISSION_ITEMS,
        listing_source_id: command.listing_source_id,
        operation: ProductListingIngestionOperation::CaptureRaw,
        actor: ProductListingIngestionActor::Service(CRAWLER_SERVICE_ID.to_owned()),
        request_id: context.request_id.clone(),
        correlation_id: context.correlation_id.clone(),
    };
    let message = ProductListingIngestionMessage {
        metadata,
        intent: ProductListingIngestionIntent::CaptureRaw(command),
    };
    let encoded = codec::encoded_len(&message);
    let ProductListingIngestionIntent::CaptureRaw(command) = message.intent else {
        unreachable!("the measured message carries the capture intent it was built with");
    };
    (command, encoded)
}

/// Logs a terminal discard with its identifying metadata, never the payload.
fn warn_discarded_unsendable(
    listing_source_id: ListingSourceId,
    url: &str,
    encoded: Result<usize, codec::CodecError>,
) {
    match encoded {
        Ok(encoded_bytes) => warn!(
            event = "crawler.raw_capture.discarded_unsendable",
            listing_source_id = %listing_source_id,
            url,
            encoded_bytes,
            max_message_bytes = MAX_MESSAGE_BYTES,
            "Discarded crawler raw observation that exceeds the ingestion message limit"
        ),
        Err(error) => warn!(
            event = "crawler.raw_capture.discarded_unsendable",
            listing_source_id = %listing_source_id,
            url,
            error = %error,
            "Discarded crawler raw observation that cannot be encoded for the ingestion queue"
        ),
    }
}

#[async_trait]
impl ProductListingRawCaptureService for ProductListingRawCaptureServiceImpl {
    #[tracing::instrument(
        name = "crawler_raw_capture_batch",
        skip(self, observations),
        fields(total = observations.len())
    )]
    async fn capture(
        &self,
        observations: Vec<ProductListingRawCaptureItem>,
    ) -> Vec<ProductListingRawCaptureOutcome> {
        let mut results = vec![ProductListingRawCaptureOutcome::default(); observations.len()];
        let mut sources =
            Vec::<(ListingSourceId, Vec<(usize, ProductListingRawCaptureItem)>)>::new();
        let mut source_positions = HashMap::<ListingSourceId, usize>::new();

        for (input_index, mut observation) in observations.into_iter().enumerate() {
            let listing_source_id = observation.command.listing_source_id;
            let context = crawler_operation_context(listing_source_id);
            let (command, encoded) = measure_encoded_bytes(observation.command, &context);
            observation.command = command;
            if !matches!(encoded, Ok(bytes) if bytes <= MAX_MESSAGE_BYTES) {
                warn_discarded_unsendable(
                    listing_source_id,
                    &observation.command.source_record_key,
                    encoded,
                );
                results[input_index] = ProductListingRawCaptureOutcome::DiscardedUnsendable;
                continue;
            }
            let position = *source_positions
                .entry(listing_source_id)
                .or_insert_with(|| {
                    sources.push((listing_source_id, Vec::new()));
                    sources.len() - 1
                });
            sources[position].1.push((input_index, observation));
        }

        let source_outcomes = stream::iter(
            sources
                .into_iter()
                .map(|(listing_source_id, items)| self.submit_source(listing_source_id, items)),
        )
        .buffer_unordered(self.max_concurrent_sources)
        .collect::<Vec<_>>()
        .await;

        for (input_index, outcome) in source_outcomes.into_iter().flatten() {
            results[input_index] = outcome;
        }
        results
    }
}

const CRAWLER_SERVICE_ID: &str = "crawler";

fn crawler_operation_context(listing_source_id: ListingSourceId) -> OperationContext {
    let key = format!("crawler-raw-capture:{listing_source_id}");
    OperationContext {
        principal: Principal::Service(CRAWLER_SERVICE_ID.to_owned()),
        request_id: RequestId::new(key.clone()),
        correlation_id: CorrelationId::new(key),
    }
}

/// Writes display-only raw-capture snapshots. The file is never a replay source.
pub struct FileProductListingRawCaptureService {
    output_path: std::path::PathBuf,
}

impl FileProductListingRawCaptureService {
    pub fn new(output_path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            output_path: output_path.into(),
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RawCaptureSnapshot {
    listing_source_id: String,
    ingestion_method: String,
    source_record_key: String,
    action: String,
    payload_format: String,
    payload_schema_version: u16,
    raw_values_schema_version: u16,
    source_payload: serde_json::Value,
    raw_values: serde_json::Value,
    normalization_context: serde_json::Value,
    provenance: serde_json::Value,
}

impl From<&ProductListingRawCaptureItem> for RawCaptureSnapshot {
    fn from(item: &ProductListingRawCaptureItem) -> Self {
        let command = &item.command;
        Self {
            listing_source_id: command.listing_source_id.to_string(),
            ingestion_method: command.ingestion_method.as_str().to_owned(),
            source_record_key: command.source_record_key.clone(),
            action: command.input.operation().as_str().to_owned(),
            payload_format: command.input.payload_format().as_str().to_owned(),
            payload_schema_version: command.input.payload_schema_version(),
            raw_values_schema_version: command.input.raw_values_schema_version(),
            source_payload: command.input.source_payload().value().clone(),
            raw_values: command.input.raw_values().value().clone(),
            normalization_context: command.input.normalization_context().value().clone(),
            provenance: command.provenance.value().clone(),
        }
    }
}

#[async_trait]
impl ProductListingRawCaptureService for FileProductListingRawCaptureService {
    async fn capture(
        &self,
        observations: Vec<ProductListingRawCaptureItem>,
    ) -> Vec<ProductListingRawCaptureOutcome> {
        if observations.is_empty() {
            return Vec::new();
        }

        let mut snapshots: Vec<serde_json::Value> = if self.output_path.exists() {
            match std::fs::read_to_string(&self.output_path)
                .ok()
                .and_then(|content| serde_json::from_str(&content).ok())
            {
                Some(snapshots) => snapshots,
                None => {
                    return vec![
                        ProductListingRawCaptureOutcome::RetryableFailure;
                        observations.len()
                    ];
                }
            }
        } else {
            Vec::new()
        };
        let new_snapshots = match observations
            .iter()
            .map(RawCaptureSnapshot::from)
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(snapshots) => snapshots,
            Err(_) => {
                return vec![ProductListingRawCaptureOutcome::RetryableFailure; observations.len()];
            }
        };
        snapshots.extend(new_snapshots);
        let json = match serde_json::to_string_pretty(&snapshots) {
            Ok(json) => json,
            Err(_) => {
                return vec![ProductListingRawCaptureOutcome::RetryableFailure; observations.len()];
            }
        };
        match std::fs::write(&self.output_path, json) {
            Ok(()) => {
                debug!(count = observations.len(), path = %self.output_path.display(), "Wrote raw capture snapshots to file");
                vec![ProductListingRawCaptureOutcome::Persisted; observations.len()]
            }
            Err(error) => {
                warn!(error = %error, path = %self.output_path.display(), "Failed to write raw capture snapshots");
                vec![ProductListingRawCaptureOutcome::RetryableFailure; observations.len()]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use product_listing_normalization::{
        MAX_SOURCE_PAYLOAD_JSON_BYTES, NormalizationContext, RawProductListingOperation,
        RawProductListingPayloadFormat, RawProductListingValues, SourcePayload,
    };
    use product_listing_service::use_cases::{
        ProductListingIngestionIdempotencyKey, ProductListingIngestionNotAttemptedReason,
        ProductListingIngestionSubmissionError, ProductListingIngestionSubmissionResult,
    };
    use std::sync::Mutex;
    use url::Url;

    /// Answers every submission from a script and records what it was asked to submit.
    struct FakeSubmitUseCase {
        submissions: Mutex<Vec<ProductListingIngestionSubmission>>,
        respond: Responder,
    }

    type Responder = Box<dyn Fn(usize, &ProductListingIngestionSubmission) -> Reply + Send + Sync>;

    enum Reply {
        Outcomes(Vec<(usize, ProductListingIngestionOutcome)>),
        Failure,
    }

    impl FakeSubmitUseCase {
        fn accepting_all() -> Self {
            Self::responding(|_, submission| {
                Reply::Outcomes(
                    submission
                        .items
                        .iter()
                        .map(|item| (item.index, ProductListingIngestionOutcome::Accepted))
                        .collect(),
                )
            })
        }

        fn responding(
            respond: impl Fn(usize, &ProductListingIngestionSubmission) -> Reply + Send + Sync + 'static,
        ) -> Self {
            Self {
                submissions: Mutex::default(),
                respond: Box::new(respond),
            }
        }

        fn submissions(&self) -> Vec<ProductListingIngestionSubmission> {
            self.submissions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }
    }

    #[async_trait]
    impl SubmitInternalProductListingIngestionUseCase for FakeSubmitUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            submission: ProductListingIngestionSubmission,
        ) -> Result<ProductListingIngestionSubmissionResult, ProductListingIngestionSubmissionError>
        {
            assert_eq!(
                Principal::Service("crawler".to_owned()),
                context.principal,
                "the crawler submits as its own service principal"
            );
            let call = {
                let mut submissions = self
                    .submissions
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                submissions.push(submission.clone());
                submissions.len() - 1
            };
            match (self.respond)(call, &submission) {
                Reply::Failure => Err(ProductListingIngestionSubmissionError::Forbidden),
                Reply::Outcomes(outcomes) => Ok(ProductListingIngestionSubmissionResult {
                    submission_id: ProductListingIngestionSubmissionId::from_wire(&format!(
                        "plis1_{}",
                        "0".repeat(64)
                    ))
                    .expect("canonical submission id"),
                    idempotency_key: ProductListingIngestionIdempotencyKey::new("test-key")
                        .expect("valid key"),
                    original_input_count: submission.original_input_count,
                    items: outcomes
                        .into_iter()
                        .map(|(index, outcome)| ProductListingIngestionItemOutcome {
                            index,
                            command_id: ProductListingIngestionCommandId::from_wire(&format!(
                                "plic1_{}",
                                "0".repeat(64)
                            ))
                            .expect("canonical command id"),
                            outcome,
                        })
                        .collect(),
                }),
            }
        }
    }

    fn item_with_payload(
        listing_source_id: ListingSourceId,
        url: &str,
        payload: serde_json::Value,
    ) -> ProductListingRawCaptureItem {
        let url = Url::parse(url).unwrap_or_else(|error| panic!("test URL: {error}"));
        let input = ProductListingNormalizationInput::new(
            RawProductListingOperation::Upsert,
            RawProductListingPayloadFormat::CrawlerExtractedProduct,
            1,
            1,
            SourcePayload::new(payload).unwrap_or_else(|error| panic!("payload: {error}")),
            RawProductListingValues::new(serde_json::json!({}))
                .unwrap_or_else(|error| panic!("values: {error}")),
            NormalizationContext::new(serde_json::json!({}))
                .unwrap_or_else(|error| panic!("context: {error}")),
        )
        .unwrap_or_else(|error| panic!("input: {error}"));
        let provenance = RawProductListingProvenance::new(serde_json::json!({}))
            .unwrap_or_else(|error| panic!("provenance: {error}"));
        ProductListingRawCaptureItem::crawler(listing_source_id, url.as_str(), input, provenance)
    }

    fn item(listing_source_id: ListingSourceId, url: &str) -> ProductListingRawCaptureItem {
        item_with_payload(
            listing_source_id,
            url,
            serde_json::json!({"title": "Chair"}),
        )
    }

    fn service(use_case: &Arc<FakeSubmitUseCase>) -> ProductListingRawCaptureServiceImpl {
        ProductListingRawCaptureServiceImpl::new(
            Arc::clone(use_case) as Arc<dyn SubmitInternalProductListingIngestionUseCase>,
            2,
        )
    }

    fn keys(submission: &ProductListingIngestionSubmission) -> Vec<String> {
        submission
            .items
            .iter()
            .map(|item| match &item.intent {
                ProductListingIngestionIntent::CaptureRaw(command) => {
                    command.source_record_key.clone()
                }
                other => panic!("unexpected intent {other:?}"),
            })
            .collect()
    }

    fn rejected_by_publisher(code: &str) -> ProductListingIngestionOutcome {
        ProductListingIngestionOutcome::Rejected {
            reason: ProductListingIngestionRejectionReason::Publisher {
                code: code.to_owned(),
            },
            retryable: false,
        }
    }

    #[tokio::test]
    async fn should_submit_web_crawl_capture_raw_intents_and_report_accepted_as_persisted() {
        let source = ListingSourceId::new();
        let use_case = Arc::new(FakeSubmitUseCase::accepting_all());

        let outcomes = service(&use_case)
            .capture(vec![
                item(source, "https://example.test/products/one"),
                item(source, "https://example.test/products/two"),
            ])
            .await;

        assert_eq!(
            vec![ProductListingRawCaptureOutcome::Persisted; 2],
            outcomes
        );
        let submissions = use_case.submissions();
        assert_eq!(1, submissions.len());
        assert_eq!(source, submissions[0].listing_source_id);
        assert_eq!(2, submissions[0].original_input_count);
        assert!(submissions[0].idempotency_key.is_none());
        assert_eq!(
            vec![
                "https://example.test/products/one",
                "https://example.test/products/two"
            ],
            keys(&submissions[0])
        );
        for item in &submissions[0].items {
            let ProductListingIngestionIntent::CaptureRaw(command) = &item.intent else {
                panic!("capture intent expected");
            };
            assert_eq!("WEB_CRAWL", command.ingestion_method.as_str());
        }
    }

    #[tokio::test]
    async fn should_map_every_non_accepted_outcome_to_its_crawler_disposition() {
        let cases = [
            (
                ProductListingIngestionOutcome::Accepted,
                ProductListingRawCaptureOutcome::Persisted,
            ),
            (
                rejected_by_publisher("INVALID_MESSAGE_SIZE"),
                ProductListingRawCaptureOutcome::DiscardedUnsendable,
            ),
            (
                rejected_by_publisher("ENCODING_FAILED"),
                ProductListingRawCaptureOutcome::DiscardedUnsendable,
            ),
            (
                rejected_by_publisher("SEND_FAILED"),
                ProductListingRawCaptureOutcome::RetryableFailure,
            ),
            (
                ProductListingIngestionOutcome::Rejected {
                    reason: ProductListingIngestionRejectionReason::InvalidNormalizationInput,
                    retryable: false,
                },
                ProductListingRawCaptureOutcome::RetryableFailure,
            ),
            (
                ProductListingIngestionOutcome::Unconfirmed,
                ProductListingRawCaptureOutcome::RetryableFailure,
            ),
            (
                ProductListingIngestionOutcome::NotAttempted {
                    reason: ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
                    retryable: true,
                },
                ProductListingRawCaptureOutcome::RetryableFailure,
            ),
        ];
        for (queue_outcome, expected) in cases {
            let use_case = Arc::new(FakeSubmitUseCase::responding({
                let queue_outcome = queue_outcome.clone();
                move |_, _| Reply::Outcomes(vec![(0, queue_outcome.clone())])
            }));

            let outcomes = service(&use_case)
                .capture(vec![item(
                    ListingSourceId::new(),
                    "https://example.test/p/1",
                )])
                .await;

            assert_eq!(vec![expected], outcomes, "queue outcome {queue_outcome:?}");
        }
    }

    #[tokio::test]
    async fn should_retry_items_with_missing_or_contradictory_outcomes() {
        let use_case = Arc::new(FakeSubmitUseCase::responding(|_, _| {
            Reply::Outcomes(vec![
                (0, ProductListingIngestionOutcome::Accepted),
                // Position 1 is reported twice; position 2 not at all.
                (1, ProductListingIngestionOutcome::Accepted),
                (1, ProductListingIngestionOutcome::Unconfirmed),
            ])
        }));
        let source = ListingSourceId::new();

        let outcomes = service(&use_case)
            .capture(vec![
                item(source, "https://example.test/p/1"),
                item(source, "https://example.test/p/2"),
                item(source, "https://example.test/p/3"),
            ])
            .await;

        assert_eq!(
            vec![
                ProductListingRawCaptureOutcome::Persisted,
                ProductListingRawCaptureOutcome::RetryableFailure,
                ProductListingRawCaptureOutcome::RetryableFailure,
            ],
            outcomes
        );
    }

    #[tokio::test]
    async fn should_retry_every_item_when_the_whole_submission_fails() {
        let use_case = Arc::new(FakeSubmitUseCase::responding(|_, _| Reply::Failure));
        let source = ListingSourceId::new();

        let outcomes = service(&use_case)
            .capture(vec![
                item(source, "https://example.test/p/1"),
                item(source, "https://example.test/p/2"),
            ])
            .await;

        assert_eq!(
            vec![ProductListingRawCaptureOutcome::RetryableFailure; 2],
            outcomes
        );
    }

    #[tokio::test]
    async fn should_submit_one_source_per_submission_keeping_input_order_and_positions() {
        let (first, second) = (ListingSourceId::new(), ListingSourceId::new());
        let use_case = Arc::new(FakeSubmitUseCase::responding(|_, submission| {
            // Accept only the first item of each submission to prove positions map back.
            Reply::Outcomes(
                vec![
                    (0, ProductListingIngestionOutcome::Accepted),
                    (1, ProductListingIngestionOutcome::Unconfirmed),
                ][..submission.items.len().min(2)]
                    .to_vec(),
            )
        }));

        let outcomes = service(&use_case)
            .capture(vec![
                item(first, "https://a.test/1"),
                item(second, "https://b.test/1"),
                item(first, "https://a.test/2"),
                item(second, "https://b.test/2"),
            ])
            .await;

        assert_eq!(
            vec![
                ProductListingRawCaptureOutcome::Persisted,
                ProductListingRawCaptureOutcome::Persisted,
                ProductListingRawCaptureOutcome::RetryableFailure,
                ProductListingRawCaptureOutcome::RetryableFailure,
            ],
            outcomes
        );
        let submissions = use_case.submissions();
        assert_eq!(2, submissions.len());
        for submission in submissions {
            let expected = if submission.listing_source_id == first {
                vec!["https://a.test/1", "https://a.test/2"]
            } else {
                vec!["https://b.test/1", "https://b.test/2"]
            };
            assert_eq!(expected, keys(&submission));
        }
    }

    #[tokio::test]
    async fn should_chunk_a_large_source_into_ordered_sequential_submissions() {
        let source = ListingSourceId::new();
        let use_case = Arc::new(FakeSubmitUseCase::accepting_all());
        let observations: Vec<_> = (0..MAX_SUBMISSION_ITEMS + 5)
            .map(|n| item(source, &format!("https://example.test/p/{n}")))
            .collect();

        let outcomes = service(&use_case).capture(observations).await;

        assert_eq!(
            vec![ProductListingRawCaptureOutcome::Persisted; MAX_SUBMISSION_ITEMS + 5],
            outcomes
        );
        let submissions = use_case.submissions();
        assert_eq!(
            vec![MAX_SUBMISSION_ITEMS, 5],
            submissions
                .iter()
                .map(|submission| submission.items.len())
                .collect::<Vec<_>>()
        );
        assert_eq!("https://example.test/p/0", keys(&submissions[0])[0]);
        assert_eq!(
            format!("https://example.test/p/{MAX_SUBMISSION_ITEMS}"),
            keys(&submissions[1])[0]
        );
    }

    #[tokio::test]
    async fn should_discard_an_oversized_observation_without_sending_it() {
        let source = ListingSourceId::new();
        let use_case = Arc::new(FakeSubmitUseCase::accepting_all());
        let oversized = serde_json::json!({
            // The largest payload the input accepts still overflows once the envelope wraps it.
            "description": "x".repeat(MAX_SOURCE_PAYLOAD_JSON_BYTES - 64)
        });

        let outcomes = service(&use_case)
            .capture(vec![
                item(source, "https://example.test/p/small"),
                item_with_payload(source, "https://example.test/p/huge", oversized),
            ])
            .await;

        assert_eq!(
            vec![
                ProductListingRawCaptureOutcome::Persisted,
                ProductListingRawCaptureOutcome::DiscardedUnsendable,
            ],
            outcomes
        );
        let submissions = use_case.submissions();
        assert_eq!(1, submissions.len());
        assert_eq!(vec!["https://example.test/p/small"], keys(&submissions[0]));
        assert_eq!(1, submissions[0].original_input_count);
    }

    #[test]
    fn should_measure_the_encoded_size_of_an_observation() {
        let command = item(ListingSourceId::new(), "https://example.test/p/1").command;
        let context = crawler_operation_context(command.listing_source_id);

        let (returned, encoded) = measure_encoded_bytes(command.clone(), &context);

        assert_eq!(command.source_record_key, returned.source_record_key);
        let bytes = encoded.expect("a small observation encodes");
        assert!(bytes > 0 && bytes < MAX_MESSAGE_BYTES);
    }
}
