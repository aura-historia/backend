//! The qualification queue: the ListingSources the scope admits to crawling that wait for their
//! qualification, as the author agent works them (`docs/crawler/README.md#listingsource-qualification`).
//! A source a human labelled waits for an agent only in the one-off backfill of the sources that
//! existed before qualification did.

use crate::review::model::{
    AGENT_NEEDS_HUMAN_BEFORE_FINAL_ROUND, QueuedSourceLabel, ReviewActor,
    SourceAwaitingQualification, SourceLabel,
};
use crate::review::repository::{
    AGENT_RETRY_DUE, agent_final_round_due, listing_source_id_from_uuid,
};
use serde::Serialize;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};

/// The most sources one read of the queue answers.
pub const MAX_QUEUED_SOURCES: i64 = 100;

/// The sources waiting, new sources first, then the backfill, then the sources the agents try
/// again after their own `NEEDS_HUMAN`: a new source is not crawled until it is decided, while a
/// backfill source keeps being crawled meanwhile. A try after the agents' daily `NEEDS_HUMAN`
/// tries is their final round. Binds `$1`, the most sources, and `$2` of
/// [`agent_final_round_due`].
fn awaiting_qualification() -> String {
    let final_round_due = agent_final_round_due();
    format!(
        "SELECT s.listing_source_id, s.listing_source_name, s.crawl_root_url, s.crawl_host,
               s.qualification_backfill, s.source_label, s.source_label_note, s.source_labelled_by,
               CASE WHEN s.qualification IS NOT NULL THEN s.qualification_note END AS earlier_note,
               s.qualification IS NOT NULL AND {final_round_due} AS final_round,
               ARRAY(SELECT DISTINCT r.artifact_type FROM crawler_reviews r
                     WHERE r.listing_source_id = s.listing_source_id
                       AND r.status = 'PENDING_REVIEW'
                     ORDER BY r.artifact_type) AS pending_review_artifacts,
               COUNT(*) OVER () AS total
        FROM listing_sources s
        WHERE s.in_crawl_scope
          AND (s.qualification IS NULL OR {AGENT_RETRY_DUE})
          AND s.crawl_root_url IS NOT NULL
          AND s.crawl_host IS NOT NULL
          AND (s.source_labelled_by IS DISTINCT FROM 'HUMAN' OR s.qualification_backfill)
        ORDER BY s.qualification IS NOT NULL, s.qualification_backfill, s.created,
                 s.listing_source_id
        LIMIT $1"
    )
}

/// One read of the queue: the first sources and how many wait in all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QualificationQueue {
    pub total: i64,
    pub sources: Vec<SourceAwaitingQualification>,
}

#[derive(Clone)]
pub struct QualificationQueueReader {
    pool: PgPool,
}

impl QualificationQueueReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The first [`MAX_QUEUED_SOURCES`] sources waiting for a qualification. A stored label or
    /// actor no label or actor has fails the read closed.
    pub async fn read(&self) -> Result<QualificationQueue, sqlx::Error> {
        let rows = sqlx::query(sqlx::AssertSqlSafe(awaiting_qualification()))
            .bind(MAX_QUEUED_SOURCES)
            .bind(i64::from(AGENT_NEEDS_HUMAN_BEFORE_FINAL_ROUND))
            .fetch_all(&self.pool)
            .await?;
        Ok(QualificationQueue {
            total: rows
                .first()
                .map(|row| row.try_get("total"))
                .transpose()?
                .unwrap_or(0),
            sources: rows.iter().map(source_of_row).collect::<Result<_, _>>()?,
        })
    }
}

fn source_of_row(row: &PgRow) -> Result<SourceAwaitingQualification, sqlx::Error> {
    let invalid = |column: &str, value: &str| {
        sqlx::Error::Decode(format!("invalid persisted {column} `{value}`").into())
    };
    let source_label = match (
        row.try_get::<Option<String>, _>("source_label")?,
        row.try_get::<Option<String>, _>("source_labelled_by")?,
    ) {
        (Some(label), Some(actor)) => Some(QueuedSourceLabel {
            label: SourceLabel::from_persisted(&label)
                .ok_or_else(|| invalid("source_label", &label))?,
            note: row.try_get("source_label_note")?,
            labelled_by: ReviewActor::from_persisted(&actor)
                .ok_or_else(|| invalid("source_labelled_by", &actor))?,
        }),
        (None, None) => None,
        _ => {
            return Err(sqlx::Error::Decode(
                "source_label without source_labelled_by".into(),
            ));
        }
    };
    Ok(SourceAwaitingQualification {
        listing_source_id: listing_source_id_from_uuid(row.try_get("listing_source_id")?)?,
        listing_source_name: row.try_get("listing_source_name")?,
        crawl_root_url: row.try_get("crawl_root_url")?,
        crawl_host: row.try_get("crawl_host")?,
        backfill: row.try_get("qualification_backfill")?,
        source_label,
        pending_review_artifacts: row.try_get("pending_review_artifacts")?,
        earlier_note: row.try_get("earlier_note")?,
        final_round: row.try_get("final_round")?,
    })
}
