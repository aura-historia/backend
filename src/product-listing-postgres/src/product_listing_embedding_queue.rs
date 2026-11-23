//! The product embedding queue in `product_listing_embedding_work` and
//! `product_listing_embedding_batches`. Work counts as current only while its listing holds its
//! embedding revision and a title; the source-marker guard of the embedding writer still decides
//! every commit.

use crate::object_id::try_from_uuid;
use crate::product_listing_embedding_source_reader::{
    first_image_url, localized_description, localized_title,
};
use application::error::{box_error, static_error};
use platform_postgres::SqlxTransaction;
use product_listing_service::ports::{
    BatchedProductEmbedding, EmbeddingBatchId, PendingProductEmbedding, ProductEmbeddingAssignment,
    ProductEmbeddingBatchKey, ProductEmbeddingClaim, ProductEmbeddingScheduleOutcome,
    ProductEmbeddingWorkRef, ProductListingEmbeddingQueue, ProductListingEmbeddingQueueError,
    ProductListingEmbeddingQueueFactory, ProductListingEmbeddingQueueReader,
    SubmittedProductEmbeddingBatch,
};
use sqlx::{PgConnection, PgPool};
use std::time::Duration;

type QueueResult<T> = Result<T, ProductListingEmbeddingQueueError>;

/// Selects current work: the work row joined with its listing at the work's revision.
macro_rules! select_current_work {
    () => {
        "SELECT work.product_listing_id, work.source_event_id, work.attempts,
                work.batch_position, listing.title_text, listing.title_language,
                listing.description_text, listing.description_language, listing.product_images
         FROM product_listing_embedding_work work
         JOIN product_listings listing
           ON listing.product_listing_id = work.product_listing_id
          AND listing.embedding_source_event_id = work.source_event_id
          AND listing.title_text <> ''
          AND listing.title_language IS NOT NULL"
    };
}

/// Matches the work `$1`/`$2` only while it is still where the claim `$3` took it from: in the
/// batch `$3`, or with `$3` NULL, pending.
macro_rules! claimed_work {
    () => {
        "WHERE product_listing_id = $1 AND source_event_id = $2
           AND (batch_id = $3 OR ($3::uuid IS NULL AND batch_id IS NULL AND parked IS NULL))"
    };
}

#[derive(Debug, thiserror::Error)]
#[error("product embedding queue SQL query failed")]
struct EmbeddingQueueSqlxError(#[source] sqlx::Error);

fn sql(source: sqlx::Error) -> ProductListingEmbeddingQueueError {
    ProductListingEmbeddingQueueError::QueryFailed {
        source: box_error(EmbeddingQueueSqlxError(source)),
    }
}

fn invalid_state(message: &'static str) -> ProductListingEmbeddingQueueError {
    ProductListingEmbeddingQueueError::InvalidPersistedState {
        source: static_error(message),
    }
}

#[derive(sqlx::FromRow)]
struct CurrentWorkRow {
    product_listing_id: uuid::Uuid,
    source_event_id: uuid::Uuid,
    attempts: i16,
    batch_position: Option<i32>,
    title_text: String,
    title_language: String,
    description_text: Option<String>,
    description_language: Option<String>,
    product_images: serde_json::Value,
}

impl CurrentWorkRow {
    fn pending(self) -> QueueResult<PendingProductEmbedding> {
        let mapping = |source| ProductListingEmbeddingQueueError::InvalidPersistedState {
            source: box_error(source),
        };
        let title = localized_title(Some(self.title_text), Some(self.title_language))
            .map_err(invalid_state)?
            .ok_or_else(|| invalid_state("persisted title of embedding work is empty"))?;
        let description = localized_description(self.description_text, self.description_language)
            .map_err(invalid_state)?
            .map(|description| description.payload)
            .filter(|description| !description.as_ref().is_empty());
        Ok(PendingProductEmbedding {
            work: ProductEmbeddingWorkRef {
                product_listing_id: try_from_uuid(self.product_listing_id, "ProductListing ID")
                    .map_err(mapping)?,
                source_event_id: try_from_uuid(self.source_event_id, "event ID")
                    .map_err(mapping)?,
            },
            title: title.payload,
            description,
            image_url: first_image_url(self.product_images).map_err(invalid_state)?,
            attempts: u16::try_from(self.attempts)
                .map_err(|_| invalid_state("persisted embedding attempts are invalid"))?,
        })
    }

    fn batched(self) -> QueueResult<BatchedProductEmbedding> {
        let position = self
            .batch_position
            .and_then(|position| u32::try_from(position).ok())
            .ok_or_else(|| invalid_state("batched embedding work has no valid position"))?;
        Ok(BatchedProductEmbedding {
            position,
            embedding: self.pending()?,
        })
    }
}

fn to_db_attempts(attempts: u16) -> QueueResult<i16> {
    i16::try_from(attempts).map_err(|_| invalid_state("embedding attempts are out of range"))
}

/// The batch a claim names, or `NULL` for pending work; bound as `$3` of `claimed_work`.
fn claimed_batch(claim: ProductEmbeddingClaim) -> Option<uuid::Uuid> {
    match claim {
        ProductEmbeddingClaim::Batch(key) => Some(*key.as_uuid()),
        ProductEmbeddingClaim::Pending => None,
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingEmbeddingQueueFactory;

impl SqlxProductListingEmbeddingQueueFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingEmbeddingQueueFactory<SqlxTransaction>
    for SqlxProductListingEmbeddingQueueFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductListingEmbeddingQueue + 'tx {
        SqlxProductListingEmbeddingQueue {
            connection: tx.connection(),
        }
    }
}

struct SqlxProductListingEmbeddingQueue<'tx> {
    connection: &'tx mut PgConnection,
}

#[async_trait::async_trait]
impl ProductListingEmbeddingQueue for SqlxProductListingEmbeddingQueue<'_> {
    async fn schedule(
        &mut self,
        work: ProductEmbeddingWorkRef,
    ) -> QueueResult<ProductEmbeddingScheduleOutcome> {
        // The share lock keeps the revision current until this transaction commits, so a newer
        // revision's job always schedules after this one.
        let current = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT embedding_source_event_id FROM product_listings WHERE product_listing_id = $1 FOR SHARE",
        )
        .bind(work.product_listing_id.as_uuid())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(sql)?;
        if current.as_ref() != Some(work.source_event_id.as_uuid()) {
            return Ok(ProductEmbeddingScheduleOutcome::Superseded);
        }
        let embedded = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM product_listing_events
                WHERE product_listing_id = $1
                  AND event_type = 'ENRICHMENT_EMBEDDED'
                  AND event_group = 'ENRICHMENT'
                  AND event_type_schema_version = 1
                  AND payload ->> 'sourceEventId' = $2
            )
            "#,
        )
        .bind(work.product_listing_id.as_uuid())
        .bind(work.source_event_id.as_uuid().to_string())
        .fetch_one(&mut *self.connection)
        .await
        .map_err(sql)?;
        if embedded {
            return Ok(ProductEmbeddingScheduleOutcome::AlreadyEmbedded);
        }
        // A redelivered job of the same revision leaves its work, and any batch it is in, alone.
        sqlx::query(
            r#"
            INSERT INTO product_listing_embedding_work (product_listing_id, source_event_id)
            VALUES ($1, $2)
            ON CONFLICT (product_listing_id) DO UPDATE SET
                source_event_id = EXCLUDED.source_event_id,
                attempts = 0,
                parked = NULL,
                batch_id = NULL,
                batch_position = NULL,
                enqueued = now()
            WHERE product_listing_embedding_work.source_event_id <> EXCLUDED.source_event_id
            "#,
        )
        .bind(work.product_listing_id.as_uuid())
        .bind(work.source_event_id.as_uuid())
        .execute(&mut *self.connection)
        .await
        .map_err(sql)?;
        Ok(ProductEmbeddingScheduleOutcome::Scheduled)
    }

    async fn discard_superseded(&mut self) -> QueueResult<u64> {
        let deleted = sqlx::query(
            r#"
            DELETE FROM product_listing_embedding_work work
            WHERE work.batch_id IS NULL
              AND NOT EXISTS (
                  SELECT 1
                  FROM product_listings listing
                  WHERE listing.product_listing_id = work.product_listing_id
                    AND listing.embedding_source_event_id = work.source_event_id
                    AND listing.title_language IS NOT NULL
                    AND listing.title_text <> ''
              )
            "#,
        )
        .execute(&mut *self.connection)
        .await
        .map_err(sql)?;
        Ok(deleted.rows_affected())
    }

    async fn lock_pending(&mut self, limit: u32) -> QueueResult<Vec<PendingProductEmbedding>> {
        sqlx::query_as::<_, CurrentWorkRow>(concat!(
            select_current_work!(),
            "
            WHERE work.batch_id IS NULL AND work.parked IS NULL
            ORDER BY work.enqueued, work.product_listing_id
            LIMIT $1
            FOR UPDATE OF work SKIP LOCKED
            "
        ))
        .bind(i64::from(limit))
        .fetch_all(&mut *self.connection)
        .await
        .map_err(sql)?
        .into_iter()
        .map(CurrentWorkRow::pending)
        .collect()
    }

    async fn create_batch(
        &mut self,
        key: ProductEmbeddingBatchKey,
        assignments: &[ProductEmbeddingAssignment],
    ) -> QueueResult<()> {
        sqlx::query("INSERT INTO product_listing_embedding_batches (batch_id) VALUES ($1)")
            .bind(key.as_uuid())
            .execute(&mut *self.connection)
            .await
            .map_err(sql)?;
        let mut listings = Vec::with_capacity(assignments.len());
        let mut events = Vec::with_capacity(assignments.len());
        let mut positions = Vec::with_capacity(assignments.len());
        for assignment in assignments {
            listings.push(*assignment.work.product_listing_id.as_uuid());
            events.push(*assignment.work.source_event_id.as_uuid());
            positions.push(
                i32::try_from(assignment.position)
                    .map_err(|_| invalid_state("batch position is out of range"))?,
            );
        }
        let assigned = sqlx::query(
            r#"
            UPDATE product_listing_embedding_work work
            SET batch_id = $1, batch_position = assignment.batch_position
            FROM UNNEST($2::uuid[], $3::uuid[], $4::integer[])
                AS assignment(product_listing_id, source_event_id, batch_position)
            WHERE work.product_listing_id = assignment.product_listing_id
              AND work.source_event_id = assignment.source_event_id
              AND work.batch_id IS NULL
              AND work.parked IS NULL
            "#,
        )
        .bind(key.as_uuid())
        .bind(&listings)
        .bind(&events)
        .bind(&positions)
        .execute(&mut *self.connection)
        .await
        .map_err(sql)?;
        if assigned.rows_affected() != assignments.len() as u64 {
            return Err(invalid_state(
                "locked embedding work changed before its batch assignment",
            ));
        }
        Ok(())
    }

    async fn record_submission(
        &mut self,
        key: ProductEmbeddingBatchKey,
        provider_batch_id: &EmbeddingBatchId,
    ) -> QueueResult<()> {
        let recorded = sqlx::query(
            r#"
            UPDATE product_listing_embedding_batches
            SET provider_batch_id = $2, submitted = now()
            WHERE batch_id = $1 AND provider_batch_id IS NULL
            "#,
        )
        .bind(key.as_uuid())
        .bind(provider_batch_id.as_str())
        .execute(&mut *self.connection)
        .await
        .map_err(sql)?;
        if recorded.rows_affected() != 1 {
            return Err(invalid_state(
                "embedding batch was closed before its submission was recorded",
            ));
        }
        Ok(())
    }

    async fn close_batch(&mut self, key: ProductEmbeddingBatchKey) -> QueueResult<()> {
        sqlx::query(
            "UPDATE product_listing_embedding_work SET batch_id = NULL, batch_position = NULL WHERE batch_id = $1",
        )
        .bind(key.as_uuid())
        .execute(&mut *self.connection)
        .await
        .map_err(sql)?;
        sqlx::query("DELETE FROM product_listing_embedding_batches WHERE batch_id = $1")
            .bind(key.as_uuid())
            .execute(&mut *self.connection)
            .await
            .map_err(sql)?;
        Ok(())
    }

    async fn close_abandoned_submissions(&mut self, older_than: Duration) -> QueueResult<u64> {
        let abandoned = sqlx::query_scalar::<_, uuid::Uuid>(
            r#"
            SELECT batch_id
            FROM product_listing_embedding_batches
            WHERE provider_batch_id IS NULL
              AND created < now() - make_interval(secs => $1)
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .bind(older_than.as_secs_f64())
        .fetch_all(&mut *self.connection)
        .await
        .map_err(sql)?;
        for batch in &abandoned {
            self.close_batch(ProductEmbeddingBatchKey::from_uuid(*batch))
                .await?;
        }
        Ok(abandoned.len() as u64)
    }

    async fn complete(
        &mut self,
        work: ProductEmbeddingWorkRef,
        claim: ProductEmbeddingClaim,
    ) -> QueueResult<()> {
        sqlx::query(concat!(
            "DELETE FROM product_listing_embedding_work ",
            claimed_work!()
        ))
        .bind(work.product_listing_id.as_uuid())
        .bind(work.source_event_id.as_uuid())
        .bind(claimed_batch(claim))
        .execute(&mut *self.connection)
        .await
        .map_err(sql)?;
        Ok(())
    }

    async fn requeue(
        &mut self,
        work: ProductEmbeddingWorkRef,
        claim: ProductEmbeddingClaim,
        attempts: u16,
    ) -> QueueResult<()> {
        sqlx::query(concat!(
            "UPDATE product_listing_embedding_work
             SET attempts = $4, batch_id = NULL, batch_position = NULL ",
            claimed_work!()
        ))
        .bind(work.product_listing_id.as_uuid())
        .bind(work.source_event_id.as_uuid())
        .bind(claimed_batch(claim))
        .bind(to_db_attempts(attempts)?)
        .execute(&mut *self.connection)
        .await
        .map_err(sql)?;
        Ok(())
    }

    async fn park(
        &mut self,
        work: ProductEmbeddingWorkRef,
        claim: ProductEmbeddingClaim,
        attempts: u16,
    ) -> QueueResult<()> {
        sqlx::query(concat!(
            "UPDATE product_listing_embedding_work
             SET parked = now(), attempts = $4, batch_id = NULL, batch_position = NULL ",
            claimed_work!()
        ))
        .bind(work.product_listing_id.as_uuid())
        .bind(work.source_event_id.as_uuid())
        .bind(claimed_batch(claim))
        .bind(to_db_attempts(attempts)?)
        .execute(&mut *self.connection)
        .await
        .map_err(sql)?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct SqlxProductListingEmbeddingQueueReader {
    pool: PgPool,
}

impl SqlxProductListingEmbeddingQueueReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ProductListingEmbeddingQueueReader for SqlxProductListingEmbeddingQueueReader {
    async fn submitted_batches(&self) -> QueueResult<Vec<SubmittedProductEmbeddingBatch>> {
        sqlx::query_as::<_, (uuid::Uuid, String, f64)>(
            r#"
            SELECT batch_id, provider_batch_id,
                   GREATEST(EXTRACT(EPOCH FROM now() - submitted), 0)::float8
            FROM product_listing_embedding_batches
            WHERE provider_batch_id IS NOT NULL
            ORDER BY submitted, batch_id
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(sql)?
        .into_iter()
        .map(|(key, provider_batch_id, age)| {
            Ok(SubmittedProductEmbeddingBatch {
                key: ProductEmbeddingBatchKey::from_uuid(key),
                provider_batch_id: EmbeddingBatchId::new(provider_batch_id)
                    .map_err(|_| invalid_state("persisted provider batch ID is invalid"))?,
                age: Duration::try_from_secs_f64(age)
                    .map_err(|_| invalid_state("persisted batch submission time is invalid"))?,
            })
        })
        .collect()
    }

    async fn batch_items(
        &self,
        key: ProductEmbeddingBatchKey,
    ) -> QueueResult<Vec<BatchedProductEmbedding>> {
        sqlx::query_as::<_, CurrentWorkRow>(concat!(
            select_current_work!(),
            "
            WHERE work.batch_id = $1
            ORDER BY work.batch_position
            "
        ))
        .bind(key.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(sql)?
        .into_iter()
        .map(CurrentWorkRow::batched)
        .collect()
    }

    async fn pending(&self, limit: u32) -> QueueResult<Vec<PendingProductEmbedding>> {
        sqlx::query_as::<_, CurrentWorkRow>(concat!(
            select_current_work!(),
            "
            WHERE work.batch_id IS NULL AND work.parked IS NULL
            ORDER BY work.enqueued, work.product_listing_id
            LIMIT $1
            "
        ))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(sql)?
        .into_iter()
        .map(CurrentWorkRow::pending)
        .collect()
    }
}

/// Age of the oldest embedding work that is neither committed nor parked, pending or in a
/// batch, for the host's health metrics; `None` without such work.
pub async fn oldest_unembedded_age(
    connection: &mut PgConnection,
) -> Result<Option<Duration>, sqlx::Error> {
    let age = sqlx::query_scalar::<_, Option<f64>>(
        r#"
        SELECT EXTRACT(EPOCH FROM now() - min(enqueued))::float8
        FROM product_listing_embedding_work
        WHERE parked IS NULL
        "#,
    )
    .fetch_one(connection)
    .await?;
    // A clock step can make the age negative; it counts as none.
    Ok(age.map(|age| Duration::try_from_secs_f64(age).unwrap_or_default()))
}
