use crate::ports::{
    PendingProductListingRawStreamCursor, PendingProductListingRawStreamPageRequest,
    PendingProductListingRawStreamReader, ProductListingRawNormalizationCompletion,
    ProductListingRawNormalizationHead, ProductListingRawNormalizationOutcome,
    ProductListingRawNormalizationPortError, ProductListingRawNormalizationWriter,
    ProductListingRawNormalizationWriterFactory, ProductListingRawRevisionReader,
};
use application::patch_field::PatchField;
use application::transaction::{Transaction, TransactionError, UnitOfWork};
use domain_primitives::change_outcome::ChangeOutcome;
use indexmap::IndexSet;
use product_listing_normalization::error::NormalizationFailureScope;
use product_listing_normalization::{
    ListingAvailabilityQuickCheck, PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
    ProductListingRawValuesNormalizationError, ProductListingRawValuesNormalizationOutcome,
    ProductListingRawValuesNormalizer, ProductListingRawValuesPatch,
    ProductListingRawValuesResolved,
};
use product_listing_service::canonical_product_listing_write::{
    CanonicalProductListingNonAuctionUpsert, CanonicalProductListingWriteError,
    CanonicalProductListingWriter,
};
use product_listing_service::ports::{
    ProductListingEventAppenderFactory, ProductListingRawRevisionId, ProductListingRawStreamId,
    ProductListingRepositoryFactory,
};
use std::time::Instant;
use time::OffsetDateTime;

pub const NORMALIZER_VERSION: u16 = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizeProductListingRawRevisionMode {
    /// CDC wake-up metadata. The handler drains from the stream head and never trusts delivery order.
    RawRevision {
        product_listing_raw_stream_id: ProductListingRawStreamId,
        product_listing_raw_revision_id: ProductListingRawRevisionId,
        revision: u64,
    },
    /// Starts a bounded reconciliation traversal from its oldest pending stream.
    Reconcile,
    /// Continues a bounded reconciliation traversal from a result cursor. A result without a
    /// cursor has reached the end and the next run should use `Reconcile` to wrap safely.
    ReconcileFromCursor {
        pending_stream_cursor: PendingProductListingRawStreamCursor,
    },
    /// Drains one worker-local recovery continuation without reading or moving the page cursor.
    ReconcileContinuation {
        product_listing_raw_stream_id: ProductListingRawStreamId,
    },
}

/// A wake-up is scoped to one stream; reconciliation uses the same handler and drains streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizeProductListingRawRevisionCommand {
    pub mode: NormalizeProductListingRawRevisionMode,
    pub max_revisions_per_stream: u32,
    pub pending_stream_limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedRawRevisionResult {
    pub product_listing_raw_stream_id: ProductListingRawStreamId,
    pub revision: u64,
    pub outcome: ProductListingRawNormalizationOutcome,
}

/// Safe metadata for a reconciliation stream that remains pending after a retryable failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingRawNormalizationStreamFailure {
    pub product_listing_raw_stream_id: ProductListingRawStreamId,
    pub error_code: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NormalizeProductListingRawRevisionResult {
    pub revisions: Vec<NormalizedRawRevisionResult>,
    /// Per-stream reconciliation failures. Source errors remain internal to the handler.
    pub stream_failures: Vec<ProductListingRawNormalizationStreamFailure>,
    /// Present for bounded reconciliation only; it is the scanned page, not an unbounded count.
    pub pending_stream_page_count: Option<usize>,
    /// Age of the oldest stream in the bounded reconciliation page.
    pub oldest_pending_age_seconds: Option<u64>,
    /// Non-durable cursor for the next reconciliation page. `None` safely resets the traversal.
    pub next_pending_stream_cursor: Option<PendingProductListingRawStreamCursor>,
    /// Streams needing another bounded drain after a clean capped turn. The SQS Lambda
    /// retains the wake-up until progress is fully drained.
    pub continuation_stream_ids: Vec<ProductListingRawStreamId>,
}

#[derive(Debug, thiserror::Error)]
pub enum NormalizeProductListingRawRevisionError {
    #[error("normalization work limit must be greater than zero")]
    InvalidLimit,
    #[error("failed to read pending raw product listing streams")]
    PendingStreamReadFailed {
        #[source]
        source: ProductListingRawNormalizationPortError,
    },
    #[error("failed to begin raw product listing normalization transaction")]
    BeginTransactionFailed {
        #[source]
        source: TransactionError,
    },
    #[error("raw product listing normalization storage failed")]
    PersistenceFailed {
        #[source]
        source: ProductListingRawNormalizationPortError,
    },
    #[error("raw product listing normalization stored state is invalid")]
    InvalidPersistedState {
        #[source]
        source: ProductListingRawNormalizationPortError,
    },
    #[error("raw product listing schema version is unsupported")]
    UnsupportedStoredSchemaVersion,
    #[error("raw product listing normalizer configuration is invalid")]
    NormalizationConfigurationFailed {
        #[source]
        source: ProductListingRawValuesNormalizationError,
    },
    #[error("raw product listing normalization failed")]
    CanonicalWriteFailed {
        #[source]
        source: CanonicalProductListingWriteError,
    },
    #[error("failed to commit raw product listing normalization transaction")]
    CommitTransactionFailed {
        #[source]
        source: TransactionError,
    },
}

#[derive(Debug)]
struct StreamDrainResult {
    revisions: Vec<NormalizedRawRevisionResult>,
    error: Option<NormalizeProductListingRawRevisionError>,
}

impl StreamDrainResult {
    fn completed(revisions: Vec<NormalizedRawRevisionResult>) -> Self {
        Self {
            revisions,
            error: None,
        }
    }

    fn failed(
        revisions: Vec<NormalizedRawRevisionResult>,
        error: NormalizeProductListingRawRevisionError,
    ) -> Self {
        Self {
            revisions,
            error: Some(error),
        }
    }

    fn requires_continuation(&self, max_revisions: u32) -> bool {
        self.error.is_none() && u32::try_from(self.revisions.len()) == Ok(max_revisions)
    }
}

#[async_trait::async_trait]
pub trait NormalizeProductListingRawRevisionUseCase: Send + Sync {
    async fn execute(
        &self,
        command: NormalizeProductListingRawRevisionCommand,
    ) -> Result<NormalizeProductListingRawRevisionResult, NormalizeProductListingRawRevisionError>;
}

pub struct NormalizeProductListingRawRevisionHandler<U, W, R, E, P> {
    unit_of_work: U,
    raw_normalizations: W,
    products: R,
    events: E,
    pending_streams: P,
    normalizer: ProductListingRawValuesNormalizer,
}

impl<U, W, R, E, P> NormalizeProductListingRawRevisionHandler<U, W, R, E, P> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        unit_of_work: U,
        raw_normalizations: W,
        products: R,
        events: E,
        pending_streams: P,
    ) -> Self {
        Self {
            unit_of_work,
            raw_normalizations,
            products,
            events,
            pending_streams,
            normalizer: ProductListingRawValuesNormalizer::new(),
        }
    }
}

impl<U, W, R, E, P> NormalizeProductListingRawRevisionHandler<U, W, R, E, P>
where
    U: UnitOfWork,
    W: ProductListingRawNormalizationWriterFactory<U::Tx>,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    P: PendingProductListingRawStreamReader + ProductListingRawRevisionReader,
{
    async fn drain_stream(
        &self,
        product_listing_raw_stream_id: ProductListingRawStreamId,
        max_revisions: u32,
    ) -> StreamDrainResult {
        let mut results = Vec::new();
        for _ in 0..max_revisions {
            let candidate = match self
                .pending_streams
                .find_next_revision(product_listing_raw_stream_id)
                .await
            {
                Ok(candidate) => candidate,
                Err(error) => return StreamDrainResult::failed(results, map_port_error(error)),
            };
            let Some(candidate) = candidate else {
                return StreamDrainResult::completed(results);
            };
            if let Err(error) = validate_stored_schema(&candidate.input) {
                return StreamDrainResult::failed(results, error);
            }
            let normalized = match require_terminal_normalization_outcome(
                self.normalizer.normalize(&candidate.input),
            ) {
                Ok(normalized) => normalized,
                Err(error) => return StreamDrainResult::failed(results, error),
            };
            let mut tx = match self.unit_of_work.begin().await {
                Ok(tx) => tx,
                Err(source) => {
                    return StreamDrainResult::failed(
                        results,
                        NormalizeProductListingRawRevisionError::BeginTransactionFailed { source },
                    );
                }
            };
            let work = match self
                .raw_normalizations
                .in_transaction(&mut tx)
                .lock_next(product_listing_raw_stream_id)
                .await
            {
                Ok(work) => work,
                Err(error) => return StreamDrainResult::failed(results, map_port_error(error)),
            };
            let Some(revision) = work.next_revision else {
                return StreamDrainResult::completed(results);
            };
            if revision.product_listing_raw_revision_id != candidate.product_listing_raw_revision_id
                || revision.revision != candidate.revision
            {
                continue;
            }
            let completion = match self
                .complete_work_in_transaction(&mut tx, work.head, revision, &normalized)
                .await
            {
                Ok(completion) => completion,
                Err(error) => return StreamDrainResult::failed(results, error),
            };
            let result = NormalizedRawRevisionResult {
                product_listing_raw_stream_id,
                revision: completion.revision,
                outcome: completion.outcome,
            };
            if let Err(error) = self
                .raw_normalizations
                .in_transaction(&mut tx)
                .complete(completion)
                .await
            {
                return StreamDrainResult::failed(results, map_port_error(error));
            }
            if let Err(source) = tx.commit().await {
                return StreamDrainResult::failed(
                    results,
                    NormalizeProductListingRawRevisionError::CommitTransactionFailed { source },
                );
            }
            results.push(result);
        }
        StreamDrainResult::completed(results)
    }

    async fn complete_work_in_transaction(
        &self,
        tx: &mut U::Tx,
        head: ProductListingRawNormalizationHead,
        revision: crate::ports::ProductListingRawRevision,
        normalized: &ProductListingRawValuesNormalizationOutcome,
    ) -> Result<ProductListingRawNormalizationCompletion, NormalizeProductListingRawRevisionError>
    {
        match normalized {
            ProductListingRawValuesNormalizationOutcome::Invalid(error) => Ok(completion(
                &head,
                &revision,
                ProductListingRawNormalizationOutcome::Rejected,
                None,
                None,
                Some(normalization_error_code(error)),
            )),
            ProductListingRawValuesNormalizationOutcome::Delete => {
                let Some(product_listing_id) = head.product_listing_id else {
                    return Ok(completion(
                        &head,
                        &revision,
                        ProductListingRawNormalizationOutcome::Ignored,
                        None,
                        None,
                        None,
                    ));
                };
                let write = match CanonicalProductListingWriter::withdraw_in_transaction(
                    tx,
                    &self.products,
                    &self.events,
                    product_listing_id,
                )
                .await
                {
                    Ok(write) => write,
                    Err(CanonicalProductListingWriteError::InvalidInput { .. }) => {
                        return Ok(completion(
                            &head,
                            &revision,
                            ProductListingRawNormalizationOutcome::Rejected,
                            None,
                            None,
                            Some("CANONICAL_PRODUCT_LISTING_INVALID"),
                        ));
                    }
                    Err(source) => {
                        return Err(
                            NormalizeProductListingRawRevisionError::CanonicalWriteFailed {
                                source,
                            },
                        );
                    }
                };
                Ok(completion(
                    &head,
                    &revision,
                    if write.outcome == ChangeOutcome::Changed {
                        ProductListingRawNormalizationOutcome::Applied
                    } else {
                        ProductListingRawNormalizationOutcome::NoChange
                    },
                    Some(write.product_listing_id),
                    write.product_listing_event_id,
                    None,
                ))
            }
            ProductListingRawValuesNormalizationOutcome::Resolved(resolved) => {
                if let Some(bound_source_listing_id) = &head.source_listing_id
                    && bound_source_listing_id != &resolved.source_listing_id
                {
                    return Ok(completion(
                        &head,
                        &revision,
                        ProductListingRawNormalizationOutcome::Rejected,
                        None,
                        None,
                        Some("SOURCE_LISTING_ID_MISMATCH"),
                    ));
                }
                let command = canonical_upsert(head.listing_source_id, resolved.as_ref());
                let write = match CanonicalProductListingWriter::upsert_non_auction_in_transaction(
                    tx,
                    &self.products,
                    &self.events,
                    head.product_listing_id,
                    command,
                )
                .await
                {
                    Ok(write) => write,
                    Err(CanonicalProductListingWriteError::InvalidInput { .. }) => {
                        return Ok(completion(
                            &head,
                            &revision,
                            ProductListingRawNormalizationOutcome::Rejected,
                            None,
                            None,
                            Some("CANONICAL_PRODUCT_LISTING_INVALID"),
                        ));
                    }
                    Err(source) => {
                        return Err(
                            NormalizeProductListingRawRevisionError::CanonicalWriteFailed {
                                source,
                            },
                        );
                    }
                };
                let mut completion = completion(
                    &head,
                    &revision,
                    if write.outcome == ChangeOutcome::Changed {
                        ProductListingRawNormalizationOutcome::Applied
                    } else {
                        ProductListingRawNormalizationOutcome::NoChange
                    },
                    Some(write.product_listing_id),
                    write.product_listing_event_id,
                    None,
                );
                completion.next_product_listing_id = Some(write.product_listing_id);
                completion.next_source_listing_id = Some(resolved.source_listing_id.clone());
                Ok(completion)
            }
        }
    }
}

impl<U, W, R, E, P> NormalizeProductListingRawRevisionHandler<U, W, R, E, P>
where
    U: UnitOfWork,
    W: ProductListingRawNormalizationWriterFactory<U::Tx>,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    P: PendingProductListingRawStreamReader + ProductListingRawRevisionReader,
{
    async fn execute_inner(
        &self,
        command: NormalizeProductListingRawRevisionCommand,
    ) -> Result<NormalizeProductListingRawRevisionResult, NormalizeProductListingRawRevisionError>
    {
        if command.max_revisions_per_stream == 0 || command.pending_stream_limit == 0 {
            return Err(NormalizeProductListingRawRevisionError::InvalidLimit);
        }
        let max_revisions_per_stream = command.max_revisions_per_stream;
        let pending_stream_limit = command.pending_stream_limit;
        let cursor = match command.mode {
            NormalizeProductListingRawRevisionMode::RawRevision {
                product_listing_raw_stream_id,
                product_listing_raw_revision_id: _,
                revision: _,
            } => {
                let drain = self
                    .drain_stream(product_listing_raw_stream_id, max_revisions_per_stream)
                    .await;
                let requires_continuation = drain.requires_continuation(max_revisions_per_stream);
                let StreamDrainResult { revisions, error } = drain;
                if let Some(error) = error {
                    return Err(error);
                }
                return Ok(NormalizeProductListingRawRevisionResult {
                    revisions,
                    stream_failures: Vec::new(),
                    pending_stream_page_count: None,
                    oldest_pending_age_seconds: None,
                    next_pending_stream_cursor: None,
                    continuation_stream_ids: if requires_continuation {
                        vec![product_listing_raw_stream_id]
                    } else {
                        Vec::new()
                    },
                });
            }
            NormalizeProductListingRawRevisionMode::ReconcileContinuation {
                product_listing_raw_stream_id,
            } => {
                let drain = self
                    .drain_stream(product_listing_raw_stream_id, max_revisions_per_stream)
                    .await;
                let requires_continuation = drain.requires_continuation(max_revisions_per_stream);
                let StreamDrainResult { revisions, error } = drain;
                let stream_failures = error
                    .into_iter()
                    .map(|error| ProductListingRawNormalizationStreamFailure {
                        product_listing_raw_stream_id,
                        error_code: normalization_failure_code(&error),
                    })
                    .collect();
                return Ok(NormalizeProductListingRawRevisionResult {
                    revisions,
                    stream_failures,
                    pending_stream_page_count: None,
                    oldest_pending_age_seconds: None,
                    next_pending_stream_cursor: None,
                    continuation_stream_ids: if requires_continuation {
                        vec![product_listing_raw_stream_id]
                    } else {
                        Vec::new()
                    },
                });
            }
            NormalizeProductListingRawRevisionMode::Reconcile => None,
            NormalizeProductListingRawRevisionMode::ReconcileFromCursor {
                pending_stream_cursor,
            } => Some(pending_stream_cursor),
        };
        let pending_page =
            self.pending_streams
                .list_pending_stream_page(PendingProductListingRawStreamPageRequest {
                    limit: pending_stream_limit,
                    cursor,
                })
                .await
                .map_err(|source| {
                    NormalizeProductListingRawRevisionError::PendingStreamReadFailed { source }
                })?;
        let oldest_pending_age_seconds = pending_page
            .streams
            .iter()
            .map(|stream| stream.oldest_pending_at)
            .min()
            .and_then(pending_age_seconds);
        let mut result = NormalizeProductListingRawRevisionResult {
            revisions: Vec::new(),
            stream_failures: Vec::new(),
            pending_stream_page_count: Some(pending_page.streams.len()),
            oldest_pending_age_seconds,
            next_pending_stream_cursor: pending_page.next_cursor,
            continuation_stream_ids: Vec::new(),
        };
        for stream in pending_page.streams {
            let drain = self
                .drain_stream(
                    stream.product_listing_raw_stream_id,
                    max_revisions_per_stream,
                )
                .await;
            let requires_continuation = drain.requires_continuation(max_revisions_per_stream);
            let StreamDrainResult { revisions, error } = drain;
            result.revisions.extend(revisions);
            if requires_continuation {
                result
                    .continuation_stream_ids
                    .push(stream.product_listing_raw_stream_id);
            }
            if let Some(error) = error {
                result
                    .stream_failures
                    .push(ProductListingRawNormalizationStreamFailure {
                        product_listing_raw_stream_id: stream.product_listing_raw_stream_id,
                        error_code: normalization_failure_code(&error),
                    });
            }
        }
        Ok(result)
    }
}

#[async_trait::async_trait]
impl<U, W, R, E, P> NormalizeProductListingRawRevisionUseCase
    for NormalizeProductListingRawRevisionHandler<U, W, R, E, P>
where
    U: UnitOfWork + Send + Sync,
    W: ProductListingRawNormalizationWriterFactory<U::Tx> + Send + Sync,
    R: ProductListingRepositoryFactory<U::Tx> + Send + Sync,
    E: ProductListingEventAppenderFactory<U::Tx> + Send + Sync,
    P: PendingProductListingRawStreamReader + ProductListingRawRevisionReader + Send + Sync,
{
    #[tracing::instrument(name = "normalize_product_listing_raw_revision", skip_all)]
    async fn execute(
        &self,
        command: NormalizeProductListingRawRevisionCommand,
    ) -> Result<NormalizeProductListingRawRevisionResult, NormalizeProductListingRawRevisionError>
    {
        let started = Instant::now();
        let result = self.execute_inner(command).await;

        match &result {
            Ok(result) => {
                for failure in &result.stream_failures {
                    tracing::warn!(
                        metric = "product_listing_raw_normalization",
                        normalization_revisions = 0_u64,
                        normalization_failures = 1_u64,
                        normalization_batch_latency_ms = started.elapsed().as_millis() as u64,
                        product_listing_raw_stream_id = %failure.product_listing_raw_stream_id,
                        outcome = "stream_failure",
                        error_code = failure.error_code,
                        "raw product listing reconciliation stream failed"
                    );
                }
                for revision in &result.revisions {
                    tracing::info!(
                        metric = "product_listing_raw_normalization",
                        normalization_revisions = 1_u64,
                        normalization_failures = 0_u64,
                        normalization_batch_latency_ms = started.elapsed().as_millis() as u64,
                        product_listing_raw_stream_id = %revision.product_listing_raw_stream_id,
                        revision = revision.revision,
                        outcome = revision.outcome.as_str(),
                        "raw product listing normalization metric"
                    );
                }
                if let Some(pending_stream_page_count) = result.pending_stream_page_count {
                    tracing::info!(
                        metric = "product_listing_raw_normalization_backlog",
                        pending_stream_page_count,
                        oldest_pending_age_seconds = result.oldest_pending_age_seconds,
                        reconciliation_continuation_stream_count =
                            result.continuation_stream_ids.len(),
                        reconciliation_runs = 1_u64,
                        "raw product listing normalization backlog metric"
                    );
                }
            }
            Err(error) => tracing::warn!(
                metric = "product_listing_raw_normalization",
                normalization_revisions = 0_u64,
                normalization_failures = 1_u64,
                normalization_batch_latency_ms = started.elapsed().as_millis() as u64,
                outcome = "failure",
                error_code = normalization_failure_code(error),
                "raw product listing normalization metric"
            ),
        }

        result
    }
}

fn pending_age_seconds(oldest_pending_at: OffsetDateTime) -> Option<u64> {
    let age = OffsetDateTime::now_utc() - oldest_pending_at;
    u64::try_from(age.whole_seconds()).ok()
}

fn normalization_failure_code(error: &NormalizeProductListingRawRevisionError) -> &'static str {
    match error {
        NormalizeProductListingRawRevisionError::InvalidLimit => "INVALID_LIMIT",
        NormalizeProductListingRawRevisionError::PendingStreamReadFailed { .. } => {
            "PENDING_STREAM_READ_FAILED"
        }
        NormalizeProductListingRawRevisionError::BeginTransactionFailed { .. } => {
            "BEGIN_TRANSACTION_FAILED"
        }
        NormalizeProductListingRawRevisionError::PersistenceFailed { .. } => "PERSISTENCE_FAILED",
        NormalizeProductListingRawRevisionError::InvalidPersistedState { .. } => {
            "INVALID_PERSISTED_STATE"
        }
        NormalizeProductListingRawRevisionError::UnsupportedStoredSchemaVersion => {
            "UNSUPPORTED_STORED_SCHEMA_VERSION"
        }
        NormalizeProductListingRawRevisionError::NormalizationConfigurationFailed { .. } => {
            "NORMALIZATION_CONFIGURATION_FAILED"
        }
        NormalizeProductListingRawRevisionError::CanonicalWriteFailed { .. } => {
            "CANONICAL_WRITE_FAILED"
        }
        NormalizeProductListingRawRevisionError::CommitTransactionFailed { .. } => {
            "COMMIT_TRANSACTION_FAILED"
        }
    }
}

fn canonical_upsert(
    listing_source_id: listing_source_core::ListingSourceId,
    resolved: &ProductListingRawValuesResolved,
) -> CanonicalProductListingNonAuctionUpsert {
    CanonicalProductListingNonAuctionUpsert {
        listing_source_id,
        source_listing_id: resolved.source_listing_id.clone(),
        title: to_patch(&resolved.title),
        description: to_patch(&resolved.description),
        price: to_patch(&resolved.price),
        price_estimate_min: to_patch(&resolved.price_estimate_min),
        price_estimate_max: to_patch(&resolved.price_estimate_max),
        availability: availability_patch(&resolved.availability),
        url: to_patch(&resolved.url),
        images: match &resolved.images {
            ProductListingRawValuesPatch::Set(images) => {
                PatchField::Set(images.iter().cloned().collect::<IndexSet<_>>())
            }
            ProductListingRawValuesPatch::Clear => PatchField::Clear,
            ProductListingRawValuesPatch::Unchanged => PatchField::Unchanged,
        },
    }
}

fn to_patch<T: Clone>(patch: &ProductListingRawValuesPatch<T>) -> PatchField<T> {
    match patch {
        ProductListingRawValuesPatch::Set(value) => PatchField::Set(value.clone()),
        ProductListingRawValuesPatch::Clear => PatchField::Clear,
        ProductListingRawValuesPatch::Unchanged => PatchField::Unchanged,
    }
}

fn availability_patch(
    patch: &ProductListingRawValuesPatch<ListingAvailabilityQuickCheck>,
) -> PatchField<product_listing_core::listing_availability::ListingAvailability> {
    match patch {
        ProductListingRawValuesPatch::Set(ListingAvailabilityQuickCheck::Resolved(value)) => {
            PatchField::Set(*value)
        }
        ProductListingRawValuesPatch::Set(ListingAvailabilityQuickCheck::NoAssertion)
        | ProductListingRawValuesPatch::Clear => PatchField::Clear,
        ProductListingRawValuesPatch::Set(ListingAvailabilityQuickCheck::Unsupported)
        | ProductListingRawValuesPatch::Unchanged => PatchField::Unchanged,
    }
}

// The generic canonical writer needs the caller transaction. Keep that orchestration beside the
// handler rather than exposing a second inbound normalization use case.
fn validate_stored_schema(
    input: &product_listing_normalization::ProductListingNormalizationInput,
) -> Result<(), NormalizeProductListingRawRevisionError> {
    if input.payload_schema_version() != 1
        || input.raw_values_schema_version() != PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION
    {
        return Err(NormalizeProductListingRawRevisionError::UnsupportedStoredSchemaVersion);
    }
    Ok(())
}

fn completion(
    head: &ProductListingRawNormalizationHead,
    revision: &crate::ports::ProductListingRawRevision,
    outcome: ProductListingRawNormalizationOutcome,
    product_listing_id: Option<product_listing_core::product_listing_id::ProductListingId>,
    product_listing_event_id: Option<domain_primitives::event_id::EventId>,
    error_code: Option<&'static str>,
) -> ProductListingRawNormalizationCompletion {
    ProductListingRawNormalizationCompletion {
        product_listing_raw_revision_id: revision.product_listing_raw_revision_id,
        product_listing_raw_stream_id: revision.product_listing_raw_stream_id,
        revision: revision.revision,
        normalizer_version: NORMALIZER_VERSION,
        outcome,
        product_listing_id,
        product_listing_event_id,
        error_code,
        next_product_listing_id: head.product_listing_id,
        next_source_listing_id: head.source_listing_id.clone(),
    }
}

fn require_terminal_normalization_outcome(
    outcome: ProductListingRawValuesNormalizationOutcome,
) -> Result<ProductListingRawValuesNormalizationOutcome, NormalizeProductListingRawRevisionError> {
    match outcome {
        ProductListingRawValuesNormalizationOutcome::Invalid(error)
            if error.failure_scope() == NormalizationFailureScope::System =>
        {
            Err(
                NormalizeProductListingRawRevisionError::NormalizationConfigurationFailed {
                    source: error,
                },
            )
        }
        outcome => Ok(outcome),
    }
}

fn normalization_error_code(error: &ProductListingRawValuesNormalizationError) -> &'static str {
    match error {
        ProductListingRawValuesNormalizationError::InvalidRawValues(_) => "RAW_VALUES_INVALID",
        ProductListingRawValuesNormalizationError::InvalidNormalizationContextV1(_) => {
            "NORMALIZATION_CONTEXT_INVALID"
        }
        ProductListingRawValuesNormalizationError::InvalidBaseUrl(_) => {
            "NORMALIZATION_BASE_URL_INVALID"
        }
        ProductListingRawValuesNormalizationError::InvalidUrl(_) => "LISTING_URL_INVALID",
        ProductListingRawValuesNormalizationError::MachineDecimalFallbackCurrencyRequired => {
            "MACHINE_DECIMAL_FALLBACK_CURRENCY_REQUIRED"
        }
        ProductListingRawValuesNormalizationError::UnsupportedFallbackCurrency => {
            "FALLBACK_CURRENCY_UNSUPPORTED"
        }
        ProductListingRawValuesNormalizationError::UnsupportedFallbackLanguage => {
            "FALLBACK_LANGUAGE_UNSUPPORTED"
        }
        ProductListingRawValuesNormalizationError::Text(_) => "TEXT_NORMALIZATION_INVALID",
        ProductListingRawValuesNormalizationError::Price(_) => "PRICE_NORMALIZATION_INVALID",
        ProductListingRawValuesNormalizationError::ImageUrl(_) => "IMAGE_URL_NORMALIZATION_INVALID",
        ProductListingRawValuesNormalizationError::Availability(_) => {
            "AVAILABILITY_NORMALIZATION_INVALID"
        }
        ProductListingRawValuesNormalizationError::UnsupportedRawValuesSchemaVersion { .. } => {
            "RAW_VALUES_SCHEMA_UNSUPPORTED"
        }
    }
}

fn map_port_error(
    error: ProductListingRawNormalizationPortError,
) -> NormalizeProductListingRawRevisionError {
    match error {
        error @ ProductListingRawNormalizationPortError::Persistence { .. } => {
            NormalizeProductListingRawRevisionError::PersistenceFailed { source: error }
        }
        error @ ProductListingRawNormalizationPortError::InvalidPersistedState { .. } => {
            NormalizeProductListingRawRevisionError::InvalidPersistedState { source: error }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        PendingProductListingRawStream, PendingProductListingRawStreamPage,
        ProductListingRawNormalizationWork,
    };
    use application::transaction::{Transaction, TransactionError, UnitOfWork};
    use domain_primitives::versioned::Versioned;
    use indexmap::IndexSet;
    use listing_source_core::ListingSourceId;
    use product_listing_core::{
        listing_lifecycle::ListingLifecycle,
        product_listing::{
            ProductListing, ProductListingAuction, ProductListingPricing,
            RehydratedProductListingState,
        },
        product_listing_auction::{CataloguePosition, LotNumber},
        product_listing_id::{ProductListingId, ProductListingKey},
        product_listing_price::ProductListingPrice,
        source_listing_id::SourceListingId,
    };
    use product_listing_normalization::{
        NormalizationContext, NormalizationInputError, ProductListingNormalizationInput,
        ProductListingRawValuesNormalizationOutcome, ProductListingRawValuesNormalizer,
        RawProductListingOperation, RawProductListingPayloadFormat, RawProductListingValues,
        SourcePayload,
    };
    use product_listing_service::ports::{
        ProductListingEventAppendError, ProductListingEventAppender, ProductListingRawRevisionId,
        ProductListingRawStreamId, ProductListingRepository, ProductListingRepositoryError,
        ProductListingStorageVersion, ProductListingWriteEffects, VersionedProductListing,
    };
    use std::collections::{HashSet, VecDeque};
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::{Duration, OffsetDateTime};
    use url::Url;

    type SharedState = Arc<Mutex<TestState>>;
    type TestHandler = NormalizeProductListingRawRevisionHandler<
        TestUnitOfWork,
        RawPortsFake,
        ProductsFake,
        EventsFake,
        RawPortsFake,
    >;

    struct TestStream {
        head: ProductListingRawNormalizationHead,
        revisions: VecDeque<crate::ports::ProductListingRawRevision>,
    }

    #[derive(Default)]
    struct TestState {
        streams: Vec<TestStream>,
        revision_failures: HashSet<ProductListingRawStreamId>,
        pending_page: PendingProductListingRawStreamPage,
        pending_requests: Vec<PendingProductListingRawStreamPageRequest>,
        completions: Vec<ProductListingRawNormalizationCompletion>,
        completion_results: VecDeque<Result<(), ProductListingRawNormalizationPortError>>,
        begins: usize,
        commits: usize,
        rollbacks: usize,
        existing_product: Option<VersionedProductListing>,
        updated_products: Vec<ProductListing>,
        update_results: VecDeque<Result<(), ProductListingRepositoryError>>,
    }

    #[derive(Clone)]
    struct TestUnitOfWork(SharedState);

    struct TestTransaction {
        state: SharedState,
        committed: bool,
    }

    #[derive(Clone)]
    struct RawPortsFake(SharedState);

    struct RawWriterFake(SharedState);

    #[derive(Clone)]
    struct ProductsFake(SharedState);

    struct ProductRepositoryFake(SharedState);

    #[derive(Clone, Copy)]
    struct EventsFake;

    struct EventAppenderFake;

    fn lock(state: &SharedState) -> MutexGuard<'_, TestState> {
        match state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn port_error(message: &'static str) -> ProductListingRawNormalizationPortError {
        ProductListingRawNormalizationPortError::InvalidPersistedState {
            source: application::error::box_error(std::io::Error::other(message)),
        }
    }

    fn persistence_error(message: &'static str) -> ProductListingRawNormalizationPortError {
        ProductListingRawNormalizationPortError::Persistence {
            source: application::error::box_error(std::io::Error::other(message)),
        }
    }

    fn raw_head(
        stream_id: ProductListingRawStreamId,
        listing_source_id: ListingSourceId,
        product_listing_id: Option<ProductListingId>,
        source_listing_id: Option<SourceListingId>,
    ) -> ProductListingRawNormalizationHead {
        ProductListingRawNormalizationHead {
            product_listing_raw_stream_id: stream_id,
            listing_source_id,
            last_processed_revision: 0,
            product_listing_id,
            source_listing_id,
        }
    }

    fn revision(
        stream_id: ProductListingRawStreamId,
        revision: u64,
        input: ProductListingNormalizationInput,
    ) -> crate::ports::ProductListingRawRevision {
        crate::ports::ProductListingRawRevision {
            product_listing_raw_revision_id: ProductListingRawRevisionId::new(),
            product_listing_raw_stream_id: stream_id,
            revision,
            input,
        }
    }

    fn delete_input() -> Result<ProductListingNormalizationInput, NormalizationInputError> {
        ProductListingNormalizationInput::new(
            RawProductListingOperation::Delete,
            RawProductListingPayloadFormat::CrawlerExtractedProduct,
            1,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            SourcePayload::new(serde_json::json!({}))?,
            RawProductListingValues::new(serde_json::json!({}))?,
            NormalizationContext::new(serde_json::json!({}))?,
        )
    }

    fn upsert_input(
        price: &str,
    ) -> Result<ProductListingNormalizationInput, NormalizationInputError> {
        ProductListingNormalizationInput::new(
            RawProductListingOperation::Upsert,
            RawProductListingPayloadFormat::CrawlerExtractedProduct,
            1,
            PRODUCT_LISTING_RAW_VALUES_SCHEMA_VERSION,
            SourcePayload::new(serde_json::json!({}))?,
            RawProductListingValues::new(serde_json::json!({
                "sourceListingId": "source-123",
                "title": {"action": "SET", "value": "Fresh ceramic vase"},
                "description": {
                    "action": "SET",
                    "value": ["A fresh description from the source."]
                },
                "priceFormat": "DISPLAY_TEXT",
                "price": {"action": "SET", "value": price},
                "priceEstimateMin": {"action": "UNCHANGED"},
                "priceEstimateMax": {"action": "UNCHANGED"},
                "availability": {"action": "UNCHANGED"},
                "url": {"action": "UNCHANGED"},
                "images": {"action": "UNCHANGED"}
            }))?,
            NormalizationContext::new(serde_json::json!({
                "baseUrl": "https://example.test/catalogue/",
                "fallbackCurrency": "EUR",
                "fallbackLanguage": "en"
            }))?,
        )
    }

    fn existing_listing(
        product_listing_id: ProductListingId,
        listing_source_id: ListingSourceId,
        source_listing_id: SourceListingId,
    ) -> Result<(ProductListing, ProductListingAuction), Box<dyn std::error::Error>> {
        let lot_number = LotNumber::try_from("lot-77")?;
        let catalogue_position = CataloguePosition::new(7)?;
        let bidding_opens = OffsetDateTime::UNIX_EPOCH;
        let auction = ProductListingAuction::new(
            None,
            Some(lot_number),
            Some(catalogue_position),
            Some(bidding_opens),
            Some(bidding_opens + Duration::hours(1)),
            None,
        )?
        .ok_or_else(|| std::io::Error::other("auction facts should be present"))?;
        let listing = ProductListing::rehydrate(RehydratedProductListingState {
            id: product_listing_id,
            title_slug_id:
                product_listing_core::product_listing_slug_id::ProductListingSlugId::raw(
                    "listing-abcdef",
                )?,
            listing_source_id,
            source_listing_id,
            title: None,
            description: None,
            pricing: ProductListingPricing::default(),
            sale_observation: None,
            availability: None,
            lifecycle: ListingLifecycle::Active,
            url: Url::parse("https://example.test/listings/source-123")?,
            images: IndexSet::new(),
            auction: Some(auction.clone()),
        })?;
        Ok((listing, auction))
    }

    fn handler(state: &SharedState) -> TestHandler {
        NormalizeProductListingRawRevisionHandler::new(
            TestUnitOfWork(Arc::clone(state)),
            RawPortsFake(Arc::clone(state)),
            ProductsFake(Arc::clone(state)),
            EventsFake,
            RawPortsFake(Arc::clone(state)),
        )
    }

    impl Drop for TestTransaction {
        fn drop(&mut self) {
            if !self.committed {
                lock(&self.state).rollbacks += 1;
            }
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for TestUnitOfWork {
        type Tx = TestTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            lock(&self.0).begins += 1;
            Ok(TestTransaction {
                state: Arc::clone(&self.0),
                committed: false,
            })
        }
    }

    #[async_trait::async_trait]
    impl Transaction for TestTransaction {
        async fn commit(mut self) -> Result<(), TransactionError> {
            self.committed = true;
            lock(&self.state).commits += 1;
            Ok(())
        }
    }

    impl ProductListingRawNormalizationWriterFactory<TestTransaction> for RawPortsFake {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TestTransaction,
        ) -> impl ProductListingRawNormalizationWriter + 'tx {
            RawWriterFake(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl ProductListingRawNormalizationWriter for RawWriterFake {
        async fn lock_next(
            &mut self,
            product_listing_raw_stream_id: ProductListingRawStreamId,
        ) -> Result<ProductListingRawNormalizationWork, ProductListingRawNormalizationPortError>
        {
            let state = lock(&self.0);
            let Some(stream) = state.streams.iter().find(|stream| {
                stream.head.product_listing_raw_stream_id == product_listing_raw_stream_id
            }) else {
                return Err(port_error("unknown raw stream"));
            };
            Ok(ProductListingRawNormalizationWork {
                head: stream.head.clone(),
                next_revision: stream.revisions.front().cloned(),
            })
        }

        async fn complete(
            &mut self,
            completion: ProductListingRawNormalizationCompletion,
        ) -> Result<(), ProductListingRawNormalizationPortError> {
            let mut state = lock(&self.0);
            if let Some(result) = state.completion_results.pop_front() {
                result?;
            }
            {
                let Some(stream) = state.streams.iter_mut().find(|stream| {
                    stream.head.product_listing_raw_stream_id
                        == completion.product_listing_raw_stream_id
                }) else {
                    return Err(port_error("unknown raw stream completion"));
                };
                let Some(next) = stream.revisions.front() else {
                    return Err(port_error("raw stream completion has no revision"));
                };
                if next.product_listing_raw_revision_id
                    != completion.product_listing_raw_revision_id
                    || next.revision != completion.revision
                {
                    return Err(port_error("raw stream completion is out of order"));
                }
                stream.revisions.pop_front();
            }
            state.completions.push(completion);
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl ProductListingRawRevisionReader for RawPortsFake {
        async fn find_next_revision(
            &self,
            product_listing_raw_stream_id: ProductListingRawStreamId,
        ) -> Result<
            Option<crate::ports::ProductListingRawRevision>,
            ProductListingRawNormalizationPortError,
        > {
            let state = lock(&self.0);
            if state
                .revision_failures
                .contains(&product_listing_raw_stream_id)
            {
                return Err(persistence_error("raw revision read failed"));
            }
            Ok(state
                .streams
                .iter()
                .find(|stream| {
                    stream.head.product_listing_raw_stream_id == product_listing_raw_stream_id
                })
                .and_then(|stream| stream.revisions.front().cloned()))
        }
    }

    #[async_trait::async_trait]
    impl PendingProductListingRawStreamReader for RawPortsFake {
        async fn list_pending_stream_page(
            &self,
            request: PendingProductListingRawStreamPageRequest,
        ) -> Result<PendingProductListingRawStreamPage, ProductListingRawNormalizationPortError>
        {
            let mut state = lock(&self.0);
            state.pending_requests.push(request);
            Ok(state.pending_page.clone())
        }
    }

    impl ProductListingRepositoryFactory<TestTransaction> for ProductsFake {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TestTransaction,
        ) -> impl ProductListingRepository + 'tx {
            ProductRepositoryFake(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl ProductListingRepository for ProductRepositoryFake {
        async fn find_by_id(
            &mut self,
            product_listing_id: ProductListingId,
        ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
            Ok(lock(&self.0)
                .existing_product
                .as_ref()
                .filter(|product| product.value.id() == product_listing_id)
                .cloned())
        }

        async fn find_by_key(
            &mut self,
            key: &ProductListingKey,
        ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
            Ok(lock(&self.0)
                .existing_product
                .as_ref()
                .filter(|product| {
                    product.value.listing_source_id() == key.listing_source_id
                        && product.value.source_listing_id() == &key.source_listing_id
                })
                .cloned())
        }

        async fn insert(
            &mut self,
            product: &ProductListing,
            _: domain_primitives::event_id::EventId,
        ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
            Ok(Versioned::new(
                product.clone(),
                ProductListingStorageVersion::INITIAL,
            ))
        }

        async fn update(
            &mut self,
            product: &ProductListing,
            expected_version: ProductListingStorageVersion,
            _: domain_primitives::event_id::EventId,
            _: ProductListingWriteEffects,
        ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
            let mut state = lock(&self.0);
            state.updated_products.push(product.clone());
            match state.update_results.pop_front() {
                Some(Err(error)) => Err(error),
                Some(Ok(())) | None => Ok(Versioned::new(product.clone(), expected_version.next())),
            }
        }
    }

    impl ProductListingEventAppenderFactory<TestTransaction> for EventsFake {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TestTransaction,
        ) -> impl ProductListingEventAppender + 'tx {
            EventAppenderFake
        }
    }

    #[async_trait::async_trait]
    impl ProductListingEventAppender for EventAppenderFake {
        async fn append(
            &mut self,
            _: &product_listing_service::ports::product_listing_event_appender::ProductListingEvent,
        ) -> Result<(), ProductListingEventAppendError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn should_drain_raw_revisions_in_revision_order_not_delivery_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let stream_id = ProductListingRawStreamId::new();
        let mut revisions = VecDeque::new();
        for revision_number in 1..=3 {
            revisions.push_back(revision(stream_id, revision_number, delete_input()?));
        }
        let state = Arc::new(Mutex::new(TestState::default()));
        lock(&state).streams.push(TestStream {
            head: raw_head(stream_id, ListingSourceId::new(), None, None),
            revisions,
        });

        let result = handler(&state)
            .execute(NormalizeProductListingRawRevisionCommand {
                mode: NormalizeProductListingRawRevisionMode::RawRevision {
                    product_listing_raw_stream_id: stream_id,
                    product_listing_raw_revision_id: ProductListingRawRevisionId::new(),
                    revision: 3,
                },
                max_revisions_per_stream: 3,
                pending_stream_limit: 1,
            })
            .await?;

        assert_eq!(
            vec![1, 2, 3],
            result
                .revisions
                .iter()
                .map(|revision| revision.revision)
                .collect::<Vec<_>>()
        );
        assert!(
            result
                .revisions
                .iter()
                .all(|revision| revision.outcome == ProductListingRawNormalizationOutcome::Ignored)
        );
        let state = lock(&state);
        assert_eq!(
            vec![1, 2, 3],
            state
                .completions
                .iter()
                .map(|completion| completion.revision)
                .collect::<Vec<_>>()
        );
        assert_eq!(vec![stream_id], result.continuation_stream_ids);
        assert!(state.streams[0].revisions.is_empty());
        assert_eq!(3, state.begins);
        assert_eq!(3, state.commits);
        assert_eq!(0, state.rollbacks);
        Ok(())
    }

    #[tokio::test]
    async fn should_bound_reconciliation_and_continue_capped_streams()
    -> Result<(), Box<dyn std::error::Error>> {
        let blocked_stream_id = ProductListingRawStreamId::new();
        let healthy_stream_id = ProductListingRawStreamId::new();
        let blocked_oldest_at = OffsetDateTime::UNIX_EPOCH;
        let healthy_oldest_at = blocked_oldest_at + Duration::seconds(1);
        let mut healthy_revisions = VecDeque::new();
        for revision_number in 1..=3 {
            healthy_revisions.push_back(revision(
                healthy_stream_id,
                revision_number,
                delete_input()?,
            ));
        }
        let state = Arc::new(Mutex::new(TestState::default()));
        {
            let mut state = lock(&state);
            state.revision_failures.insert(blocked_stream_id);
            state.streams.push(TestStream {
                head: raw_head(blocked_stream_id, ListingSourceId::new(), None, None),
                revisions: VecDeque::new(),
            });
            state.streams.push(TestStream {
                head: raw_head(healthy_stream_id, ListingSourceId::new(), None, None),
                revisions: healthy_revisions,
            });
            state.pending_page = PendingProductListingRawStreamPage {
                streams: vec![
                    PendingProductListingRawStream {
                        product_listing_raw_stream_id: blocked_stream_id,
                        oldest_pending_at: blocked_oldest_at,
                    },
                    PendingProductListingRawStream {
                        product_listing_raw_stream_id: healthy_stream_id,
                        oldest_pending_at: healthy_oldest_at,
                    },
                ],
                next_cursor: Some(PendingProductListingRawStreamCursor {
                    oldest_pending_at: healthy_oldest_at,
                    product_listing_raw_stream_id: healthy_stream_id,
                }),
            };
        }

        let normalizer = handler(&state);
        let first = normalizer
            .execute(NormalizeProductListingRawRevisionCommand {
                mode: NormalizeProductListingRawRevisionMode::Reconcile,
                max_revisions_per_stream: 2,
                pending_stream_limit: 2,
            })
            .await?;

        assert_eq!(
            vec![1, 2],
            first
                .revisions
                .iter()
                .map(|revision| revision.revision)
                .collect::<Vec<_>>()
        );
        assert_eq!(vec![healthy_stream_id], first.continuation_stream_ids);
        assert_eq!(Some(2), first.pending_stream_page_count);
        assert!(first.oldest_pending_age_seconds.is_some());
        assert_eq!(
            Some(PendingProductListingRawStreamCursor {
                oldest_pending_at: healthy_oldest_at,
                product_listing_raw_stream_id: healthy_stream_id,
            }),
            first.next_pending_stream_cursor
        );
        assert!(matches!(
            first.stream_failures.as_slice(),
            [failure]
                if failure.product_listing_raw_stream_id == blocked_stream_id
                    && failure.error_code == "PERSISTENCE_FAILED"
        ));

        let continuation = normalizer
            .execute(NormalizeProductListingRawRevisionCommand {
                mode: NormalizeProductListingRawRevisionMode::ReconcileContinuation {
                    product_listing_raw_stream_id: healthy_stream_id,
                },
                max_revisions_per_stream: 2,
                pending_stream_limit: 2,
            })
            .await?;

        assert_eq!(
            vec![3],
            continuation
                .revisions
                .iter()
                .map(|revision| revision.revision)
                .collect::<Vec<_>>()
        );
        assert!(continuation.stream_failures.is_empty());
        assert!(continuation.continuation_stream_ids.is_empty());
        assert_eq!(None, continuation.pending_stream_page_count);
        assert_eq!(None, continuation.next_pending_stream_cursor);

        let state = lock(&state);
        assert_eq!(1, state.pending_requests.len());
        assert_eq!(2, state.pending_requests[0].limit);
        assert_eq!(None, state.pending_requests[0].cursor);
        assert_eq!(3, state.completions.len());
        assert_eq!(3, state.commits);
        Ok(())
    }

    #[tokio::test]
    async fn should_leave_raw_head_pending_when_canonical_persistence_fails()
    -> Result<(), Box<dyn std::error::Error>> {
        let stream_id = ProductListingRawStreamId::new();
        let revision_id = ProductListingRawRevisionId::new();
        let listing_source_id = ListingSourceId::new();
        let product_listing_id = ProductListingId::new();
        let source_listing_id = SourceListingId::try_from("source-123")?;
        let (listing, _) = existing_listing(
            product_listing_id,
            listing_source_id,
            source_listing_id.clone(),
        )?;
        let state = Arc::new(Mutex::new(TestState::default()));
        {
            let mut state = lock(&state);
            state.existing_product = Some(Versioned::new(
                listing,
                ProductListingStorageVersion::INITIAL,
            ));
            state.update_results.push_back(Err(
                ProductListingRepositoryError::ProductListingUpdateFailed,
            ));
            state.streams.push(TestStream {
                head: raw_head(
                    stream_id,
                    listing_source_id,
                    Some(product_listing_id),
                    Some(source_listing_id),
                ),
                revisions: VecDeque::from([crate::ports::ProductListingRawRevision {
                    product_listing_raw_revision_id: revision_id,
                    product_listing_raw_stream_id: stream_id,
                    revision: 1,
                    input: upsert_input("EUR 250")?,
                }]),
            });
        }

        let result = handler(&state)
            .execute(NormalizeProductListingRawRevisionCommand {
                mode: NormalizeProductListingRawRevisionMode::RawRevision {
                    product_listing_raw_stream_id: stream_id,
                    product_listing_raw_revision_id: revision_id,
                    revision: 1,
                },
                max_revisions_per_stream: 1,
                pending_stream_limit: 1,
            })
            .await;

        assert!(matches!(
            result,
            Err(
                NormalizeProductListingRawRevisionError::CanonicalWriteFailed {
                    source: CanonicalProductListingWriteError::Persistence { .. }
                }
            )
        ));
        let state = lock(&state);
        assert_eq!(1, state.updated_products.len());
        assert_eq!(1, state.streams[0].revisions.len());
        assert!(state.completions.is_empty());
        assert_eq!(1, state.begins);
        assert_eq!(0, state.commits);
        assert_eq!(1, state.rollbacks);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_generic_content_and_price_without_overwriting_auction_lot_facts()
    -> Result<(), Box<dyn std::error::Error>> {
        let stream_id = ProductListingRawStreamId::new();
        let revision_id = ProductListingRawRevisionId::new();
        let listing_source_id = ListingSourceId::new();
        let product_listing_id = ProductListingId::new();
        let source_listing_id = SourceListingId::try_from("source-123")?;
        let input = upsert_input("EUR 250")?;
        let ProductListingRawValuesNormalizationOutcome::Resolved(resolved) =
            ProductListingRawValuesNormalizer::new().normalize(&input)
        else {
            return Err(std::io::Error::other("generic raw input should resolve").into());
        };
        let canonical = canonical_upsert(listing_source_id, resolved.as_ref());
        assert!(matches!(
            &canonical.title,
            PatchField::Set(value) if value.payload.as_ref() == "Fresh ceramic vase"
        ));
        assert!(matches!(
            &canonical.description,
            PatchField::Set(value) if value.payload.as_ref() == "A fresh description from the source."
        ));
        assert!(matches!(
            &canonical.price,
            PatchField::Set(ProductListingPrice::Monetary(price))
                if u64::from(price.monetary_amount) == 25_000
                    && price.currency.currency_symbol() == "€"
        ));

        let (listing, auction) = existing_listing(
            product_listing_id,
            listing_source_id,
            source_listing_id.clone(),
        )?;
        let state = Arc::new(Mutex::new(TestState::default()));
        {
            let mut state = lock(&state);
            state.existing_product = Some(Versioned::new(
                listing,
                ProductListingStorageVersion::INITIAL,
            ));
            state.streams.push(TestStream {
                head: raw_head(
                    stream_id,
                    listing_source_id,
                    Some(product_listing_id),
                    Some(source_listing_id),
                ),
                revisions: VecDeque::from([crate::ports::ProductListingRawRevision {
                    product_listing_raw_revision_id: revision_id,
                    product_listing_raw_stream_id: stream_id,
                    revision: 1,
                    input,
                }]),
            });
        }

        let result = handler(&state)
            .execute(NormalizeProductListingRawRevisionCommand {
                mode: NormalizeProductListingRawRevisionMode::RawRevision {
                    product_listing_raw_stream_id: stream_id,
                    product_listing_raw_revision_id: revision_id,
                    revision: 1,
                },
                max_revisions_per_stream: 1,
                pending_stream_limit: 1,
            })
            .await?;

        assert!(matches!(
            result.revisions.as_slice(),
            [revision]
                if revision.revision == 1
                    && revision.outcome == ProductListingRawNormalizationOutcome::Applied
        ));
        let state = lock(&state);
        let updated = state
            .updated_products
            .first()
            .ok_or_else(|| std::io::Error::other("canonical update was not recorded"))?;
        assert_eq!(Some(&auction), updated.auction());
        let Some(ProductListingPrice::Monetary(price)) = updated.pricing().price else {
            return Err(std::io::Error::other("normalized price was not persisted").into());
        };
        assert_eq!(25_000, u64::from(price.monetary_amount));
        assert_eq!("€", price.currency.currency_symbol());
        assert_eq!(1, state.completions.len());
        assert_eq!(1, state.commits);
        Ok(())
    }
}
