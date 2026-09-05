use application::error::box_error;
use product_listing_service::ports::{
    ProductListingRawCaptureWrite, ProductListingRawCaptureWriteError,
    ProductListingRawCaptureWriteOutcome, ProductListingRawCaptureWriter,
    ProductListingRawCaptureWriterFactory, ProductListingRawRevisionId, ProductListingRawStreamId,
};
use sqlx::PgConnection;

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
        let listing_source_id = uuid::Uuid::from(write.listing_source_id);
        let source_record_key_sha256 = write.source_record_key_sha256.as_bytes().as_slice();

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
        .bind(uuid::Uuid::new_v4())
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
                latest_input_sha256
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

        if stream.source_record_key != write.source_record_key {
            return Err(ProductListingRawCaptureWriteError::SourceRecordKeyHashCollision);
        }

        let latest_revision = u64::try_from(stream.latest_revision)
            .map_err(|_| invalid_capture_state("raw stream revision is invalid"))?;
        if stream.latest_input_sha256.as_deref() == Some(write.input_sha256.as_bytes().as_slice()) {
            return Ok(ProductListingRawCaptureWriteOutcome::Unchanged {
                product_listing_raw_stream_id: ProductListingRawStreamId::from_uuid(
                    stream.product_listing_raw_stream_id,
                ),
                latest_revision,
            });
        }

        let revision = latest_revision
            .checked_add(1)
            .ok_or_else(|| invalid_capture_state("raw stream revision overflow"))?;
        let revision_as_i64 = i64::try_from(revision)
            .map_err(|_| invalid_capture_state("raw stream revision exceeds storage range"))?;
        let product_listing_raw_revision_id = uuid::Uuid::new_v4();

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
        .bind(product_listing_raw_revision_id)
        .bind(stream.product_listing_raw_stream_id)
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

        sqlx::query(
            r#"
            UPDATE product_listing_raw_streams
            SET latest_revision = $1,
                latest_input_sha256 = $2,
                updated = now()
            WHERE product_listing_raw_stream_id = $3
            "#,
        )
        .bind(revision_as_i64)
        .bind(write.input_sha256.as_bytes().as_slice())
        .bind(stream.product_listing_raw_stream_id)
        .execute(&mut *self.connection)
        .await
        .map_err(capture_failed)?;

        Ok(ProductListingRawCaptureWriteOutcome::Changed {
            product_listing_raw_stream_id: ProductListingRawStreamId::from_uuid(
                stream.product_listing_raw_stream_id,
            ),
            product_listing_raw_revision_id: ProductListingRawRevisionId::from_uuid(
                product_listing_raw_revision_id,
            ),
            revision,
        })
    }
}

fn capture_failed(error: sqlx::Error) -> ProductListingRawCaptureWriteError {
    ProductListingRawCaptureWriteError::CaptureFailed {
        source: box_error(error),
    }
}

fn invalid_capture_state(message: &'static str) -> ProductListingRawCaptureWriteError {
    ProductListingRawCaptureWriteError::CaptureFailed {
        source: box_error(std::io::Error::other(message)),
    }
}
