use domain_primitives::event_id::EventId;
use platform_postgres::SqlxTransaction;
use product_listing_service::ports::{
    ProductEmbeddingWrite, ProductEmbeddingWriteError, ProductEmbeddingWriteOutcome,
    ProductEmbeddingWriter, ProductEmbeddingWriterFactory,
};
use serde_json::json;
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductEmbeddingWriterFactory;

struct SqlxProductEmbeddingWriter<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, thiserror::Error)]
#[error("product embedding SQL write failed")]
struct ProductEmbeddingWriteSqlxError(#[source] sqlx::Error);

impl SqlxProductEmbeddingWriterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductEmbeddingWriterFactory<SqlxTransaction> for SqlxProductEmbeddingWriterFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductEmbeddingWriter + 'tx {
        SqlxProductEmbeddingWriter {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductEmbeddingWriter for SqlxProductEmbeddingWriter<'_> {
    async fn apply(
        &mut self,
        write: &ProductEmbeddingWrite,
    ) -> Result<ProductEmbeddingWriteOutcome, ProductEmbeddingWriteError> {
        let current_event_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT event_id FROM products WHERE product_id = $1 FOR UPDATE",
        )
        .bind(uuid::Uuid::from(write.product_id))
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(ProductEmbeddingWriteSqlxError)?;
        let Some(current_event_id) = current_event_id else {
            return Ok(ProductEmbeddingWriteOutcome::ProductNotFound);
        };
        if EventId::from(current_event_id) != write.source_event_id {
            return Ok(
                if duplicate_embedding_exists(self.connection, write).await? {
                    ProductEmbeddingWriteOutcome::Duplicate
                } else {
                    ProductEmbeddingWriteOutcome::Stale
                },
            );
        }

        let payload = json!({
            "kind": "embedded",
            "sourceEventId": write.source_event_id.to_string(),
            "embedding": write.embedding,
            "title": {
                "language": write.title.localization.as_str(),
                "text": write.title.payload.as_ref(),
            },
        });
        sqlx::query(
            r#"
            INSERT INTO product_events (
                event_id, product_id, event_type, event_group, payload, event_time
            ) VALUES ($1, $2, 'ENRICHMENT_EMBEDDED', 'ENRICHMENT', $3, now())
        "#,
        )
        .bind(uuid::Uuid::from(write.enrichment_event_id))
        .bind(uuid::Uuid::from(write.product_id))
        .bind(payload)
        .execute(&mut *self.connection)
        .await
        .map_err(ProductEmbeddingWriteSqlxError)?;
        let update = sqlx::query(
            "UPDATE products SET embedding = $1, event_id = $2, projection_version = projection_version + 1, updated = now() WHERE product_id = $3 AND event_id = $4",
        )
        .bind(&write.embedding)
        .bind(uuid::Uuid::from(write.enrichment_event_id))
        .bind(uuid::Uuid::from(write.product_id))
        .bind(uuid::Uuid::from(write.source_event_id))
        .execute(&mut *self.connection).await.map_err(ProductEmbeddingWriteSqlxError)?;
        if update.rows_affected() != 1 {
            return Err(ProductEmbeddingWriteError::WriteFailed {
                source: application::error::static_error(
                    "locked product embedding source revision changed unexpectedly",
                ),
            });
        }
        Ok(ProductEmbeddingWriteOutcome::Applied)
    }
}

async fn duplicate_embedding_exists(
    connection: &mut PgConnection,
    write: &ProductEmbeddingWrite,
) -> Result<bool, ProductEmbeddingWriteError> {
    sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM product_events
            WHERE product_id = $1
              AND event_type = 'ENRICHMENT_EMBEDDED'
              AND event_group = 'ENRICHMENT'
              AND payload ->> 'sourceEventId' = $2
        )
    "#,
    )
    .bind(uuid::Uuid::from(write.product_id))
    .bind(write.source_event_id.to_string())
    .fetch_one(&mut *connection)
    .await
    .map_err(ProductEmbeddingWriteSqlxError)
    .map_err(Into::into)
}

impl From<ProductEmbeddingWriteSqlxError> for ProductEmbeddingWriteError {
    fn from(source: ProductEmbeddingWriteSqlxError) -> Self {
        Self::WriteFailed {
            source: application::error::box_error(source),
        }
    }
}
