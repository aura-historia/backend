use crate::object_id::try_from_uuid;
use crate::product_listing_embedding_queue::db_input_version;
use domain_primitives::event_id::EventId;
use platform_postgres::SqlxTransaction;
use product_listing_service::ports::{
    ProductListingEmbeddingConfirmOutcome, ProductListingEmbeddingConfirmation,
    ProductListingEmbeddingWrite, ProductListingEmbeddingWriteError,
    ProductListingEmbeddingWriteOutcome, ProductListingEmbeddingWriter,
    ProductListingEmbeddingWriterFactory,
};
use serde_json::json;
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingEmbeddingWriterFactory;

struct SqlxProductListingEmbeddingWriter<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, thiserror::Error)]
#[error("product embedding SQL write failed")]
struct ProductListingEmbeddingWriteSqlxError(#[source] sqlx::Error);

impl SqlxProductListingEmbeddingWriterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingEmbeddingWriterFactory<SqlxTransaction>
    for SqlxProductListingEmbeddingWriterFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductListingEmbeddingWriter + 'tx {
        SqlxProductListingEmbeddingWriter {
            connection: tx.connection(),
        }
    }
}

impl SqlxProductListingEmbeddingWriter<'_> {
    /// Locks the listing, which serializes every write of its vector and recorded input, and
    /// returns its embedding revision.
    async fn lock_embedding_source(
        &mut self,
        product_listing_id: &uuid::Uuid,
    ) -> Result<Option<uuid::Uuid>, ProductListingEmbeddingWriteSqlxError> {
        sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT embedding_source_event_id FROM product_listings WHERE product_listing_id = $1 FOR UPDATE",
        )
        .bind(product_listing_id)
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(ProductListingEmbeddingWriteSqlxError)
    }

    /// The revision the listing's vector stands for and whether its input has an image; `None`
    /// without a vector or a recorded input of the current input version.
    async fn embedded_input(
        &mut self,
        product_listing_id: &uuid::Uuid,
    ) -> Result<Option<(uuid::Uuid, bool)>, ProductListingEmbeddingWriteSqlxError> {
        sqlx::query_as::<_, (uuid::Uuid, bool)>(
            r#"
            SELECT input.source_event_id, input.image_url IS NOT NULL
            FROM product_listing_embedding_inputs input
            JOIN product_listings listing USING (product_listing_id)
            WHERE input.product_listing_id = $1
              AND input.input_version = $2
              AND listing.embedding IS NOT NULL
            "#,
        )
        .bind(product_listing_id)
        .bind(db_input_version())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(ProductListingEmbeddingWriteSqlxError)
    }

    /// Records the listing's current title, description and, when the vector was computed with
    /// it, first image as the input behind its vector.
    async fn record_input(
        &mut self,
        write: &ProductListingEmbeddingWrite,
    ) -> Result<(), ProductListingEmbeddingWriteSqlxError> {
        sqlx::query(
            r#"
            INSERT INTO product_listing_embedding_inputs (
                product_listing_id, source_event_id, input_version, title_text, description_text,
                image_url, image_hash
            )
            SELECT product_listing_id, embedding_source_event_id, $4, title_text, description_text,
                   CASE WHEN $2 THEN product_images -> 0 ->> 'url' END,
                   CASE WHEN $2 AND product_images -> 0 ->> 'url' IS NOT NULL THEN $3::bytea END
            FROM product_listings
            WHERE product_listing_id = $1
            ON CONFLICT (product_listing_id) DO UPDATE SET
                source_event_id = EXCLUDED.source_event_id,
                input_version = EXCLUDED.input_version,
                title_text = EXCLUDED.title_text,
                description_text = EXCLUDED.description_text,
                image_url = EXCLUDED.image_url,
                image_hash = EXCLUDED.image_hash
            "#,
        )
        .bind(write.product_listing_id.as_uuid())
        .bind(write.image.is_some())
        .bind(
            write
                .image
                .and_then(|image| image.hash)
                .map(|hash| hash.as_bytes().to_vec()),
        )
        .bind(db_input_version())
        .execute(&mut *self.connection)
        .await
        .map_err(ProductListingEmbeddingWriteSqlxError)?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl ProductListingEmbeddingWriter for SqlxProductListingEmbeddingWriter<'_> {
    async fn apply(
        &mut self,
        write: &ProductListingEmbeddingWrite,
    ) -> Result<ProductListingEmbeddingWriteOutcome, ProductListingEmbeddingWriteError> {
        let listing = write.product_listing_id.as_uuid();
        let Some(embedding_source_event_id) = self.lock_embedding_source(listing).await? else {
            return Ok(ProductListingEmbeddingWriteOutcome::ProductListingNotFound);
        };
        if self
            .embedded_input(listing)
            .await?
            .is_some_and(|(embedded, _)| embedded == *write.source_event_id.as_uuid())
        {
            return Ok(ProductListingEmbeddingWriteOutcome::Duplicate);
        }
        let embedding_source_event_id =
            try_from_uuid::<EventId>(embedding_source_event_id, "embedding source event ID")
                .map_err(|source| ProductListingEmbeddingWriteError::WriteFailed {
                    source: application::error::box_error(source),
                })?;
        if embedding_source_event_id != write.source_event_id {
            return Ok(ProductListingEmbeddingWriteOutcome::Stale);
        }

        let payload = json!({
            "sourceEventId": write.source_event_id.as_uuid().to_string(),
        });
        sqlx::query(
            r#"
            INSERT INTO product_listing_events (
                event_id, product_listing_id, event_type, event_group, event_type_schema_version,
                payload, event_time
            ) VALUES ($1, $2, 'ENRICHMENT_EMBEDDED', 'ENRICHMENT', 1, $3, now())
        "#,
        )
        .bind(write.enrichment_event_id.as_uuid())
        .bind(write.product_listing_id.as_uuid())
        .bind(payload)
        .execute(&mut *self.connection)
        .await
        .map_err(ProductListingEmbeddingWriteSqlxError)?;
        let update = sqlx::query(
            "UPDATE product_listings SET embedding = $1, current_event_id = $2, projection_version = projection_version + 1, updated = aura_advance_updated(updated) WHERE product_listing_id = $3 AND embedding_source_event_id = $4",
        )
        .bind(&write.embedding)
        .bind(write.enrichment_event_id.as_uuid())
        .bind(write.product_listing_id.as_uuid())
        .bind(write.source_event_id.as_uuid())
        .execute(&mut *self.connection).await.map_err(ProductListingEmbeddingWriteSqlxError)?;
        if update.rows_affected() != 1 {
            return Err(ProductListingEmbeddingWriteError::WriteFailed {
                source: application::error::static_error(
                    "locked product embedding source revision changed unexpectedly",
                ),
            });
        }
        self.record_input(write).await?;
        Ok(ProductListingEmbeddingWriteOutcome::Applied)
    }

    async fn confirm(
        &mut self,
        confirmation: &ProductListingEmbeddingConfirmation,
    ) -> Result<ProductListingEmbeddingConfirmOutcome, ProductListingEmbeddingWriteError> {
        let listing = confirmation.product_listing_id.as_uuid();
        let revision = confirmation.source_event_id.as_uuid();
        if self.lock_embedding_source(listing).await?.as_ref() != Some(revision) {
            return Ok(ProductListingEmbeddingConfirmOutcome::Superseded);
        }
        let Some((embedded, has_image)) = self.embedded_input(listing).await? else {
            return Ok(ProductListingEmbeddingConfirmOutcome::EmbeddedInputChanged);
        };
        if embedded == *revision {
            return Ok(ProductListingEmbeddingConfirmOutcome::Confirmed);
        }
        // The decision kept the image presence; an input that differs was replaced meanwhile.
        if embedded != *confirmation.embedded_source_event_id.as_uuid()
            || has_image != confirmation.image_url.is_some()
        {
            return Ok(ProductListingEmbeddingConfirmOutcome::EmbeddedInputChanged);
        }
        // Writers of the input hold the listing lock; the guard also covers one that does not.
        let confirmed = sqlx::query(
            r#"
            UPDATE product_listing_embedding_inputs SET source_event_id = $2, image_url = $3
            WHERE product_listing_id = $1 AND source_event_id = $4 AND input_version = $5
            "#,
        )
        .bind(listing)
        .bind(revision)
        .bind(confirmation.image_url.as_ref().map(url::Url::as_str))
        .bind(embedded)
        .bind(db_input_version())
        .execute(&mut *self.connection)
        .await
        .map_err(ProductListingEmbeddingWriteSqlxError)?;
        Ok(if confirmed.rows_affected() == 1 {
            ProductListingEmbeddingConfirmOutcome::Confirmed
        } else {
            ProductListingEmbeddingConfirmOutcome::EmbeddedInputChanged
        })
    }
}

impl From<ProductListingEmbeddingWriteSqlxError> for ProductListingEmbeddingWriteError {
    fn from(source: ProductListingEmbeddingWriteSqlxError) -> Self {
        Self::WriteFailed {
            source: application::error::box_error(source),
        }
    }
}
