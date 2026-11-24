//! The title translation queue in `product_listing_title_translation_work` and
//! `product_listing_title_translation_batches`. Work counts as current only while its listing
//! holds its content revision and a title; the source-marker guard of the translation writer
//! still decides every commit.

use crate::object_id::try_from_uuid;
use application::error::{box_error, static_error};
use localization::Language;
use platform_postgres::SqlxTransaction;
use product_listing_service::ports::{
    BatchedTitleTranslation, PendingTitleTranslation, ProductListingTitleTranslationQueue,
    ProductListingTitleTranslationQueueError, ProductListingTitleTranslationQueueFactory,
    ProductListingTitleTranslationQueueReader, SubmittedTitleTranslationBatch,
    TitleTranslationAssignment, TitleTranslationBatchId, TitleTranslationBatchKey,
    TitleTranslationClaim, TitleTranslationMode, TitleTranslationScheduleOutcome,
    TitleTranslationWorkRef,
};
use sqlx::{PgConnection, PgPool};
use std::time::Duration;

type QueueResult<T> = Result<T, ProductListingTitleTranslationQueueError>;

/// Selects current work: the work row joined with its listing at the work's revision.
macro_rules! select_current_work {
    () => {
        "SELECT work.product_listing_id, work.source_event_id, work.mode, work.attempts,
                work.batch_group, work.batch_position, listing.title_text, listing.title_language
         FROM product_listing_title_translation_work work
         JOIN product_listings listing
           ON listing.product_listing_id = work.product_listing_id
          AND listing.content_source_event_id = work.source_event_id
          AND listing.title_text IS NOT NULL
          AND listing.title_language IS NOT NULL"
    };
}

/// Matches the work `$1`/`$2` only while it is still where the claim `$3` took it from: in the
/// batch `$3`, or with `$3` NULL, waiting for the refusal fallback.
macro_rules! claimed_work {
    () => {
        "WHERE product_listing_id = $1 AND source_event_id = $2
           AND (batch_id = $3
                OR ($3::uuid IS NULL AND batch_id IS NULL AND parked IS NULL
                    AND mode = 'REFUSAL_FALLBACK'))"
    };
}

macro_rules! unassigned {
    () => {
        "batch_id = NULL, batch_group = NULL, batch_position = NULL"
    };
}

#[derive(Debug, thiserror::Error)]
#[error("product title translation queue SQL query failed")]
struct TitleTranslationQueueSqlxError(#[source] sqlx::Error);

impl From<TitleTranslationQueueSqlxError> for ProductListingTitleTranslationQueueError {
    fn from(source: TitleTranslationQueueSqlxError) -> Self {
        Self::QueryFailed {
            source: box_error(source),
        }
    }
}

fn sql(source: sqlx::Error) -> ProductListingTitleTranslationQueueError {
    TitleTranslationQueueSqlxError(source).into()
}

fn invalid_state(message: &'static str) -> ProductListingTitleTranslationQueueError {
    ProductListingTitleTranslationQueueError::InvalidPersistedState {
        source: static_error(message),
    }
}

fn mode_to_db(mode: TitleTranslationMode) -> &'static str {
    match mode {
        TitleTranslationMode::Grouped => "GROUPED",
        TitleTranslationMode::Single => "SINGLE",
        TitleTranslationMode::RefusalFallback => "REFUSAL_FALLBACK",
    }
}

fn mode_from_db(value: &str) -> QueueResult<TitleTranslationMode> {
    match value {
        "GROUPED" => Ok(TitleTranslationMode::Grouped),
        "SINGLE" => Ok(TitleTranslationMode::Single),
        "REFUSAL_FALLBACK" => Ok(TitleTranslationMode::RefusalFallback),
        _ => Err(invalid_state("persisted title translation mode is invalid")),
    }
}

#[derive(sqlx::FromRow)]
struct CurrentWorkRow {
    product_listing_id: uuid::Uuid,
    source_event_id: uuid::Uuid,
    mode: String,
    attempts: i16,
    batch_group: Option<i32>,
    batch_position: Option<i32>,
    title_text: String,
    title_language: String,
}

impl CurrentWorkRow {
    fn pending(self) -> QueueResult<PendingTitleTranslation> {
        let mapping = |source| ProductListingTitleTranslationQueueError::InvalidPersistedState {
            source: box_error(source),
        };
        let title = crate::title::decode_nonempty_title(&self.title_text)
            .ok()
            .flatten()
            .ok_or_else(|| invalid_state("persisted title of translation work is invalid"))?;
        Ok(PendingTitleTranslation {
            work: TitleTranslationWorkRef {
                product_listing_id: try_from_uuid(self.product_listing_id, "ProductListing ID")
                    .map_err(mapping)?,
                source_event_id: try_from_uuid(self.source_event_id, "event ID")
                    .map_err(mapping)?,
            },
            title,
            source_language: Language::from_code(&self.title_language)
                .ok_or_else(|| invalid_state("persisted title language is invalid"))?,
            mode: mode_from_db(&self.mode)?,
            attempts: u16::try_from(self.attempts)
                .map_err(|_| invalid_state("persisted translation attempts are invalid"))?,
        })
    }

    fn batched(self) -> QueueResult<BatchedTitleTranslation> {
        let (Some(group), Some(position)) = (self.batch_group, self.batch_position) else {
            return Err(invalid_state("batched translation work has no position"));
        };
        let place = |value: i32| {
            u32::try_from(value)
                .map_err(|_| invalid_state("batched translation position is invalid"))
        };
        Ok(BatchedTitleTranslation {
            group: place(group)?,
            position: place(position)?,
            translation: self.pending()?,
        })
    }
}

fn to_db_attempts(attempts: u16) -> QueueResult<i16> {
    i16::try_from(attempts).map_err(|_| invalid_state("translation attempts are out of range"))
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingTitleTranslationQueueFactory;

impl SqlxProductListingTitleTranslationQueueFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingTitleTranslationQueueFactory<SqlxTransaction>
    for SqlxProductListingTitleTranslationQueueFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductListingTitleTranslationQueue + 'tx {
        SqlxProductListingTitleTranslationQueue {
            connection: tx.connection(),
        }
    }
}

struct SqlxProductListingTitleTranslationQueue<'tx> {
    connection: &'tx mut PgConnection,
}

#[async_trait::async_trait]
impl ProductListingTitleTranslationQueue for SqlxProductListingTitleTranslationQueue<'_> {
    async fn schedule(
        &mut self,
        work: TitleTranslationWorkRef,
    ) -> QueueResult<TitleTranslationScheduleOutcome> {
        // The share lock keeps the revision current until this transaction commits, so a newer
        // revision's job always schedules after this one.
        let current = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT content_source_event_id FROM product_listings WHERE product_listing_id = $1 FOR SHARE",
        )
        .bind(work.product_listing_id.as_uuid())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(sql)?;
        if current.as_ref() != Some(work.source_event_id.as_uuid()) {
            return Ok(TitleTranslationScheduleOutcome::Superseded);
        }
        let translated = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM product_listing_events
                WHERE product_listing_id = $1
                  AND event_type = 'ENRICHMENT_TRANSLATED_TITLES'
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
        if translated {
            return Ok(TitleTranslationScheduleOutcome::AlreadyTranslated);
        }
        // A redelivered job of the same revision leaves its work, and any batch it is in, alone.
        sqlx::query(concat!(
            "
            INSERT INTO product_listing_title_translation_work (product_listing_id, source_event_id)
            VALUES ($1, $2)
            ON CONFLICT (product_listing_id) DO UPDATE SET
                source_event_id = EXCLUDED.source_event_id,
                mode = 'GROUPED',
                attempts = 0,
                parked = NULL,
                ",
            unassigned!(),
            ",
                enqueued = now()
            WHERE product_listing_title_translation_work.source_event_id <> EXCLUDED.source_event_id
            "
        ))
        .bind(work.product_listing_id.as_uuid())
        .bind(work.source_event_id.as_uuid())
        .execute(&mut *self.connection)
        .await
        .map_err(sql)?;
        Ok(TitleTranslationScheduleOutcome::Scheduled)
    }

    async fn discard_superseded(&mut self) -> QueueResult<u64> {
        let deleted = sqlx::query(
            r#"
            DELETE FROM product_listing_title_translation_work work
            WHERE work.batch_id IS NULL
              AND NOT EXISTS (
                  SELECT 1
                  FROM product_listings listing
                  WHERE listing.product_listing_id = work.product_listing_id
                    AND listing.content_source_event_id = work.source_event_id
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

    async fn lock_pending(&mut self, limit: u32) -> QueueResult<Vec<PendingTitleTranslation>> {
        sqlx::query_as::<_, CurrentWorkRow>(concat!(
            select_current_work!(),
            "
            WHERE work.batch_id IS NULL
              AND work.parked IS NULL
              AND work.mode IN ('GROUPED', 'SINGLE')
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
        key: TitleTranslationBatchKey,
        assignments: &[TitleTranslationAssignment],
    ) -> QueueResult<()> {
        sqlx::query("INSERT INTO product_listing_title_translation_batches (batch_id) VALUES ($1)")
            .bind(key.as_uuid())
            .execute(&mut *self.connection)
            .await
            .map_err(sql)?;
        let place = |value: u32| {
            i32::try_from(value).map_err(|_| invalid_state("batch position is out of range"))
        };
        let mut listings = Vec::with_capacity(assignments.len());
        let mut events = Vec::with_capacity(assignments.len());
        let mut groups = Vec::with_capacity(assignments.len());
        let mut positions = Vec::with_capacity(assignments.len());
        for assignment in assignments {
            listings.push(*assignment.work.product_listing_id.as_uuid());
            events.push(*assignment.work.source_event_id.as_uuid());
            groups.push(place(assignment.group)?);
            positions.push(place(assignment.position)?);
        }
        let assigned = sqlx::query(
            r#"
            UPDATE product_listing_title_translation_work work
            SET batch_id = $1, batch_group = assignment.batch_group,
                batch_position = assignment.batch_position
            FROM UNNEST($2::uuid[], $3::uuid[], $4::integer[], $5::integer[])
                AS assignment(product_listing_id, source_event_id, batch_group, batch_position)
            WHERE work.product_listing_id = assignment.product_listing_id
              AND work.source_event_id = assignment.source_event_id
              AND work.batch_id IS NULL
              AND work.parked IS NULL
            "#,
        )
        .bind(key.as_uuid())
        .bind(&listings)
        .bind(&events)
        .bind(&groups)
        .bind(&positions)
        .execute(&mut *self.connection)
        .await
        .map_err(sql)?;
        if assigned.rows_affected() != assignments.len() as u64 {
            return Err(invalid_state(
                "locked translation work changed before its batch assignment",
            ));
        }
        Ok(())
    }

    async fn record_submission(
        &mut self,
        key: TitleTranslationBatchKey,
        provider_batch_id: &TitleTranslationBatchId,
    ) -> QueueResult<()> {
        let recorded = sqlx::query(
            r#"
            UPDATE product_listing_title_translation_batches
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
                "translation batch was closed before its submission was recorded",
            ));
        }
        Ok(())
    }

    async fn close_batch(&mut self, key: TitleTranslationBatchKey) -> QueueResult<()> {
        sqlx::query(concat!(
            "UPDATE product_listing_title_translation_work SET ",
            unassigned!(),
            " WHERE batch_id = $1"
        ))
        .bind(key.as_uuid())
        .execute(&mut *self.connection)
        .await
        .map_err(sql)?;
        sqlx::query("DELETE FROM product_listing_title_translation_batches WHERE batch_id = $1")
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
            FROM product_listing_title_translation_batches
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
            self.close_batch(TitleTranslationBatchKey::from_uuid(*batch))
                .await?;
        }
        Ok(abandoned.len() as u64)
    }

    async fn complete(
        &mut self,
        work: TitleTranslationWorkRef,
        claim: TitleTranslationClaim,
    ) -> QueueResult<()> {
        sqlx::query(concat!(
            "DELETE FROM product_listing_title_translation_work ",
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
        work: TitleTranslationWorkRef,
        claim: TitleTranslationClaim,
        mode: TitleTranslationMode,
        attempts: u16,
    ) -> QueueResult<()> {
        sqlx::query(concat!(
            "UPDATE product_listing_title_translation_work SET mode = $4, attempts = $5, ",
            unassigned!(),
            " ",
            claimed_work!()
        ))
        .bind(work.product_listing_id.as_uuid())
        .bind(work.source_event_id.as_uuid())
        .bind(claimed_batch(claim))
        .bind(mode_to_db(mode))
        .bind(to_db_attempts(attempts)?)
        .execute(&mut *self.connection)
        .await
        .map_err(sql)?;
        Ok(())
    }

    async fn park(
        &mut self,
        work: TitleTranslationWorkRef,
        claim: TitleTranslationClaim,
        attempts: u16,
    ) -> QueueResult<()> {
        sqlx::query(concat!(
            "UPDATE product_listing_title_translation_work SET parked = now(), attempts = $4, ",
            unassigned!(),
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

/// The batch a claim names, or `NULL` for the refusal fallback; bound as `$3` of
/// `claimed_work`.
fn claimed_batch(claim: TitleTranslationClaim) -> Option<uuid::Uuid> {
    match claim {
        TitleTranslationClaim::Batch(key) => Some(*key.as_uuid()),
        TitleTranslationClaim::RefusalFallback => None,
    }
}

#[derive(Clone)]
pub struct SqlxProductListingTitleTranslationQueueReader {
    pool: PgPool,
}

impl SqlxProductListingTitleTranslationQueueReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ProductListingTitleTranslationQueueReader for SqlxProductListingTitleTranslationQueueReader {
    async fn submitted_batches(&self) -> QueueResult<Vec<SubmittedTitleTranslationBatch>> {
        sqlx::query_as::<_, (uuid::Uuid, String)>(
            r#"
            SELECT batch_id, provider_batch_id
            FROM product_listing_title_translation_batches
            WHERE provider_batch_id IS NOT NULL
            ORDER BY submitted, batch_id
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(sql)?
        .into_iter()
        .map(|(key, provider_batch_id)| {
            Ok(SubmittedTitleTranslationBatch {
                key: TitleTranslationBatchKey::from_uuid(key),
                provider_batch_id: TitleTranslationBatchId::new(provider_batch_id)
                    .map_err(|_| invalid_state("persisted provider batch ID is invalid"))?,
            })
        })
        .collect()
    }

    async fn batch_items(
        &self,
        key: TitleTranslationBatchKey,
    ) -> QueueResult<Vec<BatchedTitleTranslation>> {
        sqlx::query_as::<_, CurrentWorkRow>(concat!(
            select_current_work!(),
            "
            WHERE work.batch_id = $1
            ORDER BY work.batch_group, work.batch_position
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

    async fn refusal_fallbacks(&self, limit: u32) -> QueueResult<Vec<PendingTitleTranslation>> {
        sqlx::query_as::<_, CurrentWorkRow>(concat!(
            select_current_work!(),
            "
            WHERE work.batch_id IS NULL
              AND work.parked IS NULL
              AND work.mode = 'REFUSAL_FALLBACK'
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_persist_every_mode_under_its_stable_name() {
        for mode in [
            TitleTranslationMode::Grouped,
            TitleTranslationMode::Single,
            TitleTranslationMode::RefusalFallback,
        ] {
            assert_eq!(mode, mode_from_db(mode_to_db(mode)).unwrap());
        }
        assert!(mode_from_db("grouped").is_err());
    }
}
