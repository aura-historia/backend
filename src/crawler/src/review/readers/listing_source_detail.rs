//! `GET /api/listing-sources/{id}`: one ListingSource with the live URL pattern, product schemas
//! and removed-page templates it crawls and scrapes by. The contract is in
//! `docs/crawler/operations.md` (review service).

use super::ListingSourceOverviewReader;
use crate::review::model::{ListingSourceDetail, ReviewActor, UrlPatternOverview};
use crate::review::repository::crawler_review_id_from_uuid;
use crate::scraper::css_selector::product_schema::decode_stored_product_schemas;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
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
    SELECT s.url_pattern_state, s.url_pattern, s.url_pattern_exclude,
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
        Ok(Some(detail_of_row(overview, &row)?))
    }
}

fn detail_of_row(
    overview: crate::review::model::ListingSourceOverview,
    row: &PgRow,
) -> Result<ListingSourceDetail, sqlx::Error> {
    let decode = |source: serde_json::Error| sqlx::Error::Decode(Box::new(source));
    let url_pattern = row
        .try_get::<Option<String>, _>("url_pattern")?
        .map(|value| {
            Ok::<_, sqlx::Error>(UrlPatternOverview {
                value,
                exclude: row.try_get("url_pattern_exclude")?,
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
    })
}
