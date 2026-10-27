//! The verdicts a pending review takes: approval, which makes its candidate the live
//! configuration, rejection and the needs-repair verdict. Each locks the review first, so
//! verdicts, edits and page joins on it apply one after the other.

use super::review_events::{ReplacedUrlPattern, ReviewMutation, commit_with_event};
use super::schema_payload::{ProductSchemaReviewCandidate, approval_product_schemas};
use super::{
    CrawlerReviewRepository, ReviewRepositoryError, classify_with_pattern,
    crawler_domain_id_from_uuid, fetch_review_pages, listing_source_id_from_uuid,
    lock_live_product_schemas, persist_product_schemas,
};
use crate::review::model::*;
use crate::review::page_coverage::{IncompleteCoverage, ReviewPageCoverage};
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::css_selector::removed_page_schema::{
    RemovedPageSchema, rebase_reviewed_removed_page_schemas,
};
use crate::scraper::css_selector::removed_page_schema_repository::{
    lock_stored_removed_page_schemas, store_removed_page_schemas,
};
use crate::spider::candidate_service::AWAITING_URL_PATTERN_REVIEW_CRAWL_OUTCOME;
use crate::{CrawlerDomainId, CrawlerReviewId};
use listing_source_core::ListingSourceId;
use regex::Regex;
use serde::Deserialize;
use sqlx::Row;
use tracing::info;

impl CrawlerReviewRepository {
    /// Makes the pending review's candidate the active configuration. A product-schema candidate
    /// that does not cover every review page is approved only with
    /// [`IncompleteCoverage::Acknowledged`] of the review's current coverage; URL patterns have no
    /// page coverage, so their approval ignores `incomplete_coverage`. The approval's event keeps
    /// the live configuration it replaced.
    pub async fn approve_review(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
        notes: Option<&str>,
        incomplete_coverage: IncompleteCoverage,
    ) -> Result<(), ReviewRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let review = lock_review_for_verdict(&mut transaction, review_id).await?;
        let listing_source_id = review.listing_source_id;
        let domain_id = review.domain_id;

        let approval = match review.artifact_type.as_str() {
            ARTIFACT_URL_PATTERN => ReviewMutation::UrlPatternApproved {
                notes,
                replaced_url_pattern: make_reviewed_url_pattern_live(
                    &mut transaction,
                    review_id,
                    &listing_source_id,
                    domain_id,
                    &review.candidate_payload,
                )
                .await?,
            },
            ARTIFACT_PRODUCT_SCHEMA => {
                let coverage = check_approval_coverage(
                    &mut transaction,
                    review_id,
                    &review.validation_summary,
                    incomplete_coverage,
                )
                .await?;
                let replaced = make_reviewed_product_schemas_live(
                    &mut transaction,
                    review_id,
                    &listing_source_id,
                    &review.reason,
                    &review.candidate_payload,
                    review.removed_page_schemas_base.clone(),
                )
                .await?;
                ReviewMutation::ProductSchemasApproved {
                    notes,
                    coverage,
                    replaced_product_schemas: replaced.product_schemas,
                    replaced_removed_page_schemas: replaced.removed_page_schemas,
                }
            }
            other => {
                return Err(ReviewRepositoryError::UnsupportedArtifact(
                    review_id,
                    other.into(),
                ));
            }
        };

        sqlx::query(
            "UPDATE crawler_reviews \
             SET status = 'APPROVED', reviewer_notes = COALESCE($2, reviewer_notes), \
                 reviewed = NOW(), updated = NOW() \
             WHERE review_id = $1 AND status = 'PENDING_REVIEW'",
        )
        .bind(review_id.as_uuid())
        .bind(notes)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE crawler_reviews \
             SET status = 'SUPERSEDED', reviewed = NOW(), updated = NOW() \
             WHERE listing_source_id = $1 AND artifact_type = $2 \
               AND status = 'PENDING_REVIEW' AND review_id <> $3 \
               AND ($4::uuid IS NULL OR domain_id = $4)",
        )
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(&review.artifact_type)
        .bind(review_id.as_uuid())
        .bind(domain_id.map(CrawlerDomainId::into_uuid))
        .execute(&mut *transaction)
        .await?;
        release_work_waiting_on_decided_review(&mut transaction, &review).await?;
        commit_with_event(transaction, review_id, actor, approval).await?;
        info!(review_id = %review_id, actor = %actor, "Review approved");
        Ok(())
    }

    /// Closes the review without changing configuration and, as approval does, releases the work
    /// waiting on it: the review is decided, so a page that waited on a product-schema review is
    /// retried at once and the next crawl of a domain that waited on a URL-pattern review may open
    /// a new one.
    pub async fn reject_review(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
        notes: Option<&str>,
    ) -> Result<(), ReviewRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let review = lock_review_for_verdict(&mut transaction, review_id).await?;
        sqlx::query(
            "UPDATE crawler_reviews \
             SET status = 'REJECTED', reviewer_notes = COALESCE($2, reviewer_notes), \
                 reviewed = NOW(), updated = NOW() \
             WHERE review_id = $1 AND status = 'PENDING_REVIEW'",
        )
        .bind(review_id.as_uuid())
        .bind(notes)
        .execute(&mut *transaction)
        .await?;
        release_work_waiting_on_decided_review(&mut transaction, &review).await?;
        commit_with_event(
            transaction,
            review_id,
            actor,
            ReviewMutation::Rejected { notes },
        )
        .await?;
        info!(review_id = %review_id, actor = %actor, "Review rejected");
        Ok(())
    }

    /// Records the needs-repair verdict on a pending review. The review stays pending, so its
    /// candidate stays editable, its pages and the pages waiting on it stay attached, and no
    /// second review opens for the same artifact; the reviewer later approves or rejects it. A
    /// repeated verdict replaces the note.
    pub async fn request_repair(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
        note: Option<&str>,
    ) -> Result<(), ReviewRepositoryError> {
        let note = note
            .map(str::trim)
            .filter(|note| !note.is_empty())
            .ok_or(ReviewRepositoryError::MissingRepairNote)?;
        let mut transaction = self.pool.begin().await?;
        lock_review_for_verdict(&mut transaction, review_id).await?;
        sqlx::query(
            "UPDATE crawler_reviews \
             SET repair_requested = NOW(), repair_note = $2, updated = NOW() \
             WHERE review_id = $1 AND status = 'PENDING_REVIEW'",
        )
        .bind(review_id.as_uuid())
        .bind(note)
        .execute(&mut *transaction)
        .await?;
        commit_with_event(
            transaction,
            review_id,
            actor,
            ReviewMutation::RepairRequested { note },
        )
        .await?;
        info!(review_id = %review_id, actor = %actor, "Reviewer asked for a review to be repaired");
        Ok(())
    }
}

/// Makes the reviewed decision of the URL-pattern review `review_id` the live decision of its
/// domain inside the approval's transaction, classifies the domain's stored URLs with it, and
/// returns the decision it replaced.
async fn make_reviewed_url_pattern_live(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    listing_source_id: &ListingSourceId,
    domain_id: Option<CrawlerDomainId>,
    candidate_payload: &serde_json::Value,
) -> Result<ReplacedUrlPattern, ReviewRepositoryError> {
    let candidate = UrlPatternReviewCandidate::deserialize(candidate_payload)
        .map_err(|_| ReviewRepositoryError::InvalidUrlPatternCandidate)?;
    let pattern = candidate
        .validated_pattern()
        .map_err(|_| ReviewRepositoryError::InvalidUrlPatternCandidate)?;
    let domain_id = domain_id.ok_or(sqlx::Error::RowNotFound)?;
    let (url_pattern, url_pattern_state) = sqlx::query_as::<_, (Option<String>, String)>(
        "SELECT url_pattern, url_pattern_state FROM listing_source_domains \
         WHERE listing_source_id = $1 AND domain_id = $2 FOR UPDATE",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(domain_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(ReviewRepositoryError::NotFound(review_id))?;
    sqlx::query(
        "UPDATE listing_source_domains \
         SET url_pattern = $3, \
             url_pattern_state = CASE WHEN $3 IS NULL THEN 'NO_PATTERN' ELSE 'MATCHED' END \
         WHERE listing_source_id = $1 AND domain_id = $2",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(domain_id.as_uuid())
    .bind(pattern)
    .execute(&mut **transaction)
    .await?;
    let live_pattern = pattern
        .map(Regex::new)
        .transpose()
        .map_err(|_| ReviewRepositoryError::InvalidUrlPatternCandidate)?;
    let reclassified = reclassify_domain_urls(
        transaction,
        listing_source_id,
        domain_id,
        live_pattern.as_ref(),
    )
    .await?;
    info!(
        review_id = %review_id,
        domain_id = %domain_id,
        reclassified_urls = reclassified,
        "Approved URL pattern classified the domain's stored URLs"
    );
    Ok(ReplacedUrlPattern {
        url_pattern,
        url_pattern_state,
    })
}

/// The ListingSource's live product-schema configuration an approval replaced.
struct ReplacedProductSchemas {
    /// The live schema list, `None` when none was live.
    product_schemas: Option<Vec<ProductCssSelectorSchema>>,
    removed_page_schemas: Vec<RemovedPageSchema>,
}

/// Makes the reviewed candidate of the product-schema review `review_id` live inside the
/// approval's transaction: its schemas, and its removed-page templates in place of the stored
/// ones, rebased onto the stored list's changes since `removed_page_schemas_base`. The review
/// keeps what went live, so edits through it are not refused as stale. Returns what it
/// replaced.
async fn make_reviewed_product_schemas_live(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    listing_source_id: &ListingSourceId,
    reason: &str,
    candidate_payload: &serde_json::Value,
    removed_page_schemas_base: serde_json::Value,
) -> Result<ReplacedProductSchemas, ReviewRepositoryError> {
    let reviewed = ProductSchemaReviewCandidate::parse(candidate_payload)?;
    let existing = lock_live_product_schemas(transaction, listing_source_id).await?;
    let schemas = approval_product_schemas(reason, existing.as_ref(), reviewed.schemas.clone())?;
    persist_product_schemas(transaction, listing_source_id, &schemas).await?;
    let RemovedPageSchemasReplacement { replaced, stored } = replace_stored_removed_page_schemas(
        transaction,
        review_id,
        listing_source_id,
        removed_page_schemas_base,
        reviewed.removed_page_schemas.clone(),
    )
    .await?;
    let live = reviewed
        .clone()
        .with_schemas(schemas)
        .with_removed_page_schemas(stored);
    if live != reviewed {
        sqlx::query("UPDATE crawler_reviews SET candidate_payload = $2 WHERE review_id = $1")
            .bind(review_id.as_uuid())
            .bind(live.to_payload()?)
            .execute(&mut **transaction)
            .await?;
    }
    Ok(ReplacedProductSchemas {
        product_schemas: existing.map(|existing| existing.product_schemas),
        removed_page_schemas: replaced,
    })
}

/// The ListingSource's stored removed-page templates before and after an approval replaced them.
struct RemovedPageSchemasReplacement {
    replaced: Vec<RemovedPageSchema>,
    stored: Vec<RemovedPageSchema>,
}

/// Replaces the ListingSource's stored removed-page templates with the `reviewed` list, rebased
/// onto the changes other writers made since the review read the stored list as `base`. A
/// stored list or base that no longer decodes fails the approval.
async fn replace_stored_removed_page_schemas(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    listing_source_id: &ListingSourceId,
    base: serde_json::Value,
    reviewed: Vec<RemovedPageSchema>,
) -> Result<RemovedPageSchemasReplacement, ReviewRepositoryError> {
    let base: Vec<RemovedPageSchema> =
        serde_json::from_value(base).map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
    let replaced = lock_stored_removed_page_schemas(transaction, listing_source_id)
        .await?
        .unwrap_or_default();
    let stored = rebase_reviewed_removed_page_schemas(&base, reviewed, &replaced);
    if stored != replaced {
        store_removed_page_schemas(transaction, listing_source_id, &stored).await?;
        info!(
            review_id = %review_id,
            listing_source_id = %listing_source_id,
            removed_page_template_count = stored.len(),
            retired_template_count = replaced.iter().filter(|schema| !stored.contains(schema)).count(),
            "Approved removed-page templates from a product schema review"
        );
    }
    Ok(RemovedPageSchemasReplacement { replaced, stored })
}

/// The state of a pending review a verdict decides on, read under the review's row lock.
struct ReviewUnderVerdict {
    listing_source_id: ListingSourceId,
    domain_id: Option<CrawlerDomainId>,
    artifact_type: String,
    reason: String,
    candidate_payload: serde_json::Value,
    validation_summary: serde_json::Value,
    removed_page_schemas_base: serde_json::Value,
}

/// Locks the review for a verdict, so verdicts, edits and page joins on it apply one after the
/// other, and reads what the verdict decides on. Only a pending review takes a verdict; an
/// unknown review is told apart from a closed one.
async fn lock_review_for_verdict(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
) -> Result<ReviewUnderVerdict, ReviewRepositoryError> {
    let review = sqlx::query(
        "SELECT status, listing_source_id, domain_id, artifact_type, reason, candidate_payload, \
                validation_summary, removed_page_schemas_base \
         FROM crawler_reviews WHERE review_id = $1 FOR UPDATE",
    )
    .bind(review_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(ReviewRepositoryError::NotFound(review_id))?;
    if review.try_get::<String, _>("status")? != STATUS_PENDING_REVIEW {
        return Err(ReviewRepositoryError::NotPending(review_id));
    }
    Ok(ReviewUnderVerdict {
        listing_source_id: listing_source_id_from_uuid(
            review.try_get::<uuid::Uuid, _>("listing_source_id")?,
        )?,
        domain_id: review
            .try_get::<Option<uuid::Uuid>, _>("domain_id")?
            .map(crawler_domain_id_from_uuid)
            .transpose()?,
        artifact_type: review.try_get("artifact_type")?,
        reason: review.try_get("reason")?,
        candidate_payload: review.try_get("candidate_payload")?,
        validation_summary: review.try_get("validation_summary")?,
        removed_page_schemas_base: review.try_get("removed_page_schemas_base")?,
    })
}

/// Refuses to approve a product-schema candidate whose stored schema matrix does not cover every
/// review page, unless the approver acknowledged exactly the review's current coverage. The
/// caller holds the review's row lock, which page joins and removals, candidate edits and matrix
/// stores take first too, so the coverage checked is the one the approval decides on. A
/// candidate change drops the stored matrix, so a stored matrix evaluated the current candidate;
/// one that no longer decodes vouches for no page. Returns the coverage the approval goes ahead
/// under.
async fn check_approval_coverage(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    validation_summary: &serde_json::Value,
    incomplete_coverage: IncompleteCoverage,
) -> Result<ReviewPageCoverage, ReviewRepositoryError> {
    let pages = fetch_review_pages(&mut **transaction, review_id).await?;
    let matrix = SchemaMatrix::stored_in(validation_summary).and_then(Result::ok);
    let coverage = ReviewPageCoverage::of(matrix.as_ref(), &pages);
    if !coverage.admits(incomplete_coverage) {
        return Err(match incomplete_coverage {
            IncompleteCoverage::Refuse => ReviewRepositoryError::IncompleteCoverage {
                review_id,
                coverage,
            },
            IncompleteCoverage::Acknowledged(acknowledged) => {
                ReviewRepositoryError::StaleCoverageAcknowledgement {
                    review_id,
                    coverage,
                    acknowledged,
                }
            }
        });
    }
    if !coverage.is_complete() {
        info!(
            review_id = %review_id,
            covered_pages = coverage.covered,
            removed_pages = coverage.removed,
            review_pages = coverage.total,
            not_evaluated_pages = coverage.not_evaluated,
            "Approving a product schema candidate with acknowledged incomplete coverage"
        );
    }
    Ok(coverage)
}

/// Releases the work that waited on the decided review for an immediate retry: every page of the
/// ListingSource that waits on its product-schema review, or the domain whose crawl waits on its
/// URL-pattern review. It runs last in the verdict's transaction, after the review row is locked
/// and closed, so a wait written while the verdict commits waits for it and then finds the review
/// closed instead of holding back a page the verdict released.
async fn release_work_waiting_on_decided_review(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review: &ReviewUnderVerdict,
) -> Result<(), ReviewRepositoryError> {
    match review.artifact_type.as_str() {
        ARTIFACT_PRODUCT_SCHEMA => {
            sqlx::query(
                "UPDATE listing_source_urls \
                 SET next_retry_at = NULL, last_error_kind = NULL, \
                     last_error_message = NULL, schema_review_waited_on = NULL, \
                     schema_review_wait_streak = 0, updated = NOW() \
                 WHERE listing_source_id = $1 \
                   AND last_error_kind = 'PendingSchemaReview' \
                   AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')",
            )
            .bind(uuid::Uuid::from(review.listing_source_id))
            .execute(&mut **transaction)
            .await?;
        }
        ARTIFACT_URL_PATTERN => {
            if let Some(domain_id) = review.domain_id {
                release_domain_waiting_for_url_pattern(
                    transaction,
                    &review.listing_source_id,
                    domain_id,
                )
                .await?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Classifies the domain's stored URLs with the pattern decision that just went live, as the
/// spider would on its next crawl, so URLs a crawl stored while the review was pending need no
/// second crawl from the root. Sold URLs keep their class, as a re-crawl leaves them.
async fn reclassify_domain_urls(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    listing_source_id: &ListingSourceId,
    domain_id: CrawlerDomainId,
    pattern: Option<&Regex>,
) -> Result<u64, ReviewRepositoryError> {
    let stored = sqlx::query_as::<_, (String, String)>(
        "SELECT url, url_class FROM listing_source_urls \
         WHERE listing_source_id = $1 AND domain_id = $2 AND crawler_disposition = 'ACTIVE'",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(domain_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await?;
    let (urls, classes): (Vec<String>, Vec<String>) = stored
        .into_iter()
        .filter_map(|(url, current_class)| {
            let class = classify_with_pattern(&url, pattern);
            (class != current_class).then_some((url, class))
        })
        .unzip();
    if urls.is_empty() {
        return Ok(0);
    }
    let result = sqlx::query(
        "UPDATE listing_source_urls AS u \
         SET url_class = input.url_class, updated = NOW() \
         FROM UNNEST($1::text[], $2::text[]) AS input(url, url_class) \
         WHERE u.url = input.url AND u.listing_source_id = $3 AND u.domain_id = $4",
    )
    .bind(&urls)
    .bind(&classes)
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(domain_id.as_uuid())
    .execute(&mut **transaction)
    .await?;
    Ok(result.rows_affected())
}

/// Lets the scheduler crawl a domain again whose last crawl stopped at a URL-pattern review,
/// now that the review is decided. Rows written before the review-wait outcome existed carry
/// the wait as `last_crawl_error_kind` instead and are released the same way.
async fn release_domain_waiting_for_url_pattern(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    listing_source_id: &ListingSourceId,
    domain_id: CrawlerDomainId,
) -> Result<(), ReviewRepositoryError> {
    sqlx::query(
        "UPDATE listing_source_domains \
         SET next_crawl_at = NULL, \
             last_crawl_error_kind = CASE \
                 WHEN last_crawl_error_kind = 'PendingUrlPatternReview' THEN NULL \
                 ELSE last_crawl_error_kind \
             END \
         WHERE listing_source_id = $1 AND domain_id = $2 \
           AND (last_crawl_error_kind = 'PendingUrlPatternReview' OR last_crawl_outcome = $3)",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(domain_id.as_uuid())
    .bind(AWAITING_URL_PATTERN_REVIEW_CRAWL_OUTCOME)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}
