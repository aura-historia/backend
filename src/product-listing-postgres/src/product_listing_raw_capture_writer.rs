use crate::object_id::try_from_uuid;
use application::error::box_error;
use product_listing_service::ports::{
    ProductListingRawCaptureWrite, ProductListingRawCaptureWriteError,
    ProductListingRawCaptureWriteOutcome, ProductListingRawCaptureWriter,
    ProductListingRawCaptureWriterFactory, ProductListingRawRevisionId, ProductListingRawStreamId,
};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingRawCaptureWriterFactory;

struct SqlxProductListingRawCaptureWriter<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct RawStreamHeadRow {
    product_listing_raw_stream_id: uuid::Uuid,
    source_record_key: String,
    latest_revision: i64,
    latest_input_sha256: Option<Vec<u8>>,
    latest_provider_source_epoch_seconds: Option<i64>,
    latest_provider_source_nanoseconds: Option<i32>,
    latest_provider_source_operation: Option<String>,
    latest_provider_source_ordering_state: String,
    latest_provider_source_observation_sha256: Option<Vec<u8>>,
    latest_provider_delete_barrier_at: Option<OffsetDateTime>,
}

impl SqlxProductListingRawCaptureWriterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingRawCaptureWriterFactory<platform_postgres::SqlxTransaction>
    for SqlxProductListingRawCaptureWriterFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut platform_postgres::SqlxTransaction,
    ) -> impl ProductListingRawCaptureWriter + 'tx {
        SqlxProductListingRawCaptureWriter {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingRawCaptureWriter for SqlxProductListingRawCaptureWriter<'_> {
    async fn capture(
        &mut self,
        write: ProductListingRawCaptureWrite,
    ) -> Result<ProductListingRawCaptureWriteOutcome, ProductListingRawCaptureWriteError> {
        let listing_source_id = write.listing_source_id.into_uuid();
        let source_record_key_sha256 = write.source_record_key_sha256.as_bytes().as_slice();
        let provider_receipt = match write.ingestion_method {
            product_listing_service::ports::ProductListingRawIngestionMethod::WebCrawl => None,
            product_listing_service::ports::ProductListingRawIngestionMethod::Shopify
            | product_listing_service::ports::ProductListingRawIngestionMethod::Woocommerce => {
                write.provider_receipt.as_ref()
            }
        };
        let canonical_source_evidence_sha256 = match write.ingestion_method {
            product_listing_service::ports::ProductListingRawIngestionMethod::WebCrawl => None,
            product_listing_service::ports::ProductListingRawIngestionMethod::Shopify
            | product_listing_service::ports::ProductListingRawIngestionMethod::Woocommerce => {
                Some(write.input.source_payload().canonical_sha256())
            }
        }
        .transpose()
        .map_err(|error| ProductListingRawCaptureWriteError::CaptureFailed {
            source: box_error(error),
        })?
        .map(|evidence| *evidence.as_bytes());
        let source_order_observation_sha256 =
            canonical_source_evidence_sha256
                .as_ref()
                .map(|source_evidence_sha256| {
                    provider_source_order_observation_sha256(
                        write.input.operation().as_str(),
                        source_evidence_sha256,
                    )
                });

        let new_stream_id = ProductListingRawStreamId::new();
        sqlx::query(
            r#"
            INSERT INTO product_listing_raw_streams (
                product_listing_raw_stream_id,
                listing_source_id,
                ingestion_method,
                source_record_key,
                source_record_key_sha256,
                latest_revision
            ) VALUES ($1, $2, $3, $4, $5, 0)
            ON CONFLICT (listing_source_id, ingestion_method, source_record_key_sha256) DO NOTHING
            "#,
        )
        .bind(new_stream_id.as_uuid())
        .bind(listing_source_id)
        .bind(write.ingestion_method.as_str())
        .bind(&write.source_record_key)
        .bind(source_record_key_sha256)
        .execute(&mut *self.connection)
        .await
        .map_err(capture_failed)?;

        let stream = sqlx::query_as::<_, RawStreamHeadRow>(
            r#"
            SELECT
                product_listing_raw_stream_id,
                source_record_key,
                latest_revision,
                latest_input_sha256,
                latest_provider_source_epoch_seconds,
                latest_provider_source_nanoseconds,
                latest_provider_source_operation,
                latest_provider_source_ordering_state,
                latest_provider_source_observation_sha256,
                latest_provider_delete_barrier_at
            FROM product_listing_raw_streams
            WHERE listing_source_id = $1
              AND ingestion_method = $2
              AND source_record_key_sha256 = $3
            FOR UPDATE
            "#,
        )
        .bind(listing_source_id)
        .bind(write.ingestion_method.as_str())
        .bind(source_record_key_sha256)
        .fetch_one(&mut *self.connection)
        .await
        .map_err(capture_failed)?;

        let stream_id = try_from_uuid::<ProductListingRawStreamId>(
            stream.product_listing_raw_stream_id,
            "ProductListing raw stream ID",
        )
        .map_err(invalid_capture_state_error)?;

        if stream.source_record_key != write.source_record_key {
            return Err(ProductListingRawCaptureWriteError::SourceRecordKeyHashCollision);
        }

        let latest_revision = u64::try_from(stream.latest_revision)
            .map_err(|_| invalid_capture_state("raw stream revision is invalid"))?;

        // Retained broad-topic commands are readable, but no longer carry write authority.
        if write.ingestion_method
            == product_listing_service::ports::ProductListingRawIngestionMethod::Shopify
            && write.input.payload_schema_version() == 1
        {
            return Ok(ProductListingRawCaptureWriteOutcome::Stale {
                product_listing_raw_stream_id: stream_id,
                latest_revision,
            });
        }

        if let Some(provider_receipt) = provider_receipt {
            let canonical_source_evidence_sha256 =
                canonical_source_evidence_sha256.as_ref().ok_or_else(|| {
                    invalid_capture_state("provider receipt source evidence is missing")
                })?;
            if provider_receipt.source_evidence_sha256().as_bytes()
                != canonical_source_evidence_sha256
            {
                return Err(ProductListingRawCaptureWriteError::ProviderReceiptDigestConflict);
            }

            sqlx::query(
                r#"
                DELETE FROM product_listing_raw_provider_observation_receipts
                WHERE product_listing_raw_stream_id = $1
                  AND provider_scope = $2
                  AND provider_delivery_id = $3
                  AND expires_at <= clock_timestamp()
                "#,
            )
            .bind(stream_id.as_uuid())
            .bind(provider_receipt.scope().as_str())
            .bind(provider_receipt.delivery_id())
            .execute(&mut *self.connection)
            .await
            .map_err(capture_failed)?;

            let existing_evidence_sha256 = sqlx::query_scalar::<_, Vec<u8>>(
                r#"
                SELECT observation_sha256
                FROM product_listing_raw_provider_observation_receipts
                WHERE product_listing_raw_stream_id = $1
                  AND provider_scope = $2
                  AND provider_delivery_id = $3
                "#,
            )
            .bind(stream_id.as_uuid())
            .bind(provider_receipt.scope().as_str())
            .bind(provider_receipt.delivery_id())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(capture_failed)?;

            if let Some(existing_evidence_sha256) = existing_evidence_sha256 {
                if existing_evidence_sha256.len() != canonical_source_evidence_sha256.len() {
                    return Err(invalid_capture_state(
                        "provider receipt evidence hash is invalid",
                    ));
                }
                if existing_evidence_sha256.as_slice()
                    == canonical_source_evidence_sha256.as_slice()
                {
                    return Ok(ProductListingRawCaptureWriteOutcome::Duplicate {
                        product_listing_raw_stream_id: stream_id,
                        latest_revision,
                    });
                }
                return Err(ProductListingRawCaptureWriteError::ProviderReceiptDigestConflict);
            }
        }

        let incoming_source_order =
            match (write.source_occurred_at, source_order_observation_sha256) {
                (Some(source_occurred_at), Some(source_order_observation_sha256)) => {
                    Some(ProviderSourceOrder::from_source_occurred_at(
                        source_occurred_at,
                        write.input.operation().as_str(),
                        source_order_observation_sha256,
                    )?)
                }
                _ => None,
            };
        let latest_source_order = match source_order_observation_sha256.as_ref() {
            Some(source_order_observation_sha256) => {
                latest_provider_source_order(&stream, source_order_observation_sha256.len())?
            }
            None => LatestProviderSourceOrder::NoOrdering,
        };
        let source_ordering = source_ordering_decision(
            latest_source_order,
            incoming_source_order,
            write.input.operation().as_str(),
            source_order_observation_sha256,
        )?;

        if let Some(provider_receipt) = provider_receipt {
            let canonical_source_evidence_sha256 =
                canonical_source_evidence_sha256.as_ref().ok_or_else(|| {
                    invalid_capture_state("provider receipt source evidence is missing")
                })?;
            sqlx::query(
                r#"
                INSERT INTO product_listing_raw_provider_observation_receipts (
                    product_listing_raw_stream_id,
                    provider_scope,
                    provider_delivery_id,
                    observation_sha256
                ) VALUES ($1, $2, $3, $4)
                "#,
            )
            .bind(stream_id.as_uuid())
            .bind(provider_receipt.scope().as_str())
            .bind(provider_receipt.delivery_id())
            .bind(canonical_source_evidence_sha256.as_slice())
            .execute(&mut *self.connection)
            .await
            .map_err(capture_failed)?;
        }

        let source_ordering_advancement = match source_ordering {
            SourceOrderingDecision::MatchesHead => {
                return Ok(ProductListingRawCaptureWriteOutcome::Unchanged {
                    product_listing_raw_stream_id: stream_id,
                    latest_revision,
                });
            }
            SourceOrderingDecision::Stale => {
                return Ok(ProductListingRawCaptureWriteOutcome::Stale {
                    product_listing_raw_stream_id: stream_id,
                    latest_revision,
                });
            }
            SourceOrderingDecision::Advance(advancement) => Some(advancement),
            SourceOrderingDecision::Capture => None,
        };

        if stream.latest_input_sha256.as_deref() == Some(write.input_sha256.as_bytes().as_slice()) {
            if let Some(source_ordering_advancement) = source_ordering_advancement.as_ref() {
                update_provider_source_order(
                    self.connection,
                    stream_id,
                    source_ordering_advancement,
                )
                .await?;
            }

            return Ok(ProductListingRawCaptureWriteOutcome::Unchanged {
                product_listing_raw_stream_id: stream_id,
                latest_revision,
            });
        }

        let revision = latest_revision
            .checked_add(1)
            .ok_or_else(|| invalid_capture_state("raw stream revision overflow"))?;
        let revision_as_i64 = i64::try_from(revision)
            .map_err(|_| invalid_capture_state("raw stream revision exceeds storage range"))?;
        let product_listing_raw_revision_id = ProductListingRawRevisionId::new();

        sqlx::query(
            r#"
            INSERT INTO product_listing_raw_revisions (
                product_listing_raw_revision_id,
                product_listing_raw_stream_id,
                revision,
                operation,
                payload_format,
                payload_schema_version,
                raw_values_schema_version,
                source_payload,
                raw_values,
                normalization_context,
                provenance,
                input_sha256,
                source_event_id,
                source_occurred_at
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14
            )
            "#,
        )
        .bind(product_listing_raw_revision_id.as_uuid())
        .bind(stream_id.as_uuid())
        .bind(revision_as_i64)
        .bind(write.input.operation().as_str())
        .bind(write.input.payload_format().as_str())
        .bind(
            i16::try_from(write.input.payload_schema_version()).map_err(|_| {
                invalid_capture_state("payload schema version exceeds storage range")
            })?,
        )
        .bind(
            i16::try_from(write.input.raw_values_schema_version()).map_err(|_| {
                invalid_capture_state("raw-values schema version exceeds storage range")
            })?,
        )
        .bind(write.input.source_payload().value().clone())
        .bind(write.input.raw_values().value().clone())
        .bind(write.input.normalization_context().value().clone())
        .bind(write.provenance.value().clone())
        .bind(write.input_sha256.as_bytes().as_slice())
        .bind(write.source_event_id)
        .bind(write.source_occurred_at)
        .execute(&mut *self.connection)
        .await
        .map_err(capture_failed)?;

        update_stream_head(
            self.connection,
            stream_id,
            revision_as_i64,
            write.input_sha256.as_bytes(),
            source_ordering_advancement.as_ref(),
        )
        .await?;

        Ok(ProductListingRawCaptureWriteOutcome::Changed {
            product_listing_raw_stream_id: stream_id,
            product_listing_raw_revision_id,
            revision,
        })
    }
}

fn provider_source_order_observation_sha256(
    operation: &str,
    source_evidence_sha256: &[u8; 32],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"PRODUCT_LISTING_PROVIDER_SOURCE_ORDER\0");
    digest.update(operation.as_bytes());
    digest.update([0]);
    digest.update(source_evidence_sha256);
    digest.finalize().into()
}

#[derive(Debug, Clone, Copy)]
struct ProviderSourceOrder {
    occurred_at: OffsetDateTime,
    epoch_seconds: i64,
    nanoseconds: i32,
    operation: &'static str,
    observation_sha256: [u8; 32],
}

impl ProviderSourceOrder {
    fn from_source_occurred_at(
        source_occurred_at: OffsetDateTime,
        operation: &'static str,
        observation_sha256: [u8; 32],
    ) -> Result<Self, ProductListingRawCaptureWriteError> {
        let nanoseconds = i32::try_from(source_occurred_at.nanosecond()).map_err(|_| {
            invalid_capture_state("source timestamp nanoseconds exceed storage range")
        })?;

        Ok(Self {
            occurred_at: source_occurred_at,
            epoch_seconds: source_occurred_at.unix_timestamp(),
            nanoseconds,
            operation,
            observation_sha256,
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum SourceOrderingAdvancement {
    Known(ProviderSourceOrder),
    /// Enters or stays in the timestamp-free delete state. The stored barrier becomes the
    /// later of the capture time and `not_before`, never earlier than before.
    UnknownDelete {
        observation_sha256: [u8; 32],
        not_before: Option<OffsetDateTime>,
    },
}

/// Where an incoming provider observation falls relative to the stream's source order.
#[derive(Debug, Clone, Copy)]
enum SourceOrderingDecision {
    /// Capture and record the observation as the latest source position.
    Advance(SourceOrderingAdvancement),
    /// Capture without changing the recorded source position.
    Capture,
    /// Older than the recorded position: complete without capture.
    Stale,
    /// The observation already recorded at the latest position: complete without capture.
    MatchesHead,
}

#[derive(Debug, Clone, Copy)]
enum LatestProviderSourceOrder<'a> {
    NoOrdering,
    Known {
        epoch_seconds: i64,
        nanoseconds: i32,
        operation: &'a str,
        observation_sha256: &'a [u8],
    },
    /// A delete without a source time. Its barrier is the later of its capture time and every
    /// source time known for the record, so the provider deleted the record no later than it.
    UnknownDelete {
        barrier_at: OffsetDateTime,
    },
}

impl LatestProviderSourceOrder<'_> {
    /// The latest source time this order proves, which a following delete barrier must keep.
    fn known_source_time(
        self,
    ) -> Result<Option<OffsetDateTime>, ProductListingRawCaptureWriteError> {
        match self {
            Self::NoOrdering => Ok(None),
            Self::Known {
                epoch_seconds,
                nanoseconds,
                ..
            } => OffsetDateTime::from_unix_timestamp_nanos(
                i128::from(epoch_seconds) * 1_000_000_000 + i128::from(nanoseconds),
            )
            .map(Some)
            .map_err(|_| invalid_capture_state("raw stream provider source time is invalid")),
            Self::UnknownDelete { barrier_at, .. } => Ok(Some(barrier_at)),
        }
    }
}

fn latest_provider_source_order(
    stream: &RawStreamHeadRow,
    expected_evidence_sha256_length: usize,
) -> Result<LatestProviderSourceOrder<'_>, ProductListingRawCaptureWriteError> {
    let invalid = || invalid_capture_state("raw stream provider source ordering state is invalid");
    match stream.latest_provider_source_ordering_state.as_str() {
        "NO_ORDERING" => match (
            stream.latest_provider_source_epoch_seconds,
            stream.latest_provider_source_nanoseconds,
            stream.latest_provider_source_operation.as_deref(),
            stream.latest_provider_source_observation_sha256.as_deref(),
            stream.latest_provider_delete_barrier_at,
        ) {
            (None, None, None, None, None) => Ok(LatestProviderSourceOrder::NoOrdering),
            _ => Err(invalid()),
        },
        "UNKNOWN_DELETE" => match (
            stream.latest_provider_source_epoch_seconds,
            stream.latest_provider_source_nanoseconds,
            stream.latest_provider_source_operation.as_deref(),
            stream.latest_provider_source_observation_sha256.as_deref(),
            stream.latest_provider_delete_barrier_at,
        ) {
            (None, None, None, Some(observation_sha256), Some(barrier_at))
                if observation_sha256.len() == expected_evidence_sha256_length =>
            {
                Ok(LatestProviderSourceOrder::UnknownDelete { barrier_at })
            }
            _ => Err(invalid()),
        },
        "KNOWN" => match (
            stream.latest_provider_source_epoch_seconds,
            stream.latest_provider_source_nanoseconds,
            stream.latest_provider_source_operation.as_deref(),
            stream.latest_provider_source_observation_sha256.as_deref(),
            stream.latest_provider_delete_barrier_at,
        ) {
            (
                Some(epoch_seconds),
                Some(nanoseconds),
                Some(operation @ ("UPSERT" | "DELETE")),
                Some(observation_sha256),
                None,
            ) if (0..1_000_000_000).contains(&nanoseconds)
                && observation_sha256.len() == expected_evidence_sha256_length =>
            {
                Ok(LatestProviderSourceOrder::Known {
                    epoch_seconds,
                    nanoseconds,
                    operation,
                    observation_sha256,
                })
            }
            _ => Err(invalid()),
        },
        _ => Err(invalid()),
    }
}

/// Orders an incoming provider observation against the latest recorded one; the contract is the
/// provider source order in `docs/events/flow.md`.
fn source_ordering_decision(
    latest: LatestProviderSourceOrder<'_>,
    incoming: Option<ProviderSourceOrder>,
    incoming_operation: &'static str,
    incoming_observation_sha256: Option<[u8; 32]>,
) -> Result<SourceOrderingDecision, ProductListingRawCaptureWriteError> {
    match (latest, incoming) {
        (LatestProviderSourceOrder::NoOrdering, Some(incoming)) => Ok(
            SourceOrderingDecision::Advance(SourceOrderingAdvancement::Known(incoming)),
        ),
        (
            LatestProviderSourceOrder::Known {
                operation: "DELETE",
                ..
            },
            None,
        ) if incoming_operation == "UPSERT" => {
            Err(ProductListingRawCaptureWriteError::ProviderSourceOrderAmbiguous)
        }

        (LatestProviderSourceOrder::Known { .. }, None)
        | (LatestProviderSourceOrder::NoOrdering, None)
            if incoming_operation == "UPSERT" =>
        {
            Ok(SourceOrderingDecision::Capture)
        }
        (LatestProviderSourceOrder::UnknownDelete { .. }, None)
            if incoming_operation == "UPSERT" =>
        {
            Err(ProductListingRawCaptureWriteError::ProviderSourceOrderAmbiguous)
        }
        (_, None) => match incoming_observation_sha256 {
            Some(observation_sha256) if incoming_operation == "DELETE" => Ok(
                SourceOrderingDecision::Advance(SourceOrderingAdvancement::UnknownDelete {
                    observation_sha256,
                    not_before: latest.known_source_time()?,
                }),
            ),
            _ => Ok(SourceOrderingDecision::Capture),
        },
        // The provider deleted the record no later than the barrier, so only a later source
        // time follows every delete Aura has seen.
        (LatestProviderSourceOrder::UnknownDelete { barrier_at }, Some(incoming)) => {
            if incoming.occurred_at > barrier_at {
                Ok(SourceOrderingDecision::Advance(
                    SourceOrderingAdvancement::Known(incoming),
                ))
            } else {
                Ok(SourceOrderingDecision::Stale)
            }
        }
        (
            LatestProviderSourceOrder::Known {
                epoch_seconds,
                nanoseconds,
                operation,
                observation_sha256,
            },
            Some(incoming),
        ) => {
            match (incoming.epoch_seconds, incoming.nanoseconds).cmp(&(epoch_seconds, nanoseconds))
            {
                std::cmp::Ordering::Less => Ok(SourceOrderingDecision::Stale),
                std::cmp::Ordering::Equal
                    if incoming.observation_sha256.as_slice() == observation_sha256 =>
                {
                    Ok(SourceOrderingDecision::MatchesHead)
                }
                // Withdrawal wins a tie: an upsert never overtakes a delete with its source time.
                std::cmp::Ordering::Equal
                    if operation == "DELETE" && incoming.operation == "UPSERT" =>
                {
                    Ok(SourceOrderingDecision::Stale)
                }
                // Provider clocks are often second-precise: FIFO admission orders other distinct
                // observations that share a source time, so the later one is captured.
                std::cmp::Ordering::Equal | std::cmp::Ordering::Greater => Ok(
                    SourceOrderingDecision::Advance(SourceOrderingAdvancement::Known(incoming)),
                ),
            }
        }
    }
}

async fn update_provider_source_order(
    connection: &mut PgConnection,
    product_listing_raw_stream_id: ProductListingRawStreamId,
    advancement: &SourceOrderingAdvancement,
) -> Result<(), ProductListingRawCaptureWriteError> {
    match advancement {
        SourceOrderingAdvancement::Known(order) => {
            sqlx::query(
                r#"
                UPDATE product_listing_raw_streams
                SET latest_provider_source_epoch_seconds = $1,
                    latest_provider_source_nanoseconds = $2,
                    latest_provider_source_operation = $3,
                    latest_provider_source_ordering_state = 'KNOWN',
                    latest_provider_source_observation_sha256 = $4,
                    latest_provider_delete_barrier_at = NULL,
                    updated = now()
                WHERE product_listing_raw_stream_id = $5
                "#,
            )
            .bind(order.epoch_seconds)
            .bind(order.nanoseconds)
            .bind(order.operation)
            .bind(order.observation_sha256.as_slice())
            .bind(product_listing_raw_stream_id.as_uuid())
            .execute(&mut *connection)
            .await
            .map_err(capture_failed)?;
        }
        SourceOrderingAdvancement::UnknownDelete {
            observation_sha256,
            not_before,
        } => {
            sqlx::query(
                r#"
                UPDATE product_listing_raw_streams
                SET latest_provider_source_epoch_seconds = NULL,
                    latest_provider_source_nanoseconds = NULL,
                    latest_provider_source_operation = NULL,
                    latest_provider_source_ordering_state = 'UNKNOWN_DELETE',
                    latest_provider_source_observation_sha256 = $1,
                    latest_provider_delete_barrier_at = GREATEST(
                        latest_provider_delete_barrier_at,
                        now(),
                        $2
                    ),
                    updated = now()
                WHERE product_listing_raw_stream_id = $3
                "#,
            )
            .bind(observation_sha256.as_slice())
            .bind(not_before)
            .bind(product_listing_raw_stream_id.as_uuid())
            .execute(&mut *connection)
            .await
            .map_err(capture_failed)?;
        }
    }

    Ok(())
}

async fn update_stream_head(
    connection: &mut PgConnection,
    product_listing_raw_stream_id: ProductListingRawStreamId,
    revision: i64,
    input_sha256: &[u8; 32],
    advancement: Option<&SourceOrderingAdvancement>,
) -> Result<(), ProductListingRawCaptureWriteError> {
    if let Some(advancement) = advancement {
        update_provider_source_order(connection, product_listing_raw_stream_id, advancement)
            .await?;
    }

    sqlx::query(
        r#"
        UPDATE product_listing_raw_streams
        SET latest_revision = $1,
            latest_input_sha256 = $2,
            updated = now()
        WHERE product_listing_raw_stream_id = $3
        "#,
    )
    .bind(revision)
    .bind(input_sha256.as_slice())
    .bind(product_listing_raw_stream_id.as_uuid())
    .execute(&mut *connection)
    .await
    .map_err(capture_failed)?;

    Ok(())
}

fn capture_failed(error: sqlx::Error) -> ProductListingRawCaptureWriteError {
    match &error {
        sqlx::Error::Database(database_error)
            if database_error.constraint()
                == Some("product_listing_raw_streams_listing_source_id_fkey") =>
        {
            ProductListingRawCaptureWriteError::ListingSourceNotFound
        }
        _ => ProductListingRawCaptureWriteError::CaptureFailed {
            source: box_error(error),
        },
    }
}

fn invalid_capture_state(message: &'static str) -> ProductListingRawCaptureWriteError {
    invalid_capture_state_error(std::io::Error::other(message))
}

fn invalid_capture_state_error(
    source: impl std::error::Error + Send + Sync + 'static,
) -> ProductListingRawCaptureWriteError {
    ProductListingRawCaptureWriteError::CaptureFailed {
        source: box_error(source),
    }
}
