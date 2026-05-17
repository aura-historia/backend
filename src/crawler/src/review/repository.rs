use crate::scraper::css_selector::product_schema::{ProductCssSelectorSchema, RawExtractedProduct};
use crate::scraper::css_selector::product_schema_repository::{
    ShopsProductSchemaRepository, ShopsProductSchemaRepositoryImpl,
};
use crate::scraper::css_selector::product_schema_service::clean_html_for_schema_generation;
use crate::spider::utils::url::CrawledUrl;
use common::shop_id::ShopId;
use regex::Regex;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use url::Url;

pub const STATUS_PENDING_REVIEW: &str = "PENDING_REVIEW";
pub const STATUS_APPROVED: &str = "APPROVED";
pub const STATUS_REJECTED: &str = "REJECTED";
pub const STATUS_NEEDS_REPAIR: &str = "NEEDS_REPAIR";
pub const STATUS_SUPERSEDED: &str = "SUPERSEDED";

pub const ARTIFACT_URL_PATTERN: &str = "URL_PATTERN";
pub const ARTIFACT_PRODUCT_SCHEMA: &str = "PRODUCT_SCHEMA";

pub const PAGE_ROLE_PRIMARY: &str = "PRIMARY";
pub const PAGE_ROLE_SEED: &str = "SEED";
pub const PAGE_ROLE_TRIGGERING_REPAIR_PAGE: &str = "TRIGGERING_REPAIR_PAGE";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlerReview {
    pub review_id: uuid::Uuid,
    pub shop_id: ShopId,
    pub shop_name: Option<String>,
    pub domain_id: Option<uuid::Uuid>,
    pub artifact_type: String,
    pub status: String,
    pub reason: String,
    pub candidate_payload: serde_json::Value,
    pub validation_summary: serde_json::Value,
    pub reviewer_notes: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reviewed: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlerReviewPage {
    pub review_page_id: uuid::Uuid,
    pub review_id: uuid::Uuid,
    pub url: String,
    pub role: String,
    pub raw_html: String,
    pub cleaned_html: String,
    pub html_hash: String,
    #[serde(with = "time::serde::rfc3339")]
    pub fetched: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlerReviewUrl {
    pub review_url_id: uuid::Uuid,
    pub review_id: uuid::Uuid,
    pub url: String,
    pub previous_class: Option<String>,
    pub current_pattern_match: Option<bool>,
    pub candidate_pattern_match: Option<bool>,
    pub candidate_class: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SchemaReviewPageInput {
    pub url: String,
    pub role: String,
    pub raw_html: String,
}

#[derive(Debug, Serialize)]
pub struct ReviewDetail {
    pub review: CrawlerReview,
    pub pages: Vec<CrawlerReviewPage>,
    pub urls: Vec<CrawlerReviewUrl>,
}

#[derive(Debug, Serialize)]
pub struct ShopOverview {
    pub shop_id: ShopId,
    pub shop_name: Option<String>,
    pub llm_calls_count: i64,
    pub url_pattern: Option<String>,
    pub pending_reviews: i64,
    pub product_urls: i64,
    pub blocked_urls: i64,
}

#[derive(Debug, Serialize)]
pub struct SelectorFieldEvaluation {
    pub field: String,
    pub selector: String,
    pub selector_match_count: Option<usize>,
    pub additional_selector_match_counts: Vec<usize>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SchemaPageEvaluation {
    pub page_id: uuid::Uuid,
    pub url: String,
    pub role: String,
    pub apply_ok: bool,
    pub extracted: Option<RawExtractedProduct>,
    pub error: Option<String>,
    pub fields: Vec<SelectorFieldEvaluation>,
}

#[derive(Debug, Serialize)]
pub struct SchemaCandidateEvaluation {
    pub schema_index: usize,
    pub pages: Vec<SchemaPageEvaluation>,
}

#[derive(Debug, Serialize)]
pub struct SchemaMatrix {
    pub review_id: uuid::Uuid,
    pub candidates: Vec<SchemaCandidateEvaluation>,
}

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
}

#[derive(Clone)]
pub struct CrawlerReviewRepository {
    pool: PgPool,
}

impl CrawlerReviewRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn has_pending_review(
        &self,
        shop_id: &ShopId,
        artifact_type: &str,
    ) -> Result<bool, sqlx::Error> {
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM crawler_reviews
                WHERE shop_id = $1 AND artifact_type = $2 AND status = 'PENDING_REVIEW'
            )",
        )
        .bind(uuid::Uuid::from(*shop_id))
        .bind(artifact_type)
        .fetch_one(&self.pool)
        .await?;
        Ok(exists)
    }

    pub async fn latest_pending_review_id(
        &self,
        shop_id: &ShopId,
        artifact_type: &str,
    ) -> Result<Option<uuid::Uuid>, sqlx::Error> {
        sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT review_id
             FROM crawler_reviews
             WHERE shop_id = $1 AND artifact_type = $2 AND status = 'PENDING_REVIEW'
             ORDER BY created DESC
             LIMIT 1",
        )
        .bind(uuid::Uuid::from(*shop_id))
        .bind(artifact_type)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn list_reviews(&self, limit: i64) -> Result<Vec<CrawlerReview>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT r.review_id, r.shop_id, s.shop_name, r.domain_id, r.artifact_type, r.status, r.reason,
                    r.candidate_payload, r.validation_summary, r.reviewer_notes, r.created, r.updated, r.reviewed
             FROM crawler_reviews r
             LEFT JOIN shops s ON s.shop_id = r.shop_id
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

    pub async fn list_shops(&self, limit: i64) -> Result<Vec<ShopOverview>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT
                s.shop_id,
                s.shop_name,
                s.llm_calls_count,
                s.url_pattern,
                COALESCE(r.pending_reviews, 0) AS pending_reviews,
                COALESCE(u.product_urls, 0) AS product_urls,
                COALESCE(u.blocked_urls, 0) AS blocked_urls
             FROM shops s
             LEFT JOIN (
                SELECT shop_id, COUNT(*) AS pending_reviews
                FROM crawler_reviews
                WHERE status = 'PENDING_REVIEW'
                GROUP BY shop_id
             ) r ON r.shop_id = s.shop_id
             LEFT JOIN (
                SELECT shop_id,
                       COUNT(*) FILTER (WHERE url_class = 'product') AS product_urls,
                       COUNT(*) FILTER (WHERE last_error_kind IN ('PendingSchemaReview', 'PendingUrlPatternReview')) AS blocked_urls
                FROM shop_urls
                GROUP BY shop_id
             ) u ON u.shop_id = s.shop_id
             WHERE s.active = TRUE
             ORDER BY pending_reviews DESC, s.updated DESC
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let mut shops = Vec::with_capacity(rows.len());
        for row in rows {
            shops.push(ShopOverview {
                shop_id: ShopId::from(row.try_get::<uuid::Uuid, _>("shop_id")?),
                shop_name: row.try_get("shop_name")?,
                llm_calls_count: row.try_get("llm_calls_count")?,
                url_pattern: row.try_get("url_pattern")?,
                pending_reviews: row.try_get("pending_reviews")?,
                product_urls: row.try_get("product_urls")?,
                blocked_urls: row.try_get("blocked_urls")?,
            });
        }
        Ok(shops)
    }

    pub async fn get_review(
        &self,
        review_id: uuid::Uuid,
    ) -> Result<ReviewDetail, ReviewRepositoryError> {
        let row = sqlx::query(
            "SELECT r.review_id, r.shop_id, s.shop_name, r.domain_id, r.artifact_type, r.status, r.reason,
                    r.candidate_payload, r.validation_summary, r.reviewer_notes, r.created, r.updated, r.reviewed
             FROM crawler_reviews r
             LEFT JOIN shops s ON s.shop_id = r.shop_id
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
            "SELECT review_page_id, review_id, url, role, raw_html, cleaned_html, html_hash, fetched
             FROM crawler_review_pages
             WHERE review_id = $1
             ORDER BY
               CASE role
                 WHEN 'PRIMARY' THEN 0
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

    pub async fn get_review_page_html(
        &self,
        review_page_id: uuid::Uuid,
    ) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar::<_, String>(
            "SELECT raw_html FROM crawler_review_pages WHERE review_page_id = $1",
        )
        .bind(review_page_id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn get_review_page(
        &self,
        review_page_id: uuid::Uuid,
    ) -> Result<Option<CrawlerReviewPage>, sqlx::Error> {
        sqlx::query(
            "SELECT review_page_id, review_id, url, role, raw_html, cleaned_html, html_hash, fetched
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
        shop_id: &ShopId,
        domain_id: Option<&uuid::Uuid>,
        reason: &str,
        candidate_pattern: Option<&Regex>,
        urls: &[String],
        current_pattern: Option<&Regex>,
    ) -> Result<uuid::Uuid, ReviewRepositoryError> {
        if self
            .has_pending_review(shop_id, ARTIFACT_URL_PATTERN)
            .await?
        {
            return self
                .latest_pending_review_id(shop_id, ARTIFACT_URL_PATTERN)
                .await?
                .ok_or_else(|| ReviewRepositoryError::Database(sqlx::Error::RowNotFound));
        }

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

        let review_id = self
            .insert_review(
                shop_id,
                domain_id,
                ARTIFACT_URL_PATTERN,
                reason,
                candidate_payload,
                validation_summary,
            )
            .await?;

        for raw_url in urls {
            let current_match = current_pattern.map(|pattern| pattern.is_match(raw_url));
            let candidate_match = candidate_pattern.map(|pattern| pattern.is_match(raw_url));
            let candidate_class = classify_with_pattern(raw_url, candidate_pattern);
            sqlx::query(
                "INSERT INTO crawler_review_urls (
                    review_id, url, current_pattern_match, candidate_pattern_match, candidate_class
                 ) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(review_id)
            .bind(raw_url)
            .bind(current_match)
            .bind(candidate_match)
            .bind(candidate_class)
            .execute(&self.pool)
            .await?;
        }

        Ok(review_id)
    }

    pub async fn create_schema_review(
        &self,
        shop_id: &ShopId,
        reason: &str,
        schemas: &[ProductCssSelectorSchema],
        pages: Vec<SchemaReviewPageInput>,
        validation_summary: serde_json::Value,
    ) -> Result<uuid::Uuid, ReviewRepositoryError> {
        if self
            .has_pending_review(shop_id, ARTIFACT_PRODUCT_SCHEMA)
            .await?
        {
            return self
                .latest_pending_review_id(shop_id, ARTIFACT_PRODUCT_SCHEMA)
                .await?
                .ok_or_else(|| ReviewRepositoryError::Database(sqlx::Error::RowNotFound));
        }

        let candidate_payload = json!({ "schemas": schemas });
        let review_id = self
            .insert_review(
                shop_id,
                None,
                ARTIFACT_PRODUCT_SCHEMA,
                reason,
                candidate_payload,
                validation_summary,
            )
            .await?;

        for page in pages {
            let cleaned_html = clean_html_for_schema_generation(&page.raw_html);
            let html_hash = sha256_hex(page.raw_html.as_bytes());
            sqlx::query(
                "INSERT INTO crawler_review_pages (
                    review_id, url, role, raw_html, cleaned_html, html_hash
                 ) VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(review_id)
            .bind(page.url)
            .bind(page.role)
            .bind(page.raw_html)
            .bind(cleaned_html)
            .bind(html_hash)
            .execute(&self.pool)
            .await?;
        }

        Ok(review_id)
    }

    pub async fn update_candidate_payload(
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
                        "UPDATE shops
                         SET url_pattern = $2, updated = NOW()
                         WHERE shop_id = $1",
                    )
                    .bind(uuid::Uuid::from(detail.review.shop_id))
                    .bind(pattern)
                    .execute(&self.pool)
                    .await?;
                }
            }
            ARTIFACT_PRODUCT_SCHEMA => {
                let reviewed_schemas = parse_schemas_payload(&detail.review.candidate_payload)?;
                let repo = ShopsProductSchemaRepositoryImpl::new(&self.pool);
                let existing = repo.find_product_schema(&detail.review.shop_id).await?;
                let schemas = approval_product_schemas(
                    &detail.review.reason,
                    existing.as_ref(),
                    reviewed_schemas,
                )?;
                if existing.is_some() {
                    repo.update_product_schema(&detail.review.shop_id, &schemas)
                        .await?;
                } else {
                    let now = OffsetDateTime::now_utc();
                    let schema = crate::scraper::css_selector::product_schema::ShopsProductSchema {
                        shop_id: detail.review.shop_id,
                        product_schemas: schemas,
                        created: now,
                        updated: now,
                    };
                    repo.insert_product_schema(&detail.review.shop_id, &schema)
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
            &detail.review.shop_id,
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

    pub async fn evaluate_schema_matrix(
        &self,
        review_id: uuid::Uuid,
    ) -> Result<SchemaMatrix, ReviewRepositoryError> {
        let detail = self.get_review(review_id).await?;
        let schemas = parse_schemas_payload(&detail.review.candidate_payload)?;

        let mut candidates = Vec::with_capacity(schemas.len());
        for (schema_index, schema) in schemas.iter().enumerate() {
            let mut pages = Vec::with_capacity(detail.pages.len());
            for page in &detail.pages {
                pages.push(evaluate_schema_page(schema, page));
            }
            candidates.push(SchemaCandidateEvaluation {
                schema_index,
                pages,
            });
        }

        Ok(SchemaMatrix {
            review_id,
            candidates,
        })
    }

    pub async fn trigger_crawl_now(&self, shop_id: ShopId) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE shop_domains
             SET next_crawl_at = NULL,
                 last_crawl_error_kind = NULL
             WHERE shop_id = $1",
        )
        .bind(uuid::Uuid::from(shop_id))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn trigger_scrape_now(&self, shop_id: ShopId) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE shop_urls
             SET next_retry_at = NULL,
                 last_error_kind = NULL,
                 last_error_message = NULL,
                 updated = NOW()
             WHERE shop_id = $1
               AND url_class = 'product'",
        )
        .bind(uuid::Uuid::from(shop_id))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn trigger_url_pattern_regeneration(
        &self,
        shop_id: ShopId,
    ) -> Result<u64, sqlx::Error> {
        sqlx::query(
            "UPDATE shops
             SET url_pattern = NULL,
                 updated = NOW()
             WHERE shop_id = $1",
        )
        .bind(uuid::Uuid::from(shop_id))
        .execute(&self.pool)
        .await?;

        self.trigger_crawl_now(shop_id).await
    }

    pub async fn trigger_schema_regeneration(&self, shop_id: ShopId) -> Result<u64, sqlx::Error> {
        sqlx::query("DELETE FROM shops_product_schema WHERE shop_id = $1")
            .bind(uuid::Uuid::from(shop_id))
            .execute(&self.pool)
            .await?;

        self.trigger_scrape_now(shop_id).await
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

        let repo = ShopsProductSchemaRepositoryImpl::new(&self.pool);
        let Some(existing) = repo.find_product_schema(&review.shop_id).await? else {
            return Ok(());
        };
        let merged = merge_product_schema_lists(&existing.product_schemas, reviewed_schemas)?;
        review.candidate_payload = json!({ "schemas": merged });
        Ok(())
    }

    async fn insert_review(
        &self,
        shop_id: &ShopId,
        domain_id: Option<&uuid::Uuid>,
        artifact_type: &str,
        reason: &str,
        candidate_payload: serde_json::Value,
        validation_summary: serde_json::Value,
    ) -> Result<uuid::Uuid, sqlx::Error> {
        sqlx::query_scalar::<_, uuid::Uuid>(
            "INSERT INTO crawler_reviews (
                shop_id, domain_id, artifact_type, status, reason, candidate_payload, validation_summary
             ) VALUES ($1, $2, $3, 'PENDING_REVIEW', $4, $5, $6)
             RETURNING review_id",
        )
        .bind(uuid::Uuid::from(*shop_id))
        .bind(domain_id.copied())
        .bind(artifact_type)
        .bind(reason)
        .bind(candidate_payload)
        .bind(validation_summary)
        .fetch_one(&self.pool)
        .await
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
        shop_id: &ShopId,
        artifact_type: &str,
        approved_review_id: uuid::Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE crawler_reviews
             SET status = 'SUPERSEDED', reviewed = NOW(), updated = NOW()
             WHERE shop_id = $1
               AND artifact_type = $2
               AND status = 'PENDING_REVIEW'
               AND review_id <> $3",
        )
        .bind(uuid::Uuid::from(*shop_id))
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
                    "UPDATE shop_urls
                     SET next_retry_at = NULL,
                         last_error_kind = NULL,
                         last_error_message = NULL,
                         updated = NOW()
                     WHERE shop_id = $1
                       AND last_error_kind = 'PendingSchemaReview'",
                )
                .bind(uuid::Uuid::from(review.shop_id))
                .execute(&self.pool)
                .await?;
            }
            ARTIFACT_URL_PATTERN => {
                sqlx::query(
                    "UPDATE shop_domains
                     SET next_crawl_at = NULL,
                         last_crawl_error_kind = NULL
                     WHERE shop_id = $1
                       AND last_crawl_error_kind = 'PendingUrlPatternReview'",
                )
                .bind(uuid::Uuid::from(review.shop_id))
                .execute(&self.pool)
                .await?;
            }
            _ => {}
        }
        Ok(())
    }
}

fn approval_product_schemas(
    reason: &str,
    existing: Option<&crate::scraper::css_selector::product_schema::ShopsProductSchema>,
    reviewed_schemas: Vec<ProductCssSelectorSchema>,
) -> Result<Vec<ProductCssSelectorSchema>, ReviewRepositoryError> {
    let should_backfill_existing = matches!(
        reason,
        "append_schema_generation" | "normalization_schema_repair"
    ) && reviewed_schemas.len() == 1;

    if !should_backfill_existing {
        return Ok(reviewed_schemas);
    }

    let Some(existing) = existing else {
        return Ok(reviewed_schemas);
    };

    merge_product_schema_lists(&existing.product_schemas, reviewed_schemas)
}

fn merge_product_schema_lists(
    existing_schemas: &[ProductCssSelectorSchema],
    reviewed_schemas: Vec<ProductCssSelectorSchema>,
) -> Result<Vec<ProductCssSelectorSchema>, ReviewRepositoryError> {
    let mut merged = existing_schemas.to_vec();
    let mut seen = existing_schemas
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()?;

    for schema in reviewed_schemas {
        let key = serde_json::to_value(&schema)?;
        if !seen.contains(&key) {
            seen.push(key);
            merged.push(schema);
        }
    }

    Ok(merged)
}

fn row_to_review(row: sqlx::postgres::PgRow) -> Result<CrawlerReview, sqlx::Error> {
    Ok(CrawlerReview {
        review_id: row.try_get("review_id")?,
        shop_id: ShopId::from(row.try_get::<uuid::Uuid, _>("shop_id")?),
        shop_name: row.try_get("shop_name")?,
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
        raw_html: row.try_get("raw_html")?,
        cleaned_html: row.try_get("cleaned_html")?,
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

fn parse_schemas_payload(
    candidate_payload: &serde_json::Value,
) -> Result<Vec<ProductCssSelectorSchema>, serde_json::Error> {
    serde_json::from_value(
        candidate_payload
            .get("schemas")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::Array(Vec::new())),
    )
}

fn evaluate_schema_page(
    schema: &ProductCssSelectorSchema,
    page: &CrawlerReviewPage,
) -> SchemaPageEvaluation {
    let html = Html::parse_document(&page.raw_html);
    let apply_result = schema.apply(&html);
    let fields = evaluate_schema_fields(schema, &page.raw_html);

    match apply_result {
        Ok(extracted) => SchemaPageEvaluation {
            page_id: page.review_page_id,
            url: page.url.clone(),
            role: page.role.clone(),
            apply_ok: true,
            extracted: Some(extracted),
            error: None,
            fields,
        },
        Err(err) => SchemaPageEvaluation {
            page_id: page.review_page_id,
            url: page.url.clone(),
            role: page.role.clone(),
            apply_ok: false,
            extracted: None,
            error: Some(err.to_string()),
            fields,
        },
    }
}

fn evaluate_schema_fields(
    schema: &ProductCssSelectorSchema,
    html: &str,
) -> Vec<SelectorFieldEvaluation> {
    let mut fields = Vec::new();
    fields.push(evaluate_rule(
        "shops_product_id",
        &schema.shops_product_id,
        html,
    ));
    fields.push(evaluate_rule("title", &schema.title, html));
    if let Some(rule) = &schema.description {
        fields.push(evaluate_rule("description", rule, html));
    }
    if let Some(rule) = &schema.price {
        fields.push(evaluate_rule("price", rule, html));
    }
    if let Some(rule) = &schema.price_estimate_min {
        fields.push(evaluate_rule("price_estimate_min", rule, html));
    }
    if let Some(rule) = &schema.price_estimate_max {
        fields.push(evaluate_rule("price_estimate_max", rule, html));
    }
    fields.push(evaluate_rule("state", &schema.state, html));
    fields.push(evaluate_rule("images", &schema.images, html));
    if let Some(rule) = &schema.auction_start {
        fields.push(evaluate_rule("auction_start", rule, html));
    }
    if let Some(rule) = &schema.auction_end {
        fields.push(evaluate_rule("auction_end", rule, html));
    }
    fields
}

fn evaluate_rule(
    field: &str,
    rule: &crate::scraper::css_selector::rule::ExtractionRule,
    html: &str,
) -> SelectorFieldEvaluation {
    let document = Html::parse_document(html);
    let selector = rule.selector.to_string();
    let selector_match_count = match Selector::parse(&selector) {
        Ok(parsed) => Some(document.select(&parsed).count()),
        Err(err) => {
            let error = format!("{err:?}");
            return SelectorFieldEvaluation {
                field: field.to_string(),
                selector: selector.clone(),
                selector_match_count: None,
                additional_selector_match_counts: Vec::new(),
                error: Some(error),
            };
        }
    };

    let mut additional_selector_match_counts = Vec::new();
    for additional in &rule.additional_selectors {
        match Selector::parse(additional.as_ref()) {
            Ok(parsed) => additional_selector_match_counts.push(document.select(&parsed).count()),
            Err(_) => additional_selector_match_counts.push(0),
        }
    }

    SelectorFieldEvaluation {
        field: field.to_string(),
        selector,
        selector_match_count,
        additional_selector_match_counts,
        error: None,
    }
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

fn classify_with_pattern(raw_url: &str, pattern: Option<&Regex>) -> String {
    let Ok(url) = Url::parse(raw_url) else {
        return "other".to_string();
    };
    CrawledUrl::new(url).classify(pattern).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::css_selector::rule::{
        CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule,
    };

    fn text_rule(selector: &str) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: vec![],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        }
    }

    fn image_rule(selector: &str) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: vec![],
            extract: ExtractionKind::Attribute { name: "src".into() },
            cardinality: ExtractionCardinality::All,
        }
    }

    fn schema(title_selector: &str) -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule(title_selector),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: image_rule("img"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
        }
    }

    #[test]
    fn merge_product_schema_lists_appends_new_schema_after_existing_schemas() {
        let existing_a = schema("h1.template-a");
        let existing_b = schema("h1.template-b");
        let appended = schema("h1.template-c");

        let merged = merge_product_schema_lists(
            &[existing_a.clone(), existing_b.clone()],
            vec![appended.clone()],
        )
        .expect("schema merge should serialize");

        assert_eq!(merged, vec![existing_a, existing_b, appended]);
    }

    #[test]
    fn merge_product_schema_lists_deduplicates_existing_schema() {
        let existing = schema("h1.template-a");

        let merged =
            merge_product_schema_lists(std::slice::from_ref(&existing), vec![existing.clone()])
                .expect("schema merge should serialize");

        assert_eq!(merged, vec![existing]);
    }
}
