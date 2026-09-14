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
    /// Streams that need a later worker-local recovery turn after a clean capped drain.
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
                let StreamDrainResult { revisions, error } = self
                    .drain_stream(product_listing_raw_stream_id, max_revisions_per_stream)
                    .await;
                if let Some(error) = error {
                    return Err(error);
                }
                return Ok(NormalizeProductListingRawRevisionResult {
                    revisions,
                    stream_failures: Vec::new(),
                    pending_stream_page_count: None,
                    oldest_pending_age_seconds: None,
                    next_pending_stream_cursor: None,
                    continuation_stream_ids: Vec::new(),
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
