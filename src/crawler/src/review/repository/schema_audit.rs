//! What the live schema audit reads and writes: the ListingSources due for an audit, their live
//! schemas and captured product URLs, the findings an approval of their live schemas already
//! accepted, and the audit review it opens.

use super::schema_payload::ProductSchemaReviewCandidate;
use super::{CrawlerReviewRepository, ReviewRepositoryError, listing_source_id_from_uuid};
use crate::CrawlerReviewId;
use crate::review::captured_identity_sample::CapturedProductUrl;
use crate::review::model::SchemaReviewPageInput;
use crate::review::schema_audit::SCHEMA_AUDIT_REASON;
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use listing_source_core::ListingSourceId;
use serde_json::json;
use std::time::Duration;

/// A ListingSource due for an audit of its live schemas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaAuditSource {
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: String,
}

impl CrawlerReviewRepository {
    /// The number of audit reviews still pending, which bounds how many more the audit opens.
    pub async fn pending_schema_audit_reviews(&self) -> Result<i64, ReviewRepositoryError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM crawler_reviews
             WHERE artifact_type = 'PRODUCT_SCHEMA'
               AND status = 'PENDING_REVIEW'
               AND reason = $1",
        )
        .bind(SCHEMA_AUDIT_REASON)
        .fetch_one(&self.pool)
        .await?)
    }

    /// At most `limit` crawl-enabled ListingSources with live product schemas and no pending
    /// product-schema review whose schemas were never audited, or not within `audit_age`: those
    /// never audited first, then the longest ago.
    pub async fn schema_audit_due_sources(
        &self,
        audit_age: Duration,
        limit: i64,
    ) -> Result<Vec<SchemaAuditSource>, ReviewRepositoryError> {
        let rows: Vec<(uuid::Uuid, String)> = sqlx::query_as(
            "SELECT s.listing_source_id, s.listing_source_name
             FROM listing_sources s
             WHERE s.crawl_enabled = TRUE
               AND s.crawl_host IS NOT NULL
               AND EXISTS (
                   SELECT 1 FROM listing_source_product_schemas ps
                   WHERE ps.listing_source_id = s.listing_source_id
                     AND (jsonb_typeof(ps.product_schema) <> 'array'
                          OR jsonb_array_length(ps.product_schema) > 0))
               AND NOT EXISTS (
                   SELECT 1 FROM crawler_reviews cr
                   WHERE cr.listing_source_id = s.listing_source_id
                     AND cr.artifact_type = 'PRODUCT_SCHEMA'
                     AND cr.status = 'PENDING_REVIEW')
               AND (s.schema_audited_at IS NULL
                    OR s.schema_audited_at < NOW() - make_interval(secs => $1))
             ORDER BY s.schema_audited_at ASC NULLS FIRST, s.listing_source_id
             LIMIT $2",
        )
        .bind(audit_age.as_secs_f64())
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(listing_source_id, listing_source_name)| {
                Ok(SchemaAuditSource {
                    listing_source_id: listing_source_id_from_uuid(listing_source_id)?,
                    listing_source_name,
                })
            })
            .collect()
    }

    /// The ListingSource's live product schemas and a random sample of its captured product
    /// URLs, each with the identity its listing was captured under.
    pub async fn schema_audit_sample(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<(Vec<ProductCssSelectorSchema>, Vec<CapturedProductUrl>), ReviewRepositoryError>
    {
        let listing_source_id = uuid::Uuid::from(*listing_source_id);
        Ok((
            self.live_schemas(listing_source_id).await?,
            self.captured_product_urls(listing_source_id, &[]).await?,
        ))
    }

    /// Records that the ListingSource's live schemas were audited now, whatever the audit found.
    pub async fn record_schema_audit(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<(), ReviewRepositoryError> {
        sqlx::query(
            "UPDATE listing_sources SET schema_audited_at = NOW() WHERE listing_source_id = $1",
        )
        .bind(listing_source_id.as_uuid())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The findings the last approved audit review of the ListingSource was opened with, when
    /// that approval made exactly `live_schemas` live: a human or the reviewer agent judged the
    /// live schemas with those findings, so the audit does not reopen the source for them.
    /// `None` when no approved audit review made these schemas live.
    pub async fn accepted_schema_audit_findings(
        &self,
        listing_source_id: &ListingSourceId,
        live_schemas: &[ProductCssSelectorSchema],
    ) -> Result<Option<Vec<String>>, ReviewRepositoryError> {
        let row: Option<(serde_json::Value, serde_json::Value)> = sqlx::query_as(
            "SELECT candidate_payload, validation_summary
             FROM crawler_reviews
             WHERE listing_source_id = $1
               AND artifact_type = 'PRODUCT_SCHEMA'
               AND reason = $2
               AND status = 'APPROVED'
             ORDER BY updated DESC
             LIMIT 1",
        )
        .bind(listing_source_id.as_uuid())
        .bind(SCHEMA_AUDIT_REASON)
        .fetch_optional(&self.pool)
        .await?;
        let Some((candidate_payload, validation_summary)) = row else {
            return Ok(None);
        };
        let approved = ProductSchemaReviewCandidate::decode_stored(&candidate_payload)
            .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
        if approved.schemas != live_schemas {
            return Ok(None);
        }
        Ok(Some(audit_findings_of(&validation_summary)))
    }

    /// Opens the product-schema review of an audit that found `findings` in the ListingSource's
    /// `live_schemas` on its captured product `pages`: the candidate is the live schemas, so the
    /// review agents repair them as they stand, and the review records the findings. A review the
    /// scraper opened meanwhile is joined instead, as every schema review is.
    pub async fn open_schema_audit_review(
        &self,
        listing_source_id: &ListingSourceId,
        live_schemas: &[ProductCssSelectorSchema],
        pages: Vec<SchemaReviewPageInput>,
        findings: &[String],
    ) -> Result<CrawlerReviewId, ReviewRepositoryError> {
        let page_count = pages.len();
        self.create_schema_review(
            listing_source_id,
            SCHEMA_AUDIT_REASON,
            live_schemas,
            pages,
            json!({
                "schema_audit": {
                    "findings": findings,
                    "page_count": page_count,
                }
            }),
        )
        .await
    }
}

/// The findings an audit review's validation summary records, as `field CHECK` codes; none for
/// another review.
pub(crate) fn audit_findings_of(validation_summary: &serde_json::Value) -> Vec<String> {
    validation_summary["schema_audit"]["findings"]
        .as_array()
        .map(|findings| {
            findings
                .iter()
                .filter_map(|finding| finding.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}
