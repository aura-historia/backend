use crate::review::model::*;
use crate::review::schema_evaluation::evaluate_schema_matrix_for_live_review_pages;
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::css_selector::product_schema_repository::{
    ListingSourceProductSchemaRepository, ListingSourceProductSchemaRepositoryImpl,
};
use crate::scraper::css_selector::rule::ExtractionRule;
use crate::spider::utils::url::CrawledUrl;
use listing_source_core::ListingSourceId;
use regex::Regex;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use tracing::info;
use url::Url;

mod schema_payload;

use schema_payload::{approval_product_schemas, parse_schemas_payload, update_schema_rule};

#[derive(Debug, thiserror::Error)]
pub enum ReviewRepositoryError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Regex(#[from] regex::Error),
    #[error("review not found: {0}")]
    NotFound(uuid::Uuid),
    #[error("review {0} is not pending review")]
    NotPending(uuid::Uuid),
    #[error("review {0} has unsupported artifact type {1}")]
    UnsupportedArtifact(uuid::Uuid, String),
    #[error("invalid schema field `{0}`")]
    InvalidSchemaField(String),
    #[error("field `{0}` is required and cannot be deleted")]
    RequiredSchemaField(String),
}

#[derive(Clone)]
pub struct CrawlerReviewRepository {
    pool: PgPool,
}

struct PendingReviewInsert {
    review_id: uuid::Uuid,
    inserted: bool,
}

pub struct SchemaReviewWithStatusInput<'a> {
    pub listing_source_id: &'a ListingSourceId,
    pub reason: &'a str,
    pub schemas: &'a [ProductCssSelectorSchema],
    pub pages: Vec<SchemaReviewPageInput>,
    pub validation_summary: serde_json::Value,
    pub status: &'a str,
    pub notes: Option<&'a str>,
}

struct ReviewWithStatusInsert<'a> {
    listing_source_id: &'a ListingSourceId,
    domain_id: Option<&'a uuid::Uuid>,
    artifact_type: &'a str,
    status: &'a str,
    reason: &'a str,
    candidate_payload: serde_json::Value,
    validation_summary: serde_json::Value,
    notes: Option<&'a str>,
}

impl CrawlerReviewRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn has_pending_review(
        &self,
        listing_source_id: &ListingSourceId,
        artifact_type: &str,
    ) -> Result<bool, sqlx::Error> {
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM crawler_reviews
                WHERE listing_source_id = $1 AND artifact_type = $2 AND status = 'PENDING_REVIEW'
            )",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(artifact_type)
        .fetch_one(&self.pool)
        .await?;
        Ok(exists)
    }

    pub async fn latest_pending_review_id(
        &self,
        listing_source_id: &ListingSourceId,
        artifact_type: &str,
    ) -> Result<Option<uuid::Uuid>, sqlx::Error> {
        sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT review_id
             FROM crawler_reviews
             WHERE listing_source_id = $1 AND artifact_type = $2 AND status = 'PENDING_REVIEW'
             ORDER BY created DESC
             LIMIT 1",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(artifact_type)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn list_reviews(&self, limit: i64) -> Result<Vec<CrawlerReview>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT r.review_id, r.listing_source_id, s.listing_source_name, r.domain_id, r.artifact_type, r.status, r.reason,
                    r.candidate_payload, r.validation_summary, r.reviewer_notes, r.created, r.updated, r.reviewed
             FROM crawler_reviews r
             LEFT JOIN listing_sources s ON s.listing_source_id = r.listing_source_id
             ORDER BY
               CASE WHEN r.status = 'PENDING_REVIEW' THEN 0 ELSE 1 END,
               r.created DESC
             LIMIT $1",
        )
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;

        rows.into_iter().map(row_to_review).collect()
    }

    pub async fn list_listing_sources(
        &self,
        limit: i64,
    ) -> Result<Vec<ListingSourceOverview>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT
                s.listing_source_id,
                s.listing_source_name,
                s.llm_calls_count,
                s.url_pattern,
                COALESCE(r.pending_reviews, 0) AS pending_reviews,
                COALESCE(u.product_urls, 0) AS product_urls,
                COALESCE(u.blocked_urls, 0) AS blocked_urls
             FROM listing_sources s
             LEFT JOIN (
                SELECT listing_source_id, COUNT(*) AS pending_reviews
                FROM crawler_reviews
                WHERE status = 'PENDING_REVIEW'
                GROUP BY listing_source_id
             ) r ON r.listing_source_id = s.listing_source_id
             LEFT JOIN (
                SELECT listing_source_id,
                       COUNT(*) FILTER (WHERE url_class = 'product') AS product_urls,
                       COUNT(*) FILTER (WHERE last_error_kind IN ('PendingSchemaReview', 'PendingUrlPatternReview')) AS blocked_urls
                FROM listing_source_urls
                GROUP BY listing_source_id
             ) u ON u.listing_source_id = s.listing_source_id
             WHERE s.crawl_enabled = TRUE
             ORDER BY pending_reviews DESC, s.updated DESC
             LIMIT $1",
        )
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;

        let mut listing_sources = Vec::with_capacity(rows.len());
        for row in rows {
            listing_sources.push(ListingSourceOverview {
                listing_source_id: ListingSourceId::from(
                    row.try_get::<uuid::Uuid, _>("listing_source_id")?,
                ),
                listing_source_name: row.try_get("listing_source_name")?,
                llm_calls_count: row.try_get("llm_calls_count")?,
                url_pattern: row.try_get("url_pattern")?,
                pending_reviews: row.try_get("pending_reviews")?,
                product_urls: row.try_get("product_urls")?,
                blocked_urls: row.try_get("blocked_urls")?,
            });
        }
        Ok(listing_sources)
    }

    pub async fn get_review(
        &self,
        review_id: uuid::Uuid,
    ) -> Result<ReviewDetail, ReviewRepositoryError> {
        let row = sqlx::query(
            "SELECT r.review_id, r.listing_source_id, s.listing_source_name, r.domain_id, r.artifact_type, r.status, r.reason,
                    r.candidate_payload, r.validation_summary, r.reviewer_notes, r.created, r.updated, r.reviewed
             FROM crawler_reviews r
             LEFT JOIN listing_sources s ON s.listing_source_id = r.listing_source_id
             WHERE r.review_id = $1",
        )
            .bind(review_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(ReviewRepositoryError::NotFound(review_id))?;

        let mut review = row_to_review(row)?;
        self.backfill_legacy_schema_review_payload(&mut review)
            .await?;
        let pages = self.get_review_pages(review_id).await?;
        let urls = self.get_review_urls(review_id).await?;
        Ok(ReviewDetail {
            review,
            pages,
            urls,
        })
    }

    pub async fn get_review_pages(
        &self,
        review_id: uuid::Uuid,
    ) -> Result<Vec<CrawlerReviewPage>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT review_page_id, review_id, url, role, html_hash, fetched
             FROM crawler_review_pages
             WHERE review_id = $1
             ORDER BY
               CASE role
                 WHEN 'PRIMARY' THEN 0
                  WHEN 'TRIGGERING_GENERATION_PAGE' THEN 0
                  WHEN 'TRIGGERING_REPAIR_PAGE' THEN 0
                 ELSE 1
               END,
               created",
        )
        .bind(review_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_page).collect()
    }

    async fn get_review_urls(
        &self,
        review_id: uuid::Uuid,
    ) -> Result<Vec<CrawlerReviewUrl>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT review_url_id, review_id, url, previous_class, current_pattern_match,
                    candidate_pattern_match, candidate_class
             FROM crawler_review_urls
             WHERE review_id = $1
             ORDER BY created",
        )
        .bind(review_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_review_url).collect()
    }

    pub async fn get_review_page(
        &self,
        review_page_id: uuid::Uuid,
    ) -> Result<Option<CrawlerReviewPage>, sqlx::Error> {
        sqlx::query(
            "SELECT review_page_id, review_id, url, role, html_hash, fetched
             FROM crawler_review_pages
             WHERE review_page_id = $1",
        )
        .bind(review_page_id)
        .fetch_optional(&self.pool)
        .await?
        .map(row_to_page)
        .transpose()
    }

    pub async fn create_url_pattern_review(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: Option<&uuid::Uuid>,
        reason: &str,
        candidate_pattern: Option<&Regex>,
        urls: &[String],
        current_pattern: Option<&Regex>,
    ) -> Result<uuid::Uuid, ReviewRepositoryError> {
        let candidate_payload = json!({
            "pattern": candidate_pattern.map(Regex::as_str),
            "current_pattern": current_pattern.map(Regex::as_str),
        });
        let validation_summary = json!({
            "sample_count": urls.len(),
            "candidate_product_count": candidate_pattern
                .map(|pattern| urls.iter().filter(|url| pattern.is_match(url)).count())
                .unwrap_or(0),
        });

        let insert = self
            .insert_pending_review_or_get_existing(
                listing_source_id,
                domain_id,
                ARTIFACT_URL_PATTERN,
                reason,
                candidate_payload,
                validation_summary,
            )
            .await?;

        if !insert.inserted {
            return Ok(insert.review_id);
        }

        for raw_url in urls {
            let current_match = current_pattern.map(|pattern| pattern.is_match(raw_url));
            let candidate_match = candidate_pattern.map(|pattern| pattern.is_match(raw_url));
            let candidate_class = classify_with_pattern(raw_url, candidate_pattern);
            sqlx::query(
                "INSERT INTO crawler_review_urls (
                    review_id, url, current_pattern_match, candidate_pattern_match, candidate_class
                 ) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(insert.review_id)
            .bind(raw_url)
            .bind(current_match)
            .bind(candidate_match)
            .bind(candidate_class)
            .execute(&self.pool)
            .await?;
        }

        Ok(insert.review_id)
    }

    pub async fn create_schema_review(
        &self,
        listing_source_id: &ListingSourceId,
        reason: &str,
        schemas: &[ProductCssSelectorSchema],
        pages: Vec<SchemaReviewPageInput>,
        validation_summary: serde_json::Value,
    ) -> Result<uuid::Uuid, ReviewRepositoryError> {
        self.insert_schema_review(SchemaReviewWithStatusInput {
            listing_source_id,
            reason,
            schemas,
            pages,
            validation_summary,
            status: STATUS_PENDING_REVIEW,
            notes: None,
        })
        .await
    }

    pub async fn create_schema_review_with_status(
        &self,
        input: SchemaReviewWithStatusInput<'_>,
    ) -> Result<uuid::Uuid, ReviewRepositoryError> {
        self.insert_schema_review(input).await
    }

    async fn insert_schema_review(
        &self,
        input: SchemaReviewWithStatusInput<'_>,
    ) -> Result<uuid::Uuid, ReviewRepositoryError> {
        let SchemaReviewWithStatusInput {
            listing_source_id,
            reason,
            schemas,
            pages,
            validation_summary,
            status,
            notes,
        } = input;
        let candidate_payload = json!({ "schemas": schemas });
        let insert = if status == STATUS_PENDING_REVIEW {
            self.insert_pending_review_or_get_existing(
                listing_source_id,
                None,
                ARTIFACT_PRODUCT_SCHEMA,
                reason,
                candidate_payload,
                validation_summary,
            )
            .await?
        } else {
            PendingReviewInsert {
                review_id: self
                    .insert_review_with_status(ReviewWithStatusInsert {
                        listing_source_id,
                        domain_id: None,
                        artifact_type: ARTIFACT_PRODUCT_SCHEMA,
                        status,
                        reason,
                        candidate_payload,
                        validation_summary,
                        notes,
                    })
                    .await?,
                inserted: true,
            }
        };

        if insert.inserted {
            self.insert_review_pages(insert.review_id, pages).await?;
        }

        Ok(insert.review_id)
    }

    async fn insert_pending_review_or_get_existing(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: Option<&uuid::Uuid>,
        artifact_type: &str,
        reason: &str,
        candidate_payload: serde_json::Value,
        validation_summary: serde_json::Value,
    ) -> Result<PendingReviewInsert, sqlx::Error> {
        let inserted = sqlx::query_scalar::<_, uuid::Uuid>(
            "INSERT INTO crawler_reviews (
                listing_source_id, domain_id, artifact_type, status, reason, candidate_payload,
                validation_summary, reviewer_notes, reviewed
             ) VALUES ($1, $2, $3, 'PENDING_REVIEW', $4, $5, $6, NULL, NULL)
             ON CONFLICT (listing_source_id, artifact_type)
             WHERE status = 'PENDING_REVIEW'
             DO NOTHING
             RETURNING review_id",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(domain_id.copied())
        .bind(artifact_type)
        .bind(reason)
        .bind(candidate_payload)
        .bind(validation_summary)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(review_id) = inserted {
            return Ok(PendingReviewInsert {
                review_id,
                inserted: true,
            });
        }

        let review_id = self
            .latest_pending_review_id(listing_source_id, artifact_type)
            .await?
            .ok_or(sqlx::Error::RowNotFound)?;

        Ok(PendingReviewInsert {
            review_id,
            inserted: false,
        })
    }

    pub async fn update_candidate_payload(
        &self,
        review_id: uuid::Uuid,
        candidate_payload: serde_json::Value,
    ) -> Result<(), ReviewRepositoryError> {
        let detail = self.get_review(review_id).await?;
        if detail.review.status == STATUS_PENDING_REVIEW {
            return self
                .update_pending_candidate_payload(review_id, candidate_payload)
                .await;
        }
        if detail.review.status == STATUS_APPROVED
            && detail.review.artifact_type == ARTIFACT_PRODUCT_SCHEMA
        {
            return self
                .update_approved_product_schema_payload(&detail.review, candidate_payload)
                .await;
        }

        Err(ReviewRepositoryError::NotPending(review_id))
    }

    async fn update_pending_candidate_payload(
        &self,
        review_id: uuid::Uuid,
        candidate_payload: serde_json::Value,
    ) -> Result<(), ReviewRepositoryError> {
        let result = sqlx::query(
            "UPDATE crawler_reviews
             SET candidate_payload = $2, updated = NOW()
             WHERE review_id = $1 AND status = 'PENDING_REVIEW'",
        )
        .bind(review_id)
        .bind(candidate_payload)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(ReviewRepositoryError::NotPending(review_id));
        }
        Ok(())
    }

    async fn update_approved_product_schema_payload(
        &self,
        review: &CrawlerReview,
        candidate_payload: serde_json::Value,
    ) -> Result<(), ReviewRepositoryError> {
        let schemas = parse_schemas_payload(&candidate_payload)?;
        let repo = ListingSourceProductSchemaRepositoryImpl::new(&self.pool);
        if repo
            .find_product_schema(&review.listing_source_id)
            .await?
            .is_some()
        {
            repo.update_product_schema(&review.listing_source_id, &schemas)
                .await?;
        } else {
            let now = OffsetDateTime::now_utc();
            let schema = crate::scraper::css_selector::product_schema::ListingSourceProductSchema {
                listing_source_id: review.listing_source_id,
                product_schemas: schemas.clone(),
                created: now,
                updated: now,
            };
            repo.insert_product_schema(&review.listing_source_id, &schema)
                .await?;
        }

        let validation_summary =
            append_manual_schema_edit(review.validation_summary.clone(), schemas.len());
        sqlx::query(
            "UPDATE crawler_reviews
             SET candidate_payload = $2, validation_summary = $3, updated = NOW()
             WHERE review_id = $1 AND status = 'APPROVED' AND artifact_type = 'PRODUCT_SCHEMA'",
        )
        .bind(review.review_id)
        .bind(candidate_payload)
        .bind(validation_summary)
        .execute(&self.pool)
        .await?;

        info!(
            review_id = %review.review_id,
            listing_source_id = %review.listing_source_id,
            schema_count = schemas.len(),
            "Approved product schema edited from review console"
        );
        Ok(())
    }

    pub async fn update_schema_field(
        &self,
        review_id: uuid::Uuid,
        schema_index: usize,
        field: &str,
        rule: Option<ExtractionRule>,
    ) -> Result<(), ReviewRepositoryError> {
        let detail = self.get_review(review_id).await?;
        if detail.review.artifact_type != ARTIFACT_PRODUCT_SCHEMA {
            return Err(ReviewRepositoryError::UnsupportedArtifact(
                review_id,
                detail.review.artifact_type,
            ));
        }
        if !matches!(
            detail.review.status.as_str(),
            STATUS_PENDING_REVIEW | STATUS_APPROVED
        ) {
            return Err(ReviewRepositoryError::NotPending(review_id));
        }

        let mut schemas = parse_schemas_payload(&detail.review.candidate_payload)?;
        let Some(schema) = schemas.get_mut(schema_index) else {
            return Err(ReviewRepositoryError::InvalidSchemaField(format!(
                "schema index {schema_index}"
            )));
        };

        update_schema_rule(schema, field, rule)?;
        self.update_candidate_payload(review_id, json!({ "schemas": schemas }))
            .await
    }

    pub async fn approve_review(
        &self,
        review_id: uuid::Uuid,
        notes: Option<&str>,
    ) -> Result<(), ReviewRepositoryError> {
        let detail = self.get_review(review_id).await?;
        if detail.review.status != STATUS_PENDING_REVIEW {
            return Err(ReviewRepositoryError::NotPending(review_id));
        }

        match detail.review.artifact_type.as_str() {
            ARTIFACT_URL_PATTERN => {
                let pattern = detail
                    .review
                    .candidate_payload
                    .get("pattern")
                    .and_then(serde_json::Value::as_str);
                if let Some(pattern) = pattern {
                    sqlx::query(
                        "UPDATE listing_sources
                         SET url_pattern = $2, updated = NOW()
                         WHERE listing_source_id = $1",
                    )
                    .bind(uuid::Uuid::from(detail.review.listing_source_id))
                    .bind(pattern)
                    .execute(&self.pool)
                    .await?;
                }
            }
            ARTIFACT_PRODUCT_SCHEMA => {
                let reviewed_schemas = parse_schemas_payload(&detail.review.candidate_payload)?;
                let repo = ListingSourceProductSchemaRepositoryImpl::new(&self.pool);
                let existing = repo
                    .find_product_schema(&detail.review.listing_source_id)
                    .await?;
                let schemas = approval_product_schemas(
                    &detail.review.reason,
                    existing.as_ref(),
                    reviewed_schemas,
                )?;
                if existing.is_some() {
                    repo.update_product_schema(&detail.review.listing_source_id, &schemas)
                        .await?;
                } else {
                    let now = OffsetDateTime::now_utc();
                    let schema =
                        crate::scraper::css_selector::product_schema::ListingSourceProductSchema {
                            listing_source_id: detail.review.listing_source_id,
                            product_schemas: schemas,
                            created: now,
                            updated: now,
                        };
                    repo.insert_product_schema(&detail.review.listing_source_id, &schema)
                        .await?;
                }
            }
            other => {
                return Err(ReviewRepositoryError::UnsupportedArtifact(
                    review_id,
                    other.into(),
                ));
            }
        }

        self.set_review_status(review_id, STATUS_APPROVED, notes)
            .await?;
        self.supersede_other_pending_reviews(
            &detail.review.listing_source_id,
            &detail.review.artifact_type,
            review_id,
        )
        .await?;
        self.clear_pending_blocks(&detail.review).await?;
        Ok(())
    }

    pub async fn reject_review(
        &self,
        review_id: uuid::Uuid,
        notes: Option<&str>,
        needs_repair: bool,
    ) -> Result<(), ReviewRepositoryError> {
        let status = if needs_repair {
            STATUS_NEEDS_REPAIR
        } else {
            STATUS_REJECTED
        };
        self.set_review_status(review_id, status, notes).await
    }

    pub async fn evaluate_schema_matrix_for_live_pages(
        &self,
        review_id: uuid::Uuid,
        pages: Vec<(CrawlerReviewPage, String)>,
    ) -> Result<SchemaMatrix, ReviewRepositoryError> {
        let detail = self.get_review(review_id).await?;
        let schemas = parse_schemas_payload(&detail.review.candidate_payload)?;

        Ok(evaluate_schema_matrix_for_live_review_pages(
            review_id, &schemas, &pages,
        ))
    }

    pub async fn update_review_validation_summary(
        &self,
        review_id: uuid::Uuid,
        validation_summary: serde_json::Value,
    ) -> Result<(), ReviewRepositoryError> {
        sqlx::query(
            "UPDATE crawler_reviews
             SET validation_summary = $2, updated = NOW()
             WHERE review_id = $1",
        )
        .bind(review_id)
        .bind(validation_summary)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn backfill_legacy_schema_review_payload(
        &self,
        review: &mut CrawlerReview,
    ) -> Result<(), ReviewRepositoryError> {
        if review.artifact_type != ARTIFACT_PRODUCT_SCHEMA
            || review.status != STATUS_PENDING_REVIEW
            || !matches!(
                review.reason.as_str(),
                "append_schema_generation" | "normalization_schema_repair"
            )
        {
            return Ok(());
        }

        let reviewed_schemas = parse_schemas_payload(&review.candidate_payload)?;
        if reviewed_schemas.len() != 1 {
            return Ok(());
        }

        let repo = ListingSourceProductSchemaRepositoryImpl::new(&self.pool);
        let Some(existing) = repo.find_product_schema(&review.listing_source_id).await? else {
            return Ok(());
        };
        let merged = approval_product_schemas(&review.reason, Some(&existing), reviewed_schemas)?;
        review.candidate_payload = json!({ "schemas": merged });
        Ok(())
    }

    async fn insert_review_with_status(
        &self,
        input: ReviewWithStatusInsert<'_>,
    ) -> Result<uuid::Uuid, sqlx::Error> {
        let ReviewWithStatusInsert {
            listing_source_id,
            domain_id,
            artifact_type,
            status,
            reason,
            candidate_payload,
            validation_summary,
            notes,
        } = input;
        sqlx::query_scalar::<_, uuid::Uuid>(
            "INSERT INTO crawler_reviews (
                listing_source_id, domain_id, artifact_type, status, reason, candidate_payload,
                validation_summary, reviewer_notes, reviewed
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8,
                CASE WHEN $4 = 'PENDING_REVIEW' THEN NULL ELSE NOW() END
             )
             RETURNING review_id",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(domain_id.copied())
        .bind(artifact_type)
        .bind(status)
        .bind(reason)
        .bind(candidate_payload)
        .bind(validation_summary)
        .bind(notes)
        .fetch_one(&self.pool)
        .await
    }

    async fn insert_review_pages(
        &self,
        review_id: uuid::Uuid,
        pages: Vec<SchemaReviewPageInput>,
    ) -> Result<(), sqlx::Error> {
        for page in pages {
            let html_hash = sha256_hex(page.raw_html.as_bytes());
            sqlx::query(
                "INSERT INTO crawler_review_pages (
                    review_id, url, role, html_hash
                 ) VALUES ($1, $2, $3, $4)",
            )
            .bind(review_id)
            .bind(page.url)
            .bind(page.role)
            .bind(html_hash)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    async fn set_review_status(
        &self,
        review_id: uuid::Uuid,
        status: &str,
        notes: Option<&str>,
    ) -> Result<(), ReviewRepositoryError> {
        let result = sqlx::query(
            "UPDATE crawler_reviews
             SET status = $2, reviewer_notes = COALESCE($3, reviewer_notes),
                 reviewed = NOW(), updated = NOW()
             WHERE review_id = $1 AND status = 'PENDING_REVIEW'",
        )
        .bind(review_id)
        .bind(status)
        .bind(notes)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(ReviewRepositoryError::NotPending(review_id));
        }
        Ok(())
    }

    async fn supersede_other_pending_reviews(
        &self,
        listing_source_id: &ListingSourceId,
        artifact_type: &str,
        approved_review_id: uuid::Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE crawler_reviews
             SET status = 'SUPERSEDED', reviewed = NOW(), updated = NOW()
             WHERE listing_source_id = $1
               AND artifact_type = $2
               AND status = 'PENDING_REVIEW'
               AND review_id <> $3",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(artifact_type)
        .bind(approved_review_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn clear_pending_blocks(&self, review: &CrawlerReview) -> Result<(), sqlx::Error> {
        match review.artifact_type.as_str() {
            ARTIFACT_PRODUCT_SCHEMA => {
                sqlx::query(
                    "UPDATE listing_source_urls
                     SET next_retry_at = NULL,
                         last_error_kind = NULL,
                         last_error_message = NULL,
                         updated = NOW()
                     WHERE listing_source_id = $1
                       AND last_error_kind = 'PendingSchemaReview'",
                )
                .bind(uuid::Uuid::from(review.listing_source_id))
                .execute(&self.pool)
                .await?;
            }
            ARTIFACT_URL_PATTERN => {
                sqlx::query(
                    "UPDATE listing_source_domains
                     SET next_crawl_at = NULL,
                         last_crawl_error_kind = NULL
                     WHERE listing_source_id = $1
                       AND last_crawl_error_kind = 'PendingUrlPatternReview'",
                )
                .bind(uuid::Uuid::from(review.listing_source_id))
                .execute(&self.pool)
                .await?;
            }
            _ => {}
        }
        Ok(())
    }
}

fn row_to_review(row: sqlx::postgres::PgRow) -> Result<CrawlerReview, sqlx::Error> {
    Ok(CrawlerReview {
        review_id: row.try_get("review_id")?,
        listing_source_id: ListingSourceId::from(
            row.try_get::<uuid::Uuid, _>("listing_source_id")?,
        ),
        listing_source_name: row.try_get("listing_source_name")?,
        domain_id: row.try_get("domain_id")?,
        artifact_type: row.try_get("artifact_type")?,
        status: row.try_get("status")?,
        reason: row.try_get("reason")?,
        candidate_payload: row.try_get("candidate_payload")?,
        validation_summary: row.try_get("validation_summary")?,
        reviewer_notes: row.try_get("reviewer_notes")?,
        created: row.try_get("created")?,
        updated: row.try_get("updated")?,
        reviewed: row.try_get("reviewed")?,
    })
}

fn row_to_page(row: sqlx::postgres::PgRow) -> Result<CrawlerReviewPage, sqlx::Error> {
    Ok(CrawlerReviewPage {
        review_page_id: row.try_get("review_page_id")?,
        review_id: row.try_get("review_id")?,
        url: row.try_get("url")?,
        role: row.try_get("role")?,
        html_hash: row.try_get("html_hash")?,
        fetched: row.try_get("fetched")?,
    })
}

fn row_to_review_url(row: sqlx::postgres::PgRow) -> Result<CrawlerReviewUrl, sqlx::Error> {
    Ok(CrawlerReviewUrl {
        review_url_id: row.try_get("review_url_id")?,
        review_id: row.try_get("review_id")?,
        url: row.try_get("url")?,
        previous_class: row.try_get("previous_class")?,
        current_pattern_match: row.try_get("current_pattern_match")?,
        candidate_pattern_match: row.try_get("candidate_pattern_match")?,
        candidate_class: row.try_get("candidate_class")?,
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn append_manual_schema_edit(
    mut validation_summary: serde_json::Value,
    schema_count: usize,
) -> serde_json::Value {
    let edit = json!({
        "at": OffsetDateTime::now_utc().to_string(),
        "source": "review_console",
        "operation": "approved_schema_live_update",
        "schema_count": schema_count,
    });

    if !validation_summary.is_object() {
        validation_summary = json!({ "summary": validation_summary });
    }

    if let Some(object) = validation_summary.as_object_mut() {
        let edits = object
            .entry("manual_schema_edits")
            .or_insert_with(|| json!([]));
        if let Some(edits) = edits.as_array_mut() {
            edits.push(edit);
        } else {
            *edits = json!([edit]);
        }
    }
    validation_summary
}

fn classify_with_pattern(raw_url: &str, pattern: Option<&Regex>) -> String {
    let Ok(url) = Url::parse(raw_url) else {
        return "other".to_string();
    };
    CrawledUrl::new(url).classify(pattern).to_string()
}

#[cfg(test)]
mod tests {
    use super::append_manual_schema_edit;
    use serde_json::json;

    #[test]
    fn appends_manual_schema_edit_audit_entry() {
        let summary = json!({ "auto_schema_evaluation": { "confidence": "HIGH" } });

        let updated = append_manual_schema_edit(summary, 2);

        let edits = updated
            .get("manual_schema_edits")
            .and_then(serde_json::Value::as_array)
            .expect("manual edits should be an array");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0]["source"], "review_console");
        assert_eq!(edits[0]["operation"], "approved_schema_live_update");
        assert_eq!(edits[0]["schema_count"], 2);
        assert_eq!(
            updated["auto_schema_evaluation"]["confidence"],
            json!("HIGH")
        );
    }
}
