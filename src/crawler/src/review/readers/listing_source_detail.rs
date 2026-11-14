//! `GET /api/listing-sources/{id}`: one ListingSource with the live URL pattern, product schemas
//! and removed-page templates it crawls and scrapes by. The contract is in
//! `docs/crawler/operations.md` (review service).

use super::ListingSourceOverviewReader;
use crate::review::model::{
    FAILING_URL_EXAMPLES, FAILING_URL_MESSAGE_CHARS, FailingUrl, ListingSourceDetail,
    Qualification, QualificationEvent, ReviewActor, SourceLabel, SourceLabelEvent,
    SourceLabelEventKind, UrlPatternOverview,
};
use crate::review::repository::crawler_review_id_from_uuid;
use crate::scraper::css_selector::product_schema::decode_stored_product_schemas;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crate::scraper::removal_guard::MASS_REMOVAL_HELD_ERROR_KIND;
use crate::spider::classification::product_url_pattern::{
    ProductUrlPatternError, UrlPatternMatching,
};
use listing_source_core::ListingSourceId;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};

/// Reads the page of one ListingSource.
#[derive(Clone)]
pub struct ListingSourceDetailReader {
    pool: PgPool,
    overviews: ListingSourceOverviewReader,
}

/// The live configuration of a ListingSource and the latest approvals of its artifacts, in one
/// read. An approval is the latest one by the time it was reviewed; it is the live one unless a
/// live edit or a restore replaced it since. Binds `$1`, the ListingSource.
const LIVE_CONFIGURATION: &str = "
    SELECT s.url_pattern_state, s.url_pattern, s.url_pattern_exclude, s.url_pattern_matching,
           ps.product_schema, ps.updated AS schemas_updated,
           rp.removed_page_schema,
           pattern_review.review_id AS pattern_review_id,
           pattern_review.reviewed AS pattern_approved_at,
           (SELECT e.actor FROM crawler_review_events e
            WHERE e.review_id = pattern_review.review_id AND e.action = 'APPROVED'
            ORDER BY e.created DESC, e.event_id DESC
            LIMIT 1) AS pattern_approved_by,
           schema_review.review_id AS schema_review_id
    FROM listing_sources s
    LEFT JOIN listing_source_product_schemas ps ON ps.listing_source_id = s.listing_source_id
    LEFT JOIN listing_source_removed_page_schemas rp ON rp.listing_source_id = s.listing_source_id
    LEFT JOIN LATERAL (
        SELECT review_id, reviewed FROM crawler_reviews
        WHERE listing_source_id = s.listing_source_id
          AND artifact_type = 'URL_PATTERN' AND status = 'APPROVED'
        ORDER BY reviewed DESC NULLS LAST, review_id DESC
        LIMIT 1
    ) pattern_review ON TRUE
    LEFT JOIN LATERAL (
        SELECT review_id FROM crawler_reviews
        WHERE listing_source_id = s.listing_source_id
          AND artifact_type = 'PRODUCT_SCHEMA' AND status = 'APPROVED'
        ORDER BY reviewed DESC NULLS LAST, review_id DESC
        LIMIT 1
    ) schema_review ON TRUE
    WHERE s.listing_source_id = $1";

/// The product URLs of a ListingSource whose last scrape failed, the most repeated failures
/// first. A page that waits for a review, a removal the guard holds or a missing value is no
/// scrape failure; the overview reports those. Binds `$1`, the ListingSource, `$2`, the error
/// kind of a held removal, `$3`, the most URLs and `$4`, the longest message in characters.
const FAILING_URLS: &str = "
    SELECT url, last_error_kind, last_status_code, failure_count, next_retry_at,
           left(last_error_message, $4) AS error_message
    FROM listing_source_urls
    WHERE listing_source_id = $1
      AND url_class = 'product'
      AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
      AND failure_count > 0
      AND last_error_kind IS NOT NULL
      AND last_error_kind NOT IN ('PendingSchemaReview', 'MissingRequiredValue', $2)
    ORDER BY failure_count DESC, updated DESC, url
    LIMIT $3";

/// The label history of a ListingSource, oldest first. Binds `$1`, the ListingSource.
const LABEL_HISTORY: &str = "
    SELECT event_kind, source_label, source_label_note, actor, occurred_at
    FROM listing_source_label_events
    WHERE listing_source_id = $1
    ORDER BY occurred_at, event_id";

/// The qualification history of a ListingSource, oldest first. Binds `$1`, the ListingSource.
const QUALIFICATION_HISTORY: &str = "
    SELECT qualification, source_label, note, actor, occurred_at
    FROM listing_source_qualification_events
    WHERE listing_source_id = $1
    ORDER BY occurred_at, event_id";

impl ListingSourceDetailReader {
    pub fn new(overviews: ListingSourceOverviewReader) -> Self {
        Self {
            pool: overviews.pool().clone(),
            overviews,
        }
    }

    /// The page of `listing_source_id`, or `None` when no such ListingSource exists. A stored
    /// schema or template that does not read as one fails the page instead of showing something
    /// else.
    pub async fn find_listing_source(
        &self,
        listing_source_id: ListingSourceId,
    ) -> Result<Option<ListingSourceDetail>, sqlx::Error> {
        let Some(overview) = self
            .overviews
            .find_listing_source(listing_source_id)
            .await?
        else {
            return Ok(None);
        };
        let row = sqlx::query(LIVE_CONFIGURATION)
            .bind(listing_source_id.as_uuid())
            .fetch_one(&self.pool)
            .await?;
        let failing_urls = sqlx::query(FAILING_URLS)
            .bind(listing_source_id.as_uuid())
            .bind(MASS_REMOVAL_HELD_ERROR_KIND)
            .bind(FAILING_URL_EXAMPLES)
            .bind(FAILING_URL_MESSAGE_CHARS)
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(failing_url_of_row)
            .collect::<Result<Vec<_>, _>>()?;
        let label_history = sqlx::query(LABEL_HISTORY)
            .bind(listing_source_id.as_uuid())
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(label_event_of_row)
            .collect::<Result<Vec<_>, _>>()?;
        let qualification_history = sqlx::query(QUALIFICATION_HISTORY)
            .bind(listing_source_id.as_uuid())
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(qualification_event_of_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(detail_of_row(
            overview,
            &row,
            failing_urls,
            SourceHistories {
                labels: label_history,
                qualifications: qualification_history,
            },
        )?))
    }
}

/// The label and qualification histories of a ListingSource, oldest first.
struct SourceHistories {
    labels: Vec<SourceLabelEvent>,
    qualifications: Vec<QualificationEvent>,
}

/// One entry of the qualification history; a qualification, label or actor no entry may carry
/// fails the read.
fn qualification_event_of_row(row: &PgRow) -> Result<QualificationEvent, sqlx::Error> {
    let invalid = |column: &str, value: &str| {
        sqlx::Error::Decode(format!("invalid persisted {column} `{value}`").into())
    };
    let actor: String = row.try_get("actor")?;
    Ok(QualificationEvent {
        qualification: row
            .try_get::<Option<String>, _>("qualification")?
            .map(|name| {
                Qualification::from_persisted(&name).ok_or_else(|| invalid("qualification", &name))
            })
            .transpose()?,
        label: row
            .try_get::<Option<String>, _>("source_label")?
            .map(|name| {
                SourceLabel::from_persisted(&name).ok_or_else(|| invalid("source_label", &name))
            })
            .transpose()?,
        note: row.try_get("note")?,
        actor: ReviewActor::from_persisted(&actor).ok_or_else(|| invalid("actor", &actor))?,
        occurred_at: row.try_get("occurred_at")?,
    })
}

/// One entry of the label history; a kind, label or actor no entry may carry fails the read.
fn label_event_of_row(row: &PgRow) -> Result<SourceLabelEvent, sqlx::Error> {
    let invalid = |column: &str, value: &str| {
        sqlx::Error::Decode(format!("invalid persisted {column} `{value}`").into())
    };
    let kind: String = row.try_get("event_kind")?;
    let label = row
        .try_get::<Option<String>, _>("source_label")?
        .map(|name| {
            SourceLabel::from_persisted(&name).ok_or_else(|| invalid("source_label", &name))
        })
        .transpose()?;
    let actor: String = row.try_get("actor")?;
    Ok(SourceLabelEvent {
        event: SourceLabelEventKind::from_persisted(&kind)
            .ok_or_else(|| invalid("event_kind", &kind))?,
        label,
        note: row.try_get("source_label_note")?,
        actor: ReviewActor::from_persisted(&actor).ok_or_else(|| invalid("actor", &actor))?,
        occurred_at: row.try_get("occurred_at")?,
    })
}

fn failing_url_of_row(row: &PgRow) -> Result<FailingUrl, sqlx::Error> {
    Ok(FailingUrl {
        url: row.try_get("url")?,
        error_kind: row.try_get("last_error_kind")?,
        status_code: row.try_get("last_status_code")?,
        failure_count: row.try_get("failure_count")?,
        error_message: row.try_get("error_message")?,
        next_retry_at: row.try_get("next_retry_at")?,
    })
}

fn detail_of_row(
    overview: crate::review::model::ListingSourceOverview,
    row: &PgRow,
    failing_urls: Vec<FailingUrl>,
    histories: SourceHistories,
) -> Result<ListingSourceDetail, sqlx::Error> {
    let decode = |source: serde_json::Error| sqlx::Error::Decode(Box::new(source));
    let url_pattern = row
        .try_get::<Option<String>, _>("url_pattern")?
        .map(|value| {
            Ok::<_, sqlx::Error>(UrlPatternOverview {
                value,
                exclude: row.try_get("url_pattern_exclude")?,
                matching: UrlPatternMatching::from_persisted_column(
                    row.try_get::<Option<&str>, _>("url_pattern_matching")?,
                )
                .and_then(|matching| matching.ok_or(ProductUrlPatternError::MissingMatching))
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
                approved_review_id: row
                    .try_get::<Option<uuid::Uuid>, _>("pattern_review_id")?
                    .map(crawler_review_id_from_uuid)
                    .transpose()?,
                approved_at: row.try_get("pattern_approved_at")?,
                approved_by: row
                    .try_get::<Option<String>, _>("pattern_approved_by")?
                    .map(|name| {
                        ReviewActor::from_persisted(&name).ok_or_else(|| {
                            sqlx::Error::Decode(format!("invalid persisted actor `{name}`").into())
                        })
                    })
                    .transpose()?,
            })
        })
        .transpose()?;
    let product_schemas = row
        .try_get::<Option<serde_json::Value>, _>("product_schema")?
        .map(decode_stored_product_schemas)
        .transpose()
        .map_err(decode)?
        .unwrap_or_default();
    let removed_page_schemas = row
        .try_get::<Option<serde_json::Value>, _>("removed_page_schema")?
        .map(serde_json::from_value::<Vec<RemovedPageSchema>>)
        .transpose()
        .map_err(decode)?
        .unwrap_or_default();
    Ok(ListingSourceDetail {
        overview,
        url_pattern,
        product_schemas,
        removed_page_schemas,
        live_schema_review_id: row
            .try_get::<Option<uuid::Uuid>, _>("schema_review_id")?
            .map(crawler_review_id_from_uuid)
            .transpose()?,
        live_schemas_since: row.try_get("schemas_updated")?,
        failing_urls,
        label_history: histories.labels,
        qualification_history: histories.qualifications,
    })
}
