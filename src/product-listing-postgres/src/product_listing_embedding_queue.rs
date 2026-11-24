//! The product embedding queue in `product_listing_embedding_work` and
//! `product_listing_embedding_batches`. Work counts as current only while its listing holds its
//! embedding revision and a title; the source-marker guard of the embedding writer still decides
//! every commit. Current work carries the input recorded for the listing's vector in
//! `product_listing_embedding_inputs`, which decides whether its revision is embedded again.

use crate::object_id::try_from_uuid;
use crate::product_listing_embedding_source_reader::{
    first_image_url, localized_description, localized_title,
};
use application::error::{box_error, static_error};
use platform_postgres::SqlxTransaction;
use product_listing_core::{description::Description, title::Title};
use product_listing_service::ports::{
    BatchedProductEmbedding, EmbeddedImage, EmbeddedProductImage, EmbeddedProductInput,
    EmbeddingBatchId, ImageHash, PendingProductEmbedding, ProductEmbeddingAssignment,
    ProductEmbeddingBatchKey, ProductEmbeddingClaim, ProductEmbeddingScheduleOutcome,
    ProductEmbeddingWorkRef, ProductListingEmbeddingQueue, ProductListingEmbeddingQueueError,
    ProductListingEmbeddingQueueFactory, ProductListingEmbeddingQueueReader, StagedProductImage,
    SubmittedProductEmbeddingBatch,
};
use product_listing_service::use_cases::{
    EMBEDDING_INPUT_VERSION, MAX_EMBEDDING_ATTEMPTS as MAX_ATTEMPTS,
};
use sqlx::{PgConnection, PgPool};
use std::time::Duration;
use url::Url;

type QueueResult<T> = Result<T, ProductListingEmbeddingQueueError>;

/// Selects current work: the work row joined with its listing at the work's revision, and the
/// input recorded for the listing's vector, if it has one.
macro_rules! select_current_work {
    () => {
        "SELECT work.product_listing_id, work.source_event_id, work.attempts,
                work.batch_position, work.image_staged, work.image_hash,
                listing.title_text, listing.title_language,
                listing.description_text, listing.description_language, listing.product_images,
                input.source_event_id AS embedded_source_event_id,
                input.input_version AS embedded_input_version,
                input.title_text AS embedded_title_text,
                input.description_text AS embedded_description_text,
                input.image_url AS embedded_image_url,
                input.image_hash AS embedded_image_hash
         FROM product_listing_embedding_work work
         JOIN product_listings listing
           ON listing.product_listing_id = work.product_listing_id
          AND listing.embedding_source_event_id = work.source_event_id
          AND listing.title_text <> ''
          AND listing.title_language IS NOT NULL
         LEFT JOIN product_listing_embedding_inputs input
           ON input.product_listing_id = work.product_listing_id
          AND listing.embedding IS NOT NULL"
    };
}

/// Resets what a submission recorded about the work's image, wherever work leaves its batch.
macro_rules! unstaged {
    () => {
        "batch_id = NULL, batch_position = NULL, image_staged = false, image_hash = NULL"
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
    image_staged: bool,
    image_hash: Option<Vec<u8>>,
    title_text: String,
    title_language: String,
    description_text: Option<String>,
    description_language: Option<String>,
    product_images: serde_json::Value,
    embedded_source_event_id: Option<uuid::Uuid>,
    embedded_input_version: Option<i16>,
    embedded_title_text: Option<String>,
    embedded_description_text: Option<String>,
    embedded_image_url: Option<String>,
    embedded_image_hash: Option<Vec<u8>>,
}

/// Work whose persisted document no provider could be given; it is parked or skipped alone.
struct InvalidWork {
    work: ProductEmbeddingWorkRef,
    reason: &'static str,
}

impl InvalidWork {
    fn log(&self) {
        tracing::warn!(
            event = "product.embedding.work_invalid",
            product_listing_id = %self.work.product_listing_id,
            source_event_id = %self.work.source_event_id,
            reason = self.reason,
            "Persisted embedding document is invalid; the work is parked"
        );
    }
}

/// A row's work, or `Err` for invalid identifiers, which fail the whole read closed; inside,
/// the mapped item or the work whose document is invalid.
type MappedRow<T> = QueueResult<Result<T, InvalidWork>>;

impl CurrentWorkRow {
    fn work(&self) -> QueueResult<ProductEmbeddingWorkRef> {
        let mapping = |source| ProductListingEmbeddingQueueError::InvalidPersistedState {
            source: box_error(source),
        };
        Ok(ProductEmbeddingWorkRef {
            product_listing_id: try_from_uuid(self.product_listing_id, "ProductListing ID")
                .map_err(mapping)?,
            source_event_id: try_from_uuid(self.source_event_id, "event ID").map_err(mapping)?,
        })
    }

    fn pending(self) -> MappedRow<PendingProductEmbedding> {
        let work = self.work()?;
        let attempts = u16::try_from(self.attempts)
            .map_err(|_| invalid_state("persisted embedding attempts are invalid"))?;
        let embedded = self.embedded(work);
        Ok(self
            .document()
            .map(|(title, description, image_url)| PendingProductEmbedding {
                work,
                title,
                description,
                image_url,
                attempts,
                embedded,
            })
            .map_err(|reason| InvalidWork { work, reason }))
    }

    /// The input recorded for the listing's vector. A record of another input version, or an
    /// invalid one, counts as none, so the revision is embedded again and the record replaced.
    fn embedded(&self, work: ProductEmbeddingWorkRef) -> Option<EmbeddedProductInput> {
        let source_event_id = self.embedded_source_event_id?;
        if self.embedded_input_version != Some(db_input_version()) {
            return None;
        }
        let input = (|| {
            let image = match (&self.embedded_image_url, &self.embedded_image_hash) {
                (None, None) => None,
                (Some(url), hash) => Some(EmbeddedProductImage {
                    url: Url::parse(url).map_err(|_| "recorded image URL is invalid")?,
                    hash: hash
                        .as_deref()
                        .map(|hash| {
                            ImageHash::from_bytes(hash).ok_or("recorded image hash is invalid")
                        })
                        .transpose()?,
                }),
                (None, Some(_)) => return Err("recorded image hash has no URL"),
            };
            Ok(EmbeddedProductInput {
                source_event_id: try_from_uuid(source_event_id, "event ID")
                    .map_err(|_| "recorded revision is invalid")?,
                title: self
                    .embedded_title_text
                    .clone()
                    .ok_or("recorded title is missing")?,
                description: self.embedded_description_text.clone(),
                image,
            })
        })();
        input
            .inspect_err(|reason| {
                tracing::warn!(
                    event = "product.embedding.input_invalid",
                    product_listing_id = %work.product_listing_id,
                    reason,
                    "Recorded embedding input is invalid; the revision is embedded again"
                );
            })
            .ok()
    }

    fn staged_image(&self) -> Option<StagedProductImage> {
        self.image_staged.then(|| StagedProductImage {
            hash: self.image_hash.as_deref().and_then(ImageHash::from_bytes),
        })
    }

    fn document(self) -> Result<(Title, Option<Description>, Option<Url>), &'static str> {
        let title = localized_title(Some(self.title_text), Some(self.title_language))?
            .ok_or("persisted title of embedding work is empty")?;
        let description = localized_description(self.description_text, self.description_language)?
            .map(|description| description.payload)
            .filter(|description| !description.as_ref().is_empty());
        Ok((
            title.payload,
            description,
            first_image_url(self.product_images)?,
        ))
    }

    fn batched(self) -> MappedRow<BatchedProductEmbedding> {
        let position = self
            .batch_position
            .and_then(|position| u32::try_from(position).ok())
            .ok_or_else(|| invalid_state("batched embedding work has no valid position"))?;
        let staged_image = self.staged_image();
        Ok(self.pending()?.map(|embedding| BatchedProductEmbedding {
            position,
            embedding,
            staged_image,
        }))
    }
}

/// The valid items of a read; invalid work is logged and left out.
fn valid<T>(rows: Vec<MappedRow<T>>) -> QueueResult<Vec<T>> {
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        match row? {
            Ok(item) => items.push(item),
            Err(invalid) => invalid.log(),
        }
    }
    Ok(items)
}

/// The current input version as stored; it is a small constant.
pub(crate) fn db_input_version() -> i16 {
    i16::try_from(EMBEDDING_INPUT_VERSION).unwrap_or(i16::MAX)
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
                FROM product_listing_embedding_inputs input
                JOIN product_listings listing USING (product_listing_id)
                WHERE input.product_listing_id = $1
                  AND input.source_event_id = $2
                  AND input.input_version = $3
                  AND listing.embedding IS NOT NULL
            )
            "#,
        )
        .bind(work.product_listing_id.as_uuid())
        .bind(work.source_event_id.as_uuid())
        .bind(db_input_version())
        .fetch_one(&mut *self.connection)
        .await
        .map_err(sql)?;
        if embedded {
            return Ok(ProductEmbeddingScheduleOutcome::AlreadyEmbedded);
        }
        // A redelivered job of the same revision leaves its work, and any batch it is in, alone.
        sqlx::query(concat!(
            "INSERT INTO product_listing_embedding_work (product_listing_id, source_event_id)
             VALUES ($1, $2)
             ON CONFLICT (product_listing_id) DO UPDATE SET
                 source_event_id = EXCLUDED.source_event_id,
                 attempts = 0,
                 parked = NULL,
                 enqueued = now(), ",
            unstaged!(),
            " WHERE product_listing_embedding_work.source_event_id <> EXCLUDED.source_event_id"
        ))
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

    /// Work whose document is invalid is parked here, alone, so it cannot hold back the queue.
    async fn lock_pending(&mut self, limit: u32) -> QueueResult<Vec<PendingProductEmbedding>> {
        let rows = sqlx::query_as::<_, CurrentWorkRow>(concat!(
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
        .map_err(sql)?;
        let mut pending = Vec::with_capacity(rows.len());
        for row in rows {
            match row.pending()? {
                Ok(item) => pending.push(item),
                Err(invalid) => {
                    invalid.log();
                    self.park(invalid.work, ProductEmbeddingClaim::Pending, MAX_ATTEMPTS)
                        .await?;
                }
            }
        }
        Ok(pending)
    }

    async fn create_batch(
        &mut self,
        key: ProductEmbeddingBatchKey,
        assignments: &[ProductEmbeddingAssignment],
    ) -> QueueResult<()> {
        sqlx::query(
            "INSERT INTO product_listing_embedding_batches (batch_id, input_version) VALUES ($1, $2)",
        )
        .bind(key.as_uuid())
        .bind(db_input_version())
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
        images: &[EmbeddedImage],
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
        let mut positions = Vec::with_capacity(images.len());
        let mut hashes = Vec::with_capacity(images.len());
        for image in images {
            positions.push(
                i32::try_from(image.key)
                    .map_err(|_| invalid_state("batch position is out of range"))?,
            );
            hashes.push(image.hash.map(|hash| hash.as_bytes().to_vec()));
        }
        sqlx::query(
            r#"
            UPDATE product_listing_embedding_work work
            SET image_staged = true, image_hash = image.hash
            FROM UNNEST($2::integer[], $3::bytea[]) AS image(batch_position, hash)
            WHERE work.batch_id = $1 AND work.batch_position = image.batch_position
            "#,
        )
        .bind(key.as_uuid())
        .bind(&positions)
        .bind(&hashes)
        .execute(&mut *self.connection)
        .await
        .map_err(sql)?;
        Ok(())
    }

    async fn close_batch(&mut self, key: ProductEmbeddingBatchKey) -> QueueResult<()> {
        sqlx::query(concat!(
            "UPDATE product_listing_embedding_work SET ",
            unstaged!(),
            " WHERE batch_id = $1"
        ))
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
            "UPDATE product_listing_embedding_work SET attempts = $4, ",
            unstaged!(),
            " ",
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
            "UPDATE product_listing_embedding_work SET parked = now(), attempts = $4, ",
            unstaged!(),
            " ",
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
        sqlx::query_as::<_, (uuid::Uuid, String, f64, i16)>(
            r#"
            SELECT batch_id, provider_batch_id,
                   GREATEST(EXTRACT(EPOCH FROM now() - submitted), 0)::float8, input_version
            FROM product_listing_embedding_batches
            WHERE provider_batch_id IS NOT NULL
            ORDER BY submitted, batch_id
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(sql)?
        .into_iter()
        .map(|(key, provider_batch_id, age, input_version)| {
            Ok(SubmittedProductEmbeddingBatch {
                key: ProductEmbeddingBatchKey::from_uuid(key),
                provider_batch_id: EmbeddingBatchId::new(provider_batch_id)
                    .map_err(|_| invalid_state("persisted provider batch ID is invalid"))?,
                age: Duration::try_from_secs_f64(age)
                    .map_err(|_| invalid_state("persisted batch submission time is invalid"))?,
                input_version: u16::try_from(input_version)
                    .map_err(|_| invalid_state("persisted batch input version is invalid"))?,
            })
        })
        .collect()
    }

    async fn batch_items(
        &self,
        key: ProductEmbeddingBatchKey,
    ) -> QueueResult<Vec<BatchedProductEmbedding>> {
        let rows = sqlx::query_as::<_, CurrentWorkRow>(concat!(
            select_current_work!(),
            "
            WHERE work.batch_id = $1
            ORDER BY work.batch_position
            "
        ))
        .bind(key.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(sql)?;
        valid(rows.into_iter().map(CurrentWorkRow::batched).collect())
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
