//! PostgreSQL storage for reviewed per-source description boilerplate and the raw descriptions
//! its candidate analysis reads.

use crate::object_id::try_from_uuid;
use application::error::box_error;
use listing_source_core::ListingSourceId;
use product_listing_normalization::{BoilerplateSentence, DescriptionBoilerplate};
use product_service::ports::{
    CurrentRawDescription, CurrentRawDescriptionCursor, CurrentRawDescriptionReader,
    DescriptionBoilerplateDecision, DescriptionBoilerplatePortError, DescriptionBoilerplateReader,
    DescriptionBoilerplateReview, DescriptionBoilerplateReviewWriter,
};
use sqlx::PgPool;

/// Pool-backed reader and review writer.
#[derive(Debug, Clone)]
pub struct SqlxDescriptionBoilerplateStore {
    pool: PgPool,
}

impl SqlxDescriptionBoilerplateStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct ReviewRow {
    listing_source_id: uuid::Uuid,
    sentence: String,
    decision: String,
}

#[derive(Debug, sqlx::FromRow)]
struct CurrentRawDescriptionRow {
    listing_source_id: uuid::Uuid,
    product_listing_raw_stream_id: uuid::Uuid,
    fragments: serde_json::Value,
}

#[async_trait::async_trait]
impl DescriptionBoilerplateReader for SqlxDescriptionBoilerplateStore {
    async fn strip_set(
        &self,
        listing_source_id: ListingSourceId,
    ) -> Result<DescriptionBoilerplate, DescriptionBoilerplatePortError> {
        let sentences = sqlx::query_scalar::<_, String>(
            r#"
            SELECT sentence
            FROM listing_source_description_boilerplate
            WHERE listing_source_id = $1 AND decision = 'STRIP'
            "#,
        )
        .bind(listing_source_id.as_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(persistence)?;
        sentences
            .iter()
            .map(|sentence| stored_sentence(sentence))
            .collect()
    }

    async fn reviews(
        &self,
        listing_source_id: Option<ListingSourceId>,
    ) -> Result<Vec<DescriptionBoilerplateReview>, DescriptionBoilerplatePortError> {
        let rows = sqlx::query_as::<_, ReviewRow>(
            r#"
            SELECT listing_source_id, sentence, decision
            FROM listing_source_description_boilerplate
            WHERE $1::uuid IS NULL OR listing_source_id = $1
            ORDER BY listing_source_id, sentence
            "#,
        )
        .bind(listing_source_id.map(|id| id.into_uuid()))
        .fetch_all(&self.pool)
        .await
        .map_err(persistence)?;
        rows.into_iter()
            .map(|row| {
                Ok(DescriptionBoilerplateReview {
                    listing_source_id: try_from_uuid(row.listing_source_id, "ListingSource ID")
                        .map_err(invalid_state_error)?,
                    sentence: stored_sentence(&row.sentence)?,
                    decision: DescriptionBoilerplateDecision::from_code(&row.decision)
                        .ok_or_else(|| invalid_state("boilerplate decision is unknown"))?,
                })
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl DescriptionBoilerplateReviewWriter for SqlxDescriptionBoilerplateStore {
    async fn record(
        &self,
        reviews: Vec<DescriptionBoilerplateReview>,
    ) -> Result<(), DescriptionBoilerplatePortError> {
        let mut tx = self.pool.begin().await.map_err(persistence)?;
        for review in reviews {
            sqlx::query(
                r#"
                INSERT INTO listing_source_description_boilerplate
                    (listing_source_id, sentence, decision)
                VALUES ($1, $2, $3)
                ON CONFLICT (listing_source_id, sentence) DO UPDATE
                SET decision = EXCLUDED.decision, reviewed_at = now()
                "#,
            )
            .bind(review.listing_source_id.as_uuid())
            .bind(review.sentence.as_str())
            .bind(review.decision.as_str())
            .execute(&mut *tx)
            .await
            .map_err(persistence)?;
        }
        tx.commit().await.map_err(persistence)
    }
}

#[async_trait::async_trait]
impl CurrentRawDescriptionReader for SqlxDescriptionBoilerplateStore {
    async fn page(
        &self,
        listing_source_id: Option<ListingSourceId>,
        after: Option<CurrentRawDescriptionCursor>,
        limit: u32,
    ) -> Result<Vec<CurrentRawDescription>, DescriptionBoilerplatePortError> {
        let rows = sqlx::query_as::<_, CurrentRawDescriptionRow>(
            r#"
            SELECT
                stream.listing_source_id,
                stream.product_listing_raw_stream_id,
                revision.raw_values -> 'description' -> 'value' AS fragments
            FROM product_listing_raw_streams AS stream
            JOIN product_listing_raw_revisions AS revision
              ON revision.product_listing_raw_stream_id = stream.product_listing_raw_stream_id
             AND revision.revision = stream.latest_revision
            WHERE revision.operation = 'UPSERT'
              AND revision.payload_format = 'CRAWLER_EXTRACTED_PRODUCT'
              AND revision.raw_values -> 'description' ->> 'action' = 'SET'
              AND ($1::uuid IS NULL OR stream.listing_source_id = $1)
              AND ($2::uuid IS NULL
                   OR (stream.listing_source_id, stream.product_listing_raw_stream_id) > ($2, $3::uuid))
            ORDER BY stream.listing_source_id, stream.product_listing_raw_stream_id
            LIMIT $4
            "#,
        )
        .bind(listing_source_id.map(|id| id.into_uuid()))
        .bind(after.map(|cursor| cursor.listing_source_id.into_uuid()))
        .bind(after.map(|cursor| cursor.product_listing_raw_stream_id.into_uuid()))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(persistence)?;
        rows.into_iter()
            .map(|row| {
                Ok(CurrentRawDescription {
                    listing_source_id: try_from_uuid(row.listing_source_id, "ListingSource ID")
                        .map_err(invalid_state_error)?,
                    product_listing_raw_stream_id: try_from_uuid(
                        row.product_listing_raw_stream_id,
                        "ProductListing raw stream ID",
                    )
                    .map_err(invalid_state_error)?,
                    fragments: serde_json::from_value(row.fragments).ok(),
                })
            })
            .collect()
    }
}

/// A stored sentence must already be the normalized key, or it could never match.
fn stored_sentence(sentence: &str) -> Result<BoilerplateSentence, DescriptionBoilerplatePortError> {
    let normalized = BoilerplateSentence::new(sentence).map_err(invalid_state_error)?;
    if normalized.as_str() != sentence {
        return Err(invalid_state("boilerplate sentence is not normalized"));
    }
    Ok(normalized)
}

fn persistence(
    error: impl std::error::Error + Send + Sync + 'static,
) -> DescriptionBoilerplatePortError {
    DescriptionBoilerplatePortError::Persistence {
        source: box_error(error),
    }
}

fn invalid_state_error(
    error: impl std::error::Error + Send + Sync + 'static,
) -> DescriptionBoilerplatePortError {
    DescriptionBoilerplatePortError::InvalidPersistedState {
        source: box_error(error),
    }
}

fn invalid_state(message: &'static str) -> DescriptionBoilerplatePortError {
    invalid_state_error(std::io::Error::other(message))
}
