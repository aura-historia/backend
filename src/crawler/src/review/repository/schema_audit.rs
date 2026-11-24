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
    /// What a human asked the audit to have repaired, when a human requested the audit.
    pub request_note: Option<String>,
}

/// The note of a human's request for an audit: what the review agents should repair, trimmed,
/// 1 to [`Self::MAX_CHARS`] characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaAuditRequestNote(String);

/// A request note that is blank or longer than [`SchemaAuditRequestNote::MAX_CHARS`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a schema audit request needs a note of 1 to {max} characters", max = SchemaAuditRequestNote::MAX_CHARS)]
pub struct InvalidSchemaAuditRequestNote;

impl SchemaAuditRequestNote {
    pub const MAX_CHARS: usize = 500;

    pub fn parse(note: &str) -> Result<Self, InvalidSchemaAuditRequestNote> {
        let note = note.trim();
        if note.is_empty() || note.chars().count() > Self::MAX_CHARS {
            return Err(InvalidSchemaAuditRequestNote);
        }
        Ok(Self(note.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether the ListingSource `listing_sources s` waits for an audit a human requested: the request
/// is not settled, and no audit ran since it, or the last one, which opened no review the request
/// was settled by, ran more than a day ago. An audit review settles the request once it carries the
/// request's note, was opened after the request, and was approved, or rejected by a human; an
/// agent's rejection settles nothing, and a request that arrives while the source is audited is
/// not settled by a review that does not carry its note.
macro_rules! requested_audit_due {
    () => {
        "(s.schema_audit_requested_at IS NOT NULL
          AND NOT EXISTS (
              SELECT 1 FROM crawler_reviews settled
              WHERE settled.listing_source_id = s.listing_source_id
                AND settled.artifact_type = 'PRODUCT_SCHEMA'
                AND settled.reason = 'schema_audit_required'
                AND settled.created >= s.schema_audit_requested_at
                AND settled.validation_summary->'schema_audit'->>'request_note'
                    = s.schema_audit_request_note
                AND (settled.status = 'APPROVED'
                     OR (settled.status = 'REJECTED'
                         AND EXISTS (
                             SELECT 1 FROM crawler_review_events e
                             WHERE e.review_id = settled.review_id
                               AND e.action = 'REJECTED'
                               AND e.actor = 'HUMAN'))))
          AND (s.schema_audited_at IS NULL
               OR s.schema_audited_at < s.schema_audit_requested_at
               OR s.schema_audited_at < NOW() - INTERVAL '1 day'))"
    };
}

/// What an audit review records: the audit's findings as `field CHECK` codes, a human's request
/// note, and whether the source's never-captured listings wait for the repair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaAuditOpening<'a> {
    pub findings: &'a [String],
    pub request_note: Option<&'a str>,
    pub holds_new_listings: bool,
}

/// Where an audit's review went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaAuditReview {
    /// The audit opened its own review, which records its findings.
    Opened(CrawlerReviewId),
    /// A product-schema review the scraper opened for the source meanwhile was pending, so the
    /// audit opened none; the source is due again once that review is decided.
    Deferred,
}

impl CrawlerReviewRepository {
    /// The number of audit reviews pending that were opened within `audit_age`, which bounds how
    /// many more the audit opens. An older one waits for a human and no longer holds the audit of
    /// other sources back.
    pub async fn pending_schema_audit_reviews(
        &self,
        audit_age: Duration,
    ) -> Result<i64, ReviewRepositoryError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM crawler_reviews
             WHERE artifact_type = 'PRODUCT_SCHEMA'
               AND status = 'PENDING_REVIEW'
               AND reason = $1
               AND created > NOW() - make_interval(secs => $2)",
        )
        .bind(SCHEMA_AUDIT_REASON)
        .bind(audit_age.as_secs_f64())
        .fetch_one(&self.pool)
        .await?)
    }

    /// At most `limit` crawl-enabled ListingSources with live product schemas and no pending
    /// product-schema review whose schemas a human asked to have audited, or that were never
    /// audited or not within `audit_age`: those a human asked for first, then those never audited,
    /// then the longest ago. A source whose scrape circuit cools down is left to the scraper, and
    /// one whose audit review was decided within `audit_age` is not due unless a human asks, so a
    /// review decided long after it was opened is not reopened at once.
    pub async fn schema_audit_due_sources(
        &self,
        audit_age: Duration,
        limit: i64,
    ) -> Result<Vec<SchemaAuditSource>, ReviewRepositoryError> {
        let rows: Vec<(uuid::Uuid, String, Option<String>)> = sqlx::query_as(concat!(
            "SELECT s.listing_source_id, s.listing_source_name,
                    CASE WHEN ",
            requested_audit_due!(),
            " THEN s.schema_audit_request_note END
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
               AND (s.scrape_failure_streak = 0
                    OR s.next_scrape_at IS NULL
                    OR s.next_scrape_at <= NOW())
               AND (",
            requested_audit_due!(),
            "
                    OR ((s.schema_audited_at IS NULL
                         OR s.schema_audited_at < NOW() - make_interval(secs => $1))
                        AND NOT EXISTS (
                            SELECT 1 FROM crawler_reviews decided
                            WHERE decided.listing_source_id = s.listing_source_id
                              AND decided.artifact_type = 'PRODUCT_SCHEMA'
                              AND decided.reason = $3
                              AND decided.status IN ('APPROVED', 'REJECTED')
                              AND decided.reviewed > NOW() - make_interval(secs => $1))))
             ORDER BY ",
            requested_audit_due!(),
            " DESC, s.schema_audited_at ASC NULLS FIRST, s.listing_source_id
             LIMIT $2",
        ))
        .bind(audit_age.as_secs_f64())
        .bind(limit)
        .bind(SCHEMA_AUDIT_REASON)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(listing_source_id, listing_source_name, request_note)| {
                Ok(SchemaAuditSource {
                    listing_source_id: listing_source_id_from_uuid(listing_source_id)?,
                    listing_source_name,
                    request_note,
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

    /// Records a human's request that the live schema audit reopen the ListingSource for the
    /// review agents with `note`, what they should repair, replacing an earlier request. The audit
    /// takes the source ahead of the others once no product-schema review of it is pending. A
    /// source the audit cannot take is refused rather than left waiting: one that is not crawled or
    /// has no live product schemas.
    pub async fn request_schema_audit(
        &self,
        listing_source_id: &ListingSourceId,
        note: &SchemaAuditRequestNote,
    ) -> Result<(), ReviewRepositoryError> {
        let source: Option<(bool, bool)> = sqlx::query_as(
            "SELECT s.crawl_enabled, EXISTS (
                        SELECT 1 FROM listing_source_product_schemas ps
                        WHERE ps.listing_source_id = s.listing_source_id
                          AND (jsonb_typeof(ps.product_schema) <> 'array'
                               OR jsonb_array_length(ps.product_schema) > 0))
             FROM listing_sources s WHERE s.listing_source_id = $1",
        )
        .bind(listing_source_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?;
        let not_possible =
            |reason| ReviewRepositoryError::SchemaAuditNotPossible(*listing_source_id, reason);
        match source {
            None => {
                return Err(ReviewRepositoryError::ListingSourceNotFound(
                    *listing_source_id,
                ));
            }
            Some((false, _)) => return Err(not_possible("it is not crawled")),
            Some((true, false)) => return Err(not_possible("it has no live product schemas")),
            Some((true, true)) => {}
        }
        sqlx::query(
            "UPDATE listing_sources
             SET schema_audit_requested_at = NOW(), schema_audit_request_note = $2
             WHERE listing_source_id = $1",
        )
        .bind(listing_source_id.as_uuid())
        .bind(note.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Records that the ListingSource's live schemas were audited now. The audit records it
    /// before it fetches anything, so a source whose audit fails is not taken again in every pass.
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

    /// The findings of the ListingSource's audit reviews that judged exactly `live_schemas`:
    /// those approved with these schemas as their candidate, by a human or the reviewer agent, and
    /// those a human rejected with them. Their findings are accepted, so the audit does not reopen
    /// the source for them; samples differ between audits, so every such review's findings count,
    /// not only the last one's. Empty when no audit review judged these schemas. A stored
    /// candidate that no longer decodes judged nothing the audit can compare.
    pub async fn accepted_schema_audit_findings(
        &self,
        listing_source_id: &ListingSourceId,
        live_schemas: &[ProductCssSelectorSchema],
    ) -> Result<Vec<String>, ReviewRepositoryError> {
        let rows: Vec<(serde_json::Value, serde_json::Value)> = sqlx::query_as(
            "SELECT r.candidate_payload, r.validation_summary
             FROM crawler_reviews r
             WHERE r.listing_source_id = $1
               AND r.artifact_type = 'PRODUCT_SCHEMA'
               AND r.reason = $2
               AND (r.status = 'APPROVED'
                    OR (r.status = 'REJECTED'
                        AND EXISTS (
                            SELECT 1 FROM crawler_review_events e
                            WHERE e.review_id = r.review_id
                              AND e.action = 'REJECTED'
                              AND e.actor = 'HUMAN')))",
        )
        .bind(listing_source_id.as_uuid())
        .bind(SCHEMA_AUDIT_REASON)
        .fetch_all(&self.pool)
        .await?;
        let mut accepted: Vec<String> = Vec::new();
        for (candidate_payload, validation_summary) in rows {
            let judged_these_schemas =
                ProductSchemaReviewCandidate::decode_stored(&candidate_payload)
                    .is_ok_and(|candidate| candidate.schemas == live_schemas);
            if judged_these_schemas {
                for finding in audit_findings_of(&validation_summary) {
                    if !accepted.contains(&finding) {
                        accepted.push(finding);
                    }
                }
            }
        }
        Ok(accepted)
    }

    /// Opens the product-schema review of an audit of the ListingSource's `live_schemas` on its
    /// captured product `pages`, which records what `audit` found or a human requested: the
    /// candidate is the live schemas, so the review agents repair them as they stand. The request
    /// stays until a decision of the review settles it ([`requested_audit_due`]). When the scraper
    /// opened a product-schema review for the source since the audit took it, the audit opens none
    /// and forgets its record, so the source is audited again once that review is decided; a
    /// scraper review opened in the moment between the check and the insert is joined, as every
    /// schema review is, and the record is forgotten alike.
    pub async fn open_schema_audit_review(
        &self,
        listing_source_id: &ListingSourceId,
        live_schemas: &[ProductCssSelectorSchema],
        pages: Vec<SchemaReviewPageInput>,
        audit: SchemaAuditOpening<'_>,
    ) -> Result<SchemaAuditReview, ReviewRepositoryError> {
        if self.has_pending_schema_review(listing_source_id).await? {
            self.forget_schema_audit(listing_source_id).await?;
            return Ok(SchemaAuditReview::Deferred);
        }
        let page_count = pages.len();
        let review_id = self
            .create_schema_review(
                listing_source_id,
                SCHEMA_AUDIT_REASON,
                live_schemas,
                pages,
                json!({
                    "schema_audit": {
                        "findings": audit.findings,
                        "request_note": audit.request_note,
                        "holds_new_listings": audit.holds_new_listings,
                        "page_count": page_count,
                    }
                }),
            )
            .await?;
        let reason: String =
            sqlx::query_scalar("SELECT reason FROM crawler_reviews WHERE review_id = $1")
                .bind(review_id.as_uuid())
                .fetch_one(&self.pool)
                .await?;
        if reason == SCHEMA_AUDIT_REASON {
            Ok(SchemaAuditReview::Opened(review_id))
        } else {
            self.forget_schema_audit(listing_source_id).await?;
            Ok(SchemaAuditReview::Deferred)
        }
    }

    async fn has_pending_schema_review(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<bool, ReviewRepositoryError> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM crawler_reviews
                 WHERE listing_source_id = $1
                   AND artifact_type = 'PRODUCT_SCHEMA'
                   AND status = 'PENDING_REVIEW')",
        )
        .bind(listing_source_id.as_uuid())
        .fetch_one(&self.pool)
        .await?)
    }

    /// Forgets the ListingSource's last audit, so it is due as soon as nothing else holds it, as
    /// after an audit a stop interrupted.
    pub async fn forget_schema_audit(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<(), ReviewRepositoryError> {
        sqlx::query(
            "UPDATE listing_sources SET schema_audited_at = NULL WHERE listing_source_id = $1",
        )
        .bind(listing_source_id.as_uuid())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// What a human asked the audit review to repair, as its validation summary records it; `None`
/// for a review the audit opened on its own, and for another review.
pub(crate) fn audit_request_note_of(validation_summary: &serde_json::Value) -> Option<String> {
    validation_summary["schema_audit"]["request_note"]
        .as_str()
        .map(str::to_owned)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_take_a_trimmed_note_of_one_to_the_maximum_characters() {
        let longest = "a".repeat(SchemaAuditRequestNote::MAX_CHARS);

        assert_eq!(
            SchemaAuditRequestNote::parse("  Titles lost the dating.  ").map(|note| note.0),
            Ok("Titles lost the dating.".to_owned())
        );
        assert!(SchemaAuditRequestNote::parse(&longest).is_ok());
        assert_eq!(
            SchemaAuditRequestNote::parse(&format!("{longest}a")),
            Err(InvalidSchemaAuditRequestNote)
        );
        assert_eq!(
            SchemaAuditRequestNote::parse(" \n"),
            Err(InvalidSchemaAuditRequestNote)
        );
    }
}
