use crate::network::policy::url_matches_configured_domain;
use crate::review::model::*;
use crate::review::schema_evaluation::evaluate_schema_matrix_for_live_review_pages;
use crate::scraper::css_selector::product_schema::{
    ListingSourceProductSchema, ProductCssSelectorSchema,
};
use crate::scraper::css_selector::product_schema_repository::{
    ListingSourceProductSchemaRepository, ListingSourceProductSchemaRepositoryImpl,
};
use crate::scraper::css_selector::rule::ExtractionRule;
use crate::scraper::scraper_service::pipeline::scrape_product::is_redirect_to_non_product_page;
use crate::spider::utils::url::CrawledUrl;
use crate::{CrawlerDomainId, CrawlerReviewId, CrawlerReviewPageId, CrawlerReviewUrlId};
use listing_source_core::ListingSourceId;
use regex::Regex;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use tracing::info;
use url::Url;

mod reviewer_page_url;
mod schema_payload;

use reviewer_page_url::{
    ReviewPageIdentity, is_allowed_reviewer_page_url, reviewer_page_url, without_credentials,
};
use schema_payload::{
    approval_product_schemas, decode_stored_schemas_payload, parse_schemas_payload,
    update_schema_field_payload, validate_product_schemas,
};

#[derive(Debug, thiserror::Error)]
pub enum ReviewRepositoryError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Regex(#[from] regex::Error),
    #[error("review not found: {0}")]
    NotFound(CrawlerReviewId),
    #[error("review {0} is not pending review")]
    NotPending(CrawlerReviewId),
    #[error("review {0} has unsupported artifact type {1}")]
    UnsupportedArtifact(CrawlerReviewId, String),
    #[error("invalid schema field `{0}`")]
    InvalidSchemaField(String),
    #[error("field `{0}` is required and cannot be deleted")]
    RequiredSchemaField(String),
    #[error("invalid URL-pattern review candidate")]
    InvalidUrlPatternCandidate,
    #[error("invalid ProductSchema review candidate")]
    InvalidProductSchemaCandidate,
    #[error("review candidate or pages changed during schema-matrix evaluation")]
    CandidateChangedDuringEvaluation,
    #[error(
        "approved review {0} no longer holds the live product schemas; edit the review whose candidate is live"
    )]
    StaleApprovedReview(CrawlerReviewId),
    #[error(
        "URL `{0}` is not an http(s) URL on the default port, without credentials, on a domain of the review's ListingSource"
    )]
    ReviewPageUrlNotAllowed(String),
    #[error("URL `{0}` is already a page of this review")]
    DuplicateReviewPage(String),
    #[error(
        "review {0} already has the maximum of {MAX_REVIEWER_ADDED_PAGES} reviewer-added pages"
    )]
    ReviewPageLimitReached(CrawlerReviewId),
    #[error("URL redirected to `{0}`, which is not a product page")]
    RedirectedToNonProductPage(String),
    #[error("review page not found: {0}")]
    ReviewPageNotFound(CrawlerReviewPageId),
    #[error("review page {0} is a seed or triggering page of the review and cannot be removed")]
    NotReviewerAddedPage(CrawlerReviewPageId),
}

/// Upper bound on the pages a reviewer adds to one review. A live schema-matrix refresh refetches
/// every page, seed pages included, one by one from the same throttled shop within the review
/// API's 30-second request deadline, so reviewer pages may only add a few fetches.
pub const MAX_REVIEWER_ADDED_PAGES: usize = 4;

/// Upper bound on the pages that wait on one pending review. They show the reviewer the template
/// variants that need the schema, but every matrix refresh refetches them too; further failing
/// pages still wait on the review without being attached.
pub const MAX_WAITING_PAGES: usize = 6;

#[derive(Clone)]
pub struct CrawlerReviewRepository {
    pool: PgPool,
}

pub struct EvaluatedSchemaMatrix {
    pub matrix: SchemaMatrix,
    candidate_version: i64,
    candidate_hash: String,
    /// The review pages the matrix covers, sorted so it compares with the stored page set.
    page_ids: Vec<uuid::Uuid>,
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
    ) -> Result<Option<CrawlerReviewId>, sqlx::Error> {
        sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT review_id FROM crawler_reviews \
             WHERE listing_source_id = $1 AND artifact_type = $2 AND status = 'PENDING_REVIEW' \
             ORDER BY created DESC LIMIT 1",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(artifact_type)
        .fetch_optional(&self.pool)
        .await?
        .map(crawler_review_id_from_uuid)
        .transpose()
    }

    /// Whether an operator rejected a product schema review opened for `reason` over `url`.
    pub async fn has_rejected_schema_review_for_page(
        &self,
        listing_source_id: &ListingSourceId,
        reason: &str,
        url: &str,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM crawler_reviews r \
             JOIN crawler_review_pages p ON p.review_id = r.review_id \
             WHERE r.listing_source_id = $1 AND r.artifact_type = 'PRODUCT_SCHEMA' \
               AND r.status = 'REJECTED' AND r.reason = $2 AND p.url = $3 AND p.role <> ALL($4))",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(reason)
        .bind(url)
        // Reviewer-added and waiting pages only test the candidate; rejecting the review is no
        // verdict on them.
        .bind([PAGE_ROLE_REVIEWER_ADDED, PAGE_ROLE_WAITING_PAGE])
        .fetch_one(&self.pool)
        .await
    }

    pub async fn has_pending_url_pattern_review(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM crawler_reviews \
             WHERE listing_source_id = $1 AND domain_id = $2 \
               AND artifact_type = 'URL_PATTERN' AND status = 'PENDING_REVIEW')",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(domain_id.as_uuid())
        .fetch_one(&self.pool)
        .await
    }

    pub async fn latest_pending_url_pattern_review_id(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<Option<CrawlerReviewId>, sqlx::Error> {
        sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT review_id FROM crawler_reviews \
             WHERE listing_source_id = $1 AND domain_id = $2 \
               AND artifact_type = 'URL_PATTERN' AND status = 'PENDING_REVIEW' \
             ORDER BY created DESC LIMIT 1",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(domain_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?
        .map(crawler_review_id_from_uuid)
        .transpose()
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
                s.crawl_enabled,
                s.llm_calls_count,
                COALESCE(d.domain_count, 0) AS domain_count,
                d.last_successful_crawl,
                COALESCE(r.pending_reviews, 0) AS pending_reviews,
                COALESCE(u.product_urls, 0) AS product_urls,
                COALESCE(u.blocked_urls, 0) AS blocked_urls
             FROM listing_sources s
             LEFT JOIN (
                SELECT listing_source_id, COUNT(*) AS domain_count, MAX(last_crawled) AS last_successful_crawl
                FROM listing_source_domains GROUP BY listing_source_id
             ) d ON d.listing_source_id = s.listing_source_id
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
             ORDER BY s.crawl_enabled DESC, pending_reviews DESC, s.updated DESC
             LIMIT $1",
        )
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;

        let mut listing_sources = Vec::with_capacity(rows.len());
        for row in rows {
            listing_sources.push(ListingSourceOverview {
                listing_source_id: listing_source_id_from_uuid(
                    row.try_get::<uuid::Uuid, _>("listing_source_id")?,
                )?,
                listing_source_name: row.try_get("listing_source_name")?,
                crawl_enabled: row.try_get("crawl_enabled")?,
                llm_calls_count: row.try_get("llm_calls_count")?,
                domain_count: row.try_get("domain_count")?,
                last_successful_crawl: row.try_get("last_successful_crawl")?,
                pending_reviews: row.try_get("pending_reviews")?,
                product_urls: row.try_get("product_urls")?,
                blocked_urls: row.try_get("blocked_urls")?,
            });
        }
        Ok(listing_sources)
    }

    pub async fn get_review(
        &self,
        review_id: CrawlerReviewId,
    ) -> Result<ReviewDetail, ReviewRepositoryError> {
        let row = sqlx::query(
            "SELECT r.review_id, r.listing_source_id, s.listing_source_name, r.domain_id, r.artifact_type, r.status, r.reason,
                    r.candidate_payload, r.validation_summary, r.reviewer_notes, r.created, r.updated, r.reviewed
             FROM crawler_reviews r
             LEFT JOIN listing_sources s ON s.listing_source_id = r.listing_source_id
             WHERE r.review_id = $1",
        )
            .bind(review_id.as_uuid())
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
        review_id: CrawlerReviewId,
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
        .bind(review_id.as_uuid())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_page).collect()
    }

    async fn get_review_urls(
        &self,
        review_id: CrawlerReviewId,
    ) -> Result<Vec<CrawlerReviewUrl>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT review_url_id, review_id, url, previous_class, current_pattern_match,
                    candidate_pattern_match, candidate_class
             FROM crawler_review_urls
             WHERE review_id = $1
             ORDER BY created",
        )
        .bind(review_id.as_uuid())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_review_url).collect()
    }

    pub async fn get_review_page(
        &self,
        review_page_id: CrawlerReviewPageId,
    ) -> Result<Option<CrawlerReviewPage>, sqlx::Error> {
        sqlx::query(
            "SELECT review_page_id, review_id, url, role, html_hash, fetched
             FROM crawler_review_pages
             WHERE review_page_id = $1",
        )
        .bind(review_page_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?
        .map(row_to_page)
        .transpose()
    }

    pub async fn create_url_pattern_review(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        reason: &str,
        decision: UrlPatternDecision,
        urls: &[String],
        current_pattern: Option<&Regex>,
    ) -> Result<CrawlerReviewId, ReviewRepositoryError> {
        let candidate_regex = match &decision {
            UrlPatternDecision::Pattern { value } => Some(
                Regex::new(value).map_err(|_| ReviewRepositoryError::InvalidUrlPatternCandidate)?,
            ),
            UrlPatternDecision::NoPattern | UrlPatternDecision::Undecided => None,
        };
        let candidate_pattern = candidate_regex.as_ref();
        let candidate_payload =
            serde_json::to_value(UrlPatternReviewCandidate::new(decision, current_pattern))?;
        let validation_summary = json!({
            "sample_count": urls.len(),
            "candidate_product_count": candidate_pattern
                .map(|pattern| urls.iter().filter(|url| pattern.is_match(url)).count())
                .unwrap_or(0),
        });
        let mut transaction = self.pool.begin().await?;
        let domain_owned = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM listing_source_domains \
             WHERE listing_source_id = $1 AND domain_id = $2 FOR KEY SHARE)",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(domain_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await?;
        if !domain_owned {
            return Err(ReviewRepositoryError::Database(sqlx::Error::RowNotFound));
        }
        let requested_review_id = CrawlerReviewId::new();
        let inserted = sqlx::query_scalar::<_, uuid::Uuid>(
            "INSERT INTO crawler_reviews (review_id, listing_source_id, domain_id, artifact_type, status, reason, candidate_payload, validation_summary, reviewer_notes, reviewed) \
             VALUES ($1, $2, $3, 'URL_PATTERN', 'PENDING_REVIEW', $4, $5, $6, NULL, NULL) \
             ON CONFLICT (domain_id) WHERE status = 'PENDING_REVIEW' AND artifact_type = 'URL_PATTERN' \
             DO NOTHING RETURNING review_id",
        )
        .bind(requested_review_id.as_uuid())
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(domain_id.as_uuid())
        .bind(reason)
        .bind(candidate_payload)
        .bind(validation_summary)
        .fetch_optional(&mut *transaction)
        .await?;
        let (review_id, is_new) = match inserted {
            Some(review_id) => (crawler_review_id_from_uuid(review_id)?, true),
            None => {
                let review_id = sqlx::query_scalar::<_, uuid::Uuid>(
                    "SELECT review_id FROM crawler_reviews \
                     WHERE listing_source_id = $1 AND domain_id = $2 \
                       AND artifact_type = 'URL_PATTERN' AND status = 'PENDING_REVIEW' \
                     ORDER BY created DESC LIMIT 1",
                )
                .bind(uuid::Uuid::from(*listing_source_id))
                .bind(domain_id.as_uuid())
                .fetch_one(&mut *transaction)
                .await?;
                (crawler_review_id_from_uuid(review_id)?, false)
            }
        };
        if is_new {
            for raw_url in urls {
                let current_match = current_pattern.map(|pattern| pattern.is_match(raw_url));
                let candidate_match = candidate_pattern.map(|pattern| pattern.is_match(raw_url));
                let candidate_class = classify_with_pattern(raw_url, candidate_pattern);
                let review_url_id = CrawlerReviewUrlId::new();
                sqlx::query(
                    "INSERT INTO crawler_review_urls (
                        review_url_id, review_id, url, current_pattern_match, candidate_pattern_match, candidate_class
                     ) VALUES ($1, $2, $3, $4, $5, $6)",
                )
                .bind(review_url_id.as_uuid())
                .bind(review_id.as_uuid())
                .bind(raw_url)
                .bind(current_match)
                .bind(candidate_match)
                .bind(candidate_class)
                .execute(&mut *transaction)
                .await?;
            }
        }
        transaction.commit().await?;
        Ok(review_id)
    }

    pub async fn create_schema_review(
        &self,
        listing_source_id: &ListingSourceId,
        reason: &str,
        schemas: &[ProductCssSelectorSchema],
        pages: Vec<SchemaReviewPageInput>,
        validation_summary: serde_json::Value,
    ) -> Result<CrawlerReviewId, ReviewRepositoryError> {
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
    ) -> Result<CrawlerReviewId, ReviewRepositoryError> {
        self.insert_schema_review(input).await
    }

    async fn insert_schema_review(
        &self,
        input: SchemaReviewWithStatusInput<'_>,
    ) -> Result<CrawlerReviewId, ReviewRepositoryError> {
        let SchemaReviewWithStatusInput {
            listing_source_id,
            reason,
            schemas,
            pages,
            validation_summary,
            status,
            notes,
        } = input;
        validate_product_schemas(schemas)?;
        let candidate_payload = json!({ "schemas": schemas });
        let requested_review_id = CrawlerReviewId::new();
        let mut transaction = self.pool.begin().await?;
        let inserted = if status == STATUS_PENDING_REVIEW {
            sqlx::query_scalar::<_, uuid::Uuid>(
                "INSERT INTO crawler_reviews (review_id, listing_source_id, domain_id, artifact_type, status, reason, candidate_payload, validation_summary, reviewer_notes, reviewed) \
                 VALUES ($1, $2, NULL, 'PRODUCT_SCHEMA', 'PENDING_REVIEW', $3, $4, $5, NULL, NULL) \
                 ON CONFLICT (listing_source_id) WHERE status = 'PENDING_REVIEW' AND artifact_type = 'PRODUCT_SCHEMA' \
                 DO NOTHING RETURNING review_id",
            )
            .bind(requested_review_id.as_uuid())
            .bind(uuid::Uuid::from(*listing_source_id))
            .bind(reason)
            .bind(candidate_payload)
            .bind(validation_summary)
            .fetch_optional(&mut *transaction)
            .await?
        } else {
            Some(
                sqlx::query_scalar::<_, uuid::Uuid>(
                    "INSERT INTO crawler_reviews (review_id, listing_source_id, domain_id, artifact_type, status, reason, candidate_payload, validation_summary, reviewer_notes, reviewed) \
                     VALUES ($1, $2, NULL, 'PRODUCT_SCHEMA', $3, $4, $5, $6, $7, CASE WHEN $3 = 'PENDING_REVIEW' THEN NULL ELSE NOW() END) \
                     RETURNING review_id",
                )
                .bind(requested_review_id.as_uuid())
                .bind(uuid::Uuid::from(*listing_source_id))
                .bind(status)
                .bind(reason)
                .bind(candidate_payload)
                .bind(validation_summary)
                .bind(notes)
                .fetch_one(&mut *transaction)
                .await?,
            )
        };
        let (review_id, is_new) = match inserted {
            Some(review_id) => (crawler_review_id_from_uuid(review_id)?, true),
            None => {
                let review_id = sqlx::query_scalar::<_, uuid::Uuid>(
                    "SELECT review_id FROM crawler_reviews \
                     WHERE listing_source_id = $1 AND artifact_type = 'PRODUCT_SCHEMA' AND status = 'PENDING_REVIEW' \
                     ORDER BY created DESC LIMIT 1 FOR UPDATE",
                )
                .bind(uuid::Uuid::from(*listing_source_id))
                .fetch_one(&mut *transaction)
                .await?;
                (crawler_review_id_from_uuid(review_id)?, false)
            }
        };
        if !is_new {
            attach_waiting_pages(&mut transaction, review_id, &pages).await?;
        } else {
            for page in &pages {
                insert_review_page(
                    &mut transaction,
                    review_id,
                    &page.url,
                    &page.role,
                    &page.raw_html,
                )
                .await?;
            }
        }
        transaction.commit().await?;
        Ok(review_id)
    }

    /// Attaches a page that failed every cached schema to the ListingSource's pending
    /// product-schema review as a waiting page and returns that review, or `None` when no
    /// product-schema review is pending. The page is attached once per URL and only while the
    /// review holds fewer than [`MAX_WAITING_PAGES`] waiting pages; it waits on the review either
    /// way.
    pub async fn join_pending_schema_review(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        raw_html: &str,
    ) -> Result<Option<CrawlerReviewId>, ReviewRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let Some(review_id) = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT review_id FROM crawler_reviews \
             WHERE listing_source_id = $1 AND artifact_type = 'PRODUCT_SCHEMA' AND status = 'PENDING_REVIEW' \
             ORDER BY created DESC LIMIT 1 FOR UPDATE",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Ok(None);
        };
        let review_id = crawler_review_id_from_uuid(review_id)?;
        let page = SchemaReviewPageInput {
            url: url.to_string(),
            role: PAGE_ROLE_WAITING_PAGE.to_owned(),
            raw_html: raw_html.to_owned(),
        };
        attach_waiting_pages(&mut transaction, review_id, std::slice::from_ref(&page)).await?;
        transaction.commit().await?;
        Ok(Some(review_id))
    }

    /// Checks, before the page is fetched, that `url` may be added to the review.
    pub async fn check_reviewer_page_target(
        &self,
        review_id: CrawlerReviewId,
        url: &Url,
    ) -> Result<(), ReviewRepositoryError> {
        let url = reviewer_page_url(url);
        let mut transaction = self.pool.begin().await?;
        lock_reviewer_page_target(&mut transaction, review_id, &url).await?;
        transaction.rollback().await?;
        Ok(())
    }

    /// Stores a page the reviewer chose, fetched from `requested_url` and served from
    /// `fetched_url`, as a `REVIEWER_ADDED` review page and drops the cached schema matrix, which
    /// does not cover the new page yet. A redirect to a page that is not a product page, such as
    /// a sold listing the shop sends to its homepage, is refused.
    pub async fn add_reviewer_page(
        &self,
        review_id: CrawlerReviewId,
        requested_url: &Url,
        fetched_url: &Url,
        raw_html: &str,
    ) -> Result<CrawlerReviewPage, ReviewRepositoryError> {
        let url = reviewer_page_url(fetched_url);
        let mut transaction = self.pool.begin().await?;
        let product_url_pattern =
            lock_reviewer_page_target(&mut transaction, review_id, &url).await?;
        if is_redirect_to_non_product_page(requested_url, &url, product_url_pattern.as_deref()) {
            return Err(ReviewRepositoryError::RedirectedToNonProductPage(
                url.to_string(),
            ));
        }
        let page = sqlx::query(
            "INSERT INTO crawler_review_pages (review_page_id, review_id, url, role, html_hash) \
             VALUES ($1, $2, $3, $4, $5) \
             RETURNING review_page_id, review_id, url, role, html_hash, fetched",
        )
        .bind(CrawlerReviewPageId::new().as_uuid())
        .bind(review_id.as_uuid())
        .bind(url.as_str())
        .bind(PAGE_ROLE_REVIEWER_ADDED)
        .bind(sha256_hex(raw_html.as_bytes()))
        .fetch_one(&mut *transaction)
        .await
        .and_then(row_to_page)?;
        drop_cached_schema_matrix(&mut transaction, review_id).await?;
        transaction.commit().await?;
        info!(
            review_id = %review_id,
            url = %url,
            "Reviewer added a page to a product schema review"
        );
        Ok(page)
    }

    /// Removes a page a reviewer added or a page waiting on the review, for example once the shop
    /// has deleted it and every live matrix refresh would fail on it, and drops the cached schema
    /// matrix. Seed and triggering pages stay.
    pub async fn remove_reviewer_page(
        &self,
        review_page_id: CrawlerReviewPageId,
    ) -> Result<(), ReviewRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let page = sqlx::query(
            "SELECT r.review_id, r.status, p.role \
             FROM crawler_review_pages p JOIN crawler_reviews r ON r.review_id = p.review_id \
             WHERE p.review_page_id = $1 \
             FOR UPDATE OF r",
        )
        .bind(review_page_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(ReviewRepositoryError::ReviewPageNotFound(review_page_id))?;
        let review_id = crawler_review_id_from_uuid(page.try_get("review_id")?)?;
        let role: String = page.try_get("role")?;
        if !matches!(
            role.as_str(),
            PAGE_ROLE_REVIEWER_ADDED | PAGE_ROLE_WAITING_PAGE
        ) {
            return Err(ReviewRepositoryError::NotReviewerAddedPage(review_page_id));
        }
        let status: String = page.try_get("status")?;
        if !matches!(status.as_str(), STATUS_PENDING_REVIEW | STATUS_APPROVED) {
            return Err(ReviewRepositoryError::NotPending(review_id));
        }

        let deleted = sqlx::query("DELETE FROM crawler_review_pages WHERE review_page_id = $1")
            .bind(review_page_id.as_uuid())
            .execute(&mut *transaction)
            .await?;
        if deleted.rows_affected() == 0 {
            return Err(ReviewRepositoryError::ReviewPageNotFound(review_page_id));
        }
        drop_cached_schema_matrix(&mut transaction, review_id).await?;
        transaction.commit().await?;
        info!(
            review_id = %review_id,
            review_page_id = %review_page_id,
            "Reviewer removed a page from a product schema review"
        );
        Ok(())
    }

    pub async fn update_candidate_payload(
        &self,
        review_id: CrawlerReviewId,
        candidate_payload: serde_json::Value,
    ) -> Result<(), ReviewRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let review = sqlx::query(
            "SELECT listing_source_id, artifact_type, status, candidate_payload, validation_summary \
             FROM crawler_reviews WHERE review_id = $1 FOR UPDATE",
        )
        .bind(review_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(ReviewRepositoryError::NotFound(review_id))?;
        let listing_source_id =
            listing_source_id_from_uuid(review.try_get::<uuid::Uuid, _>("listing_source_id")?)?;
        let artifact_type: String = review.try_get("artifact_type")?;
        let status: String = review.try_get("status")?;
        let stored_candidate_payload: serde_json::Value = review.try_get("candidate_payload")?;
        let validation_summary: serde_json::Value = review.try_get("validation_summary")?;

        if status == STATUS_PENDING_REVIEW {
            if artifact_type == ARTIFACT_URL_PATTERN {
                let candidate =
                    serde_json::from_value::<UrlPatternReviewCandidate>(candidate_payload.clone())
                        .map_err(|_| ReviewRepositoryError::InvalidUrlPatternCandidate)?;
                candidate
                    .validated_pattern()
                    .map_err(|_| ReviewRepositoryError::InvalidUrlPatternCandidate)?;
            }
            if artifact_type == ARTIFACT_PRODUCT_SCHEMA {
                parse_schemas_payload(&candidate_payload)?;
            }
            sqlx::query(
                "UPDATE crawler_reviews SET candidate_payload = $2, updated = NOW() \
                 WHERE review_id = $1",
            )
            .bind(review_id.as_uuid())
            .bind(candidate_payload)
            .execute(&mut *transaction)
            .await?;
            transaction.commit().await?;
            return Ok(());
        }

        if status != STATUS_APPROVED || artifact_type != ARTIFACT_PRODUCT_SCHEMA {
            return Err(ReviewRepositoryError::NotPending(review_id));
        }

        let schemas = parse_schemas_payload(&candidate_payload)?;
        persist_approved_schema_edit(
            &mut transaction,
            review_id,
            &listing_source_id,
            &stored_candidate_payload,
            &schemas,
            candidate_payload,
            validation_summary,
        )
        .await?;
        transaction.commit().await?;
        log_approved_schema_edit(review_id, &listing_source_id, schemas.len());
        Ok(())
    }

    pub async fn update_schema_field(
        &self,
        review_id: CrawlerReviewId,
        schema_index: usize,
        field: &str,
        rule: Option<ExtractionRule>,
    ) -> Result<(), ReviewRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let review = sqlx::query(
            "SELECT listing_source_id, artifact_type, status, reason, candidate_payload, validation_summary \
             FROM crawler_reviews WHERE review_id = $1 FOR UPDATE",
        )
        .bind(review_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(ReviewRepositoryError::NotFound(review_id))?;
        let listing_source_id =
            listing_source_id_from_uuid(review.try_get::<uuid::Uuid, _>("listing_source_id")?)?;
        let artifact_type: String = review.try_get("artifact_type")?;
        let status: String = review.try_get("status")?;
        let reason: String = review.try_get("reason")?;
        let stored_candidate_payload: serde_json::Value = review.try_get("candidate_payload")?;
        let validation_summary: serde_json::Value = review.try_get("validation_summary")?;

        if artifact_type != ARTIFACT_PRODUCT_SCHEMA {
            return Err(ReviewRepositoryError::UnsupportedArtifact(
                review_id,
                artifact_type,
            ));
        }
        if !matches!(status.as_str(), STATUS_PENDING_REVIEW | STATUS_APPROVED) {
            return Err(ReviewRepositoryError::NotPending(review_id));
        }

        let candidate_payload = if status == STATUS_PENDING_REVIEW
            && matches!(
                reason.as_str(),
                "append_schema_generation" | "normalization_schema_repair"
            ) {
            let reviewed_schemas = parse_schemas_payload(&stored_candidate_payload)?;
            if reviewed_schemas.len() == 1 {
                let existing =
                    lock_live_product_schemas(&mut transaction, &listing_source_id).await?;
                json!({
                    "schemas": approval_product_schemas(
                        &reason,
                        existing.as_ref(),
                        reviewed_schemas,
                    )?
                })
            } else {
                stored_candidate_payload.clone()
            }
        } else {
            stored_candidate_payload.clone()
        };
        let schemas = update_schema_field_payload(&candidate_payload, schema_index, field, rule)?;
        let candidate_payload = json!({ "schemas": schemas });

        if status == STATUS_APPROVED {
            persist_approved_schema_edit(
                &mut transaction,
                review_id,
                &listing_source_id,
                &stored_candidate_payload,
                &schemas,
                candidate_payload,
                validation_summary,
            )
            .await?;
        } else {
            sqlx::query(
                "UPDATE crawler_reviews SET candidate_payload = $2, updated = NOW() \
                 WHERE review_id = $1",
            )
            .bind(review_id.as_uuid())
            .bind(candidate_payload)
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;

        if status == STATUS_APPROVED {
            log_approved_schema_edit(review_id, &listing_source_id, schemas.len());
        }
        Ok(())
    }

    pub async fn approve_review(
        &self,
        review_id: CrawlerReviewId,
        notes: Option<&str>,
    ) -> Result<(), ReviewRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let review = sqlx::query(
            "SELECT listing_source_id, domain_id, artifact_type, reason, candidate_payload \
             FROM crawler_reviews \
             WHERE review_id = $1 AND status = 'PENDING_REVIEW' \
             FOR UPDATE",
        )
        .bind(review_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(ReviewRepositoryError::NotPending(review_id))?;
        let listing_source_id =
            listing_source_id_from_uuid(review.try_get::<uuid::Uuid, _>("listing_source_id")?)?;
        let domain_id = review
            .try_get::<Option<uuid::Uuid>, _>("domain_id")?
            .map(crawler_domain_id_from_uuid)
            .transpose()?;
        let artifact_type: String = review.try_get("artifact_type")?;
        let reason: String = review.try_get("reason")?;
        let candidate_payload: serde_json::Value = review.try_get("candidate_payload")?;

        match artifact_type.as_str() {
            ARTIFACT_URL_PATTERN => {
                let candidate =
                    serde_json::from_value::<UrlPatternReviewCandidate>(candidate_payload)
                        .map_err(|_| ReviewRepositoryError::InvalidUrlPatternCandidate)?;
                let pattern = candidate
                    .validated_pattern()
                    .map_err(|_| ReviewRepositoryError::InvalidUrlPatternCandidate)?;
                let domain_id = domain_id.ok_or(sqlx::Error::RowNotFound)?;
                let result = sqlx::query(
                    "UPDATE listing_source_domains \
                     SET url_pattern = $3, \
                         url_pattern_state = CASE WHEN $3 IS NULL THEN 'NO_PATTERN' ELSE 'MATCHED' END \
                     WHERE listing_source_id = $1 AND domain_id = $2",
                )
                .bind(uuid::Uuid::from(listing_source_id))
                .bind(domain_id.as_uuid())
                .bind(pattern)
                .execute(&mut *transaction)
                .await?;
                if result.rows_affected() == 0 {
                    return Err(ReviewRepositoryError::NotFound(review_id));
                }
            }
            ARTIFACT_PRODUCT_SCHEMA => {
                let reviewed_schemas = parse_schemas_payload(&candidate_payload)?;
                let existing =
                    lock_live_product_schemas(&mut transaction, &listing_source_id).await?;
                let schemas =
                    approval_product_schemas(&reason, existing.as_ref(), reviewed_schemas.clone())?;
                persist_product_schemas(&mut transaction, &listing_source_id, &schemas).await?;
                if schemas != reviewed_schemas {
                    // The review keeps the merged list that went live, so edits through it are
                    // not refused as stale.
                    sqlx::query(
                        "UPDATE crawler_reviews SET candidate_payload = $2 WHERE review_id = $1",
                    )
                    .bind(review_id.as_uuid())
                    .bind(json!({ "schemas": schemas }))
                    .execute(&mut *transaction)
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
        .bind(&artifact_type)
        .bind(review_id.as_uuid())
        .bind(domain_id.map(CrawlerDomainId::into_uuid))
        .execute(&mut *transaction)
        .await?;
        match artifact_type.as_str() {
            ARTIFACT_PRODUCT_SCHEMA => {
                sqlx::query(
                    "UPDATE listing_source_urls \
                     SET next_retry_at = NULL, last_error_kind = NULL, \
                         last_error_message = NULL, updated = NOW() \
                     WHERE listing_source_id = $1 \
                       AND last_error_kind = 'PendingSchemaReview' \
                       AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')",
                )
                .bind(uuid::Uuid::from(listing_source_id))
                .execute(&mut *transaction)
                .await?;
            }
            ARTIFACT_URL_PATTERN => {
                sqlx::query(
                    "UPDATE listing_source_domains \
                     SET next_crawl_at = NULL, last_crawl_error_kind = NULL \
                     WHERE listing_source_id = $1 AND domain_id = $2 \
                       AND last_crawl_error_kind = 'PendingUrlPatternReview'",
                )
                .bind(uuid::Uuid::from(listing_source_id))
                .bind(domain_id.map(CrawlerDomainId::into_uuid))
                .execute(&mut *transaction)
                .await?;
            }
            _ => {}
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn reject_review(
        &self,
        review_id: CrawlerReviewId,
        notes: Option<&str>,
    ) -> Result<(), ReviewRepositoryError> {
        self.set_review_status(review_id, STATUS_REJECTED, notes)
            .await
    }

    pub async fn request_repair(
        &self,
        review_id: CrawlerReviewId,
        notes: Option<&str>,
    ) -> Result<(), ReviewRepositoryError> {
        self.set_review_status(review_id, STATUS_NEEDS_REPAIR, notes)
            .await
    }

    pub async fn evaluate_schema_matrix_for_live_pages(
        &self,
        review_id: CrawlerReviewId,
        pages: Vec<(CrawlerReviewPage, String)>,
    ) -> Result<EvaluatedSchemaMatrix, ReviewRepositoryError> {
        let review = sqlx::query(
            "SELECT candidate_payload, candidate_version,
                    encode(digest(candidate_payload::text, 'sha256'), 'hex') AS candidate_hash
             FROM crawler_reviews
             WHERE review_id = $1",
        )
        .bind(review_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(ReviewRepositoryError::NotFound(review_id))?;
        let candidate_payload: serde_json::Value = review.try_get("candidate_payload")?;
        let schemas = parse_schemas_payload(&candidate_payload)?;
        let mut page_ids: Vec<uuid::Uuid> = pages
            .iter()
            .map(|(page, _)| *page.review_page_id.as_uuid())
            .collect();
        page_ids.sort_unstable();
        Ok(EvaluatedSchemaMatrix {
            matrix: evaluate_schema_matrix_for_live_review_pages(review_id, &schemas, &pages),
            candidate_version: review.try_get("candidate_version")?,
            candidate_hash: review.try_get("candidate_hash")?,
            page_ids,
        })
    }

    pub async fn store_schema_matrix_if_current(
        &self,
        review_id: CrawlerReviewId,
        evaluated: &EvaluatedSchemaMatrix,
    ) -> Result<(), ReviewRepositoryError> {
        let matrix = serde_json::to_value(&evaluated.matrix)?;
        let result = sqlx::query(
            "UPDATE crawler_reviews
             SET validation_summary = jsonb_set(
                     CASE jsonb_typeof(validation_summary)
                         WHEN 'object' THEN validation_summary
                         ELSE jsonb_build_object('summary', validation_summary)
                     END,
                     '{schema_matrix}',
                     $2::jsonb,
                     true
                 ),
                 updated = NOW()
             WHERE review_id = $1
               AND candidate_version = $3
               AND encode(digest(candidate_payload::text, 'sha256'), 'hex') = $4
               AND $5::uuid[] = ARRAY(
                   SELECT review_page_id FROM crawler_review_pages
                   WHERE review_id = $1 ORDER BY review_page_id
               )",
        )
        .bind(review_id.as_uuid())
        .bind(matrix)
        .bind(evaluated.candidate_version)
        .bind(&evaluated.candidate_hash)
        .bind(&evaluated.page_ids)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(ReviewRepositoryError::CandidateChangedDuringEvaluation);
        }
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

    async fn set_review_status(
        &self,
        review_id: CrawlerReviewId,
        status: &str,
        notes: Option<&str>,
    ) -> Result<(), ReviewRepositoryError> {
        let result = sqlx::query(
            "UPDATE crawler_reviews
             SET status = $2, reviewer_notes = COALESCE($3, reviewer_notes),
                 reviewed = NOW(), updated = NOW()
             WHERE review_id = $1 AND status = 'PENDING_REVIEW'",
        )
        .bind(review_id.as_uuid())
        .bind(status)
        .bind(notes)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(ReviewRepositoryError::NotPending(review_id));
        }
        Ok(())
    }
}

/// Locks and reads the ListingSource's live product schemas for the rest of the transaction.
async fn lock_live_product_schemas(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    listing_source_id: &ListingSourceId,
) -> Result<Option<ListingSourceProductSchema>, ReviewRepositoryError> {
    let existing_payload = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT product_schema FROM listing_source_product_schemas \
         WHERE listing_source_id = $1 FOR UPDATE",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .fetch_optional(&mut **transaction)
    .await?;
    existing_payload
        .map(|payload| {
            parse_product_schema_payload(payload).map(|product_schemas| {
                ListingSourceProductSchema {
                    listing_source_id: *listing_source_id,
                    product_schemas,
                    created: OffsetDateTime::UNIX_EPOCH,
                    updated: OffsetDateTime::UNIX_EPOCH,
                }
            })
        })
        .transpose()
}

/// Writes an edit of an approved review to the live schemas and records it on the review.
///
/// The edit replaces the whole live list, so it is refused unless the review's stored
/// candidate is the live list: an edit through an older review would otherwise silently
/// revert every schema change approved since.
async fn persist_approved_schema_edit(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    listing_source_id: &ListingSourceId,
    stored_candidate_payload: &serde_json::Value,
    schemas: &[ProductCssSelectorSchema],
    candidate_payload: serde_json::Value,
    validation_summary: serde_json::Value,
) -> Result<(), ReviewRepositoryError> {
    let stored_schemas = decode_stored_schemas_payload(stored_candidate_payload);
    let live = lock_live_product_schemas(transaction, listing_source_id).await?;
    if live.is_none_or(|live| stored_schemas.as_ref() != Some(&live.product_schemas)) {
        return Err(ReviewRepositoryError::StaleApprovedReview(review_id));
    }
    persist_product_schemas(transaction, listing_source_id, schemas).await?;
    let validation_summary = append_manual_schema_edit(validation_summary, schemas.len());
    sqlx::query(
        "UPDATE crawler_reviews \
         SET candidate_payload = $2, validation_summary = $3, updated = NOW() \
         WHERE review_id = $1",
    )
    .bind(review_id.as_uuid())
    .bind(candidate_payload)
    .bind(validation_summary)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Attaches `pages` to the pending review `review_id` as waiting pages, whatever role they were
/// collected under: once per page under the reviewer-page identity, up to [`MAX_WAITING_PAGES`].
/// Drops the cached schema matrix when a page was attached, so the next refresh covers it. The
/// caller holds the review row lock.
async fn attach_waiting_pages(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    pages: &[SchemaReviewPageInput],
) -> Result<(), sqlx::Error> {
    let stored = sqlx::query_as::<_, (String, String)>(
        "SELECT url, role FROM crawler_review_pages WHERE review_id = $1",
    )
    .bind(review_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await?;
    let mut waiting_pages = stored
        .iter()
        .filter(|(_, role)| role == PAGE_ROLE_WAITING_PAGE)
        .count();
    let mut identities = stored
        .iter()
        .filter_map(|(url, _)| ReviewPageIdentity::of_stored(url))
        .collect::<Vec<_>>();
    let mut attached = 0_usize;
    for page in pages {
        if waiting_pages >= MAX_WAITING_PAGES {
            break;
        }
        let Some(identity) = ReviewPageIdentity::of_stored(&page.url) else {
            continue;
        };
        if identities.contains(&identity) {
            continue;
        }
        insert_review_page(
            transaction,
            review_id,
            &page.url,
            PAGE_ROLE_WAITING_PAGE,
            &page.raw_html,
        )
        .await?;
        identities.push(identity);
        waiting_pages += 1;
        attached += 1;
    }
    if attached > 0 {
        drop_cached_schema_matrix(transaction, review_id).await?;
        info!(
            review_id = %review_id,
            attached_pages = attached,
            waiting_pages,
            "Attached pages waiting on a pending product schema review"
        );
    }
    Ok(())
}

async fn insert_review_page(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    url: &str,
    role: &str,
    raw_html: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO crawler_review_pages (review_page_id, review_id, url, role, html_hash) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(CrawlerReviewPageId::new().as_uuid())
    .bind(review_id.as_uuid())
    .bind(url)
    .bind(role)
    .bind(sha256_hex(raw_html.as_bytes()))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Drops the cached schema matrix after the review's page set changed.
async fn drop_cached_schema_matrix(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE crawler_reviews \
         SET validation_summary = validation_summary - 'schema_matrix', updated = NOW() \
         WHERE review_id = $1 AND jsonb_typeof(validation_summary) = 'object'",
    )
    .bind(review_id.as_uuid())
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Locks the review and checks that `url` may become a reviewer page of it: the review is a
/// pending or approved product schema review, `url` is an http(s) URL on a domain of the
/// review's ListingSource, it is not a page of the review yet, and the reviewer page limit is not
/// reached. Returns the product URL pattern of the domain `url` is on, once one is decided.
async fn lock_reviewer_page_target(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    url: &Url,
) -> Result<Option<String>, ReviewRepositoryError> {
    let review = sqlx::query(
        "SELECT listing_source_id, artifact_type, status \
         FROM crawler_reviews WHERE review_id = $1 FOR UPDATE",
    )
    .bind(review_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(ReviewRepositoryError::NotFound(review_id))?;
    let artifact_type: String = review.try_get("artifact_type")?;
    if artifact_type != ARTIFACT_PRODUCT_SCHEMA {
        return Err(ReviewRepositoryError::UnsupportedArtifact(
            review_id,
            artifact_type,
        ));
    }
    let status: String = review.try_get("status")?;
    if !matches!(status.as_str(), STATUS_PENDING_REVIEW | STATUS_APPROVED) {
        return Err(ReviewRepositoryError::NotPending(review_id));
    }

    let domains = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT listing_source_domain, \
                CASE WHEN url_pattern_state = 'MATCHED' THEN url_pattern END \
         FROM listing_source_domains WHERE listing_source_id = $1",
    )
    .bind(review.try_get::<uuid::Uuid, _>("listing_source_id")?)
    .fetch_all(&mut **transaction)
    .await?;
    let (domain_names, product_url_patterns): (Vec<String>, Vec<Option<String>>) =
        domains.into_iter().unzip();
    if !is_allowed_reviewer_page_url(url, &domain_names) {
        return Err(ReviewRepositoryError::ReviewPageUrlNotAllowed(
            without_credentials(url).to_string(),
        ));
    }

    let pages = sqlx::query_as::<_, (String, String)>(
        "SELECT url, role FROM crawler_review_pages WHERE review_id = $1",
    )
    .bind(review_id.as_uuid())
    .fetch_all(&mut **transaction)
    .await?;
    let identity = ReviewPageIdentity::of(url);
    if pages
        .iter()
        .any(|(page_url, _)| ReviewPageIdentity::of_stored(page_url).as_ref() == Some(&identity))
    {
        return Err(ReviewRepositoryError::DuplicateReviewPage(url.to_string()));
    }
    let reviewer_pages = pages
        .iter()
        .filter(|(_, role)| role == PAGE_ROLE_REVIEWER_ADDED)
        .count();
    if reviewer_pages >= MAX_REVIEWER_ADDED_PAGES {
        return Err(ReviewRepositoryError::ReviewPageLimitReached(review_id));
    }
    Ok(domain_names
        .iter()
        .zip(product_url_patterns)
        .find(|(domain, _)| url_matches_configured_domain(url, domain))
        .and_then(|(_, pattern)| pattern))
}

fn log_approved_schema_edit(
    review_id: CrawlerReviewId,
    listing_source_id: &ListingSourceId,
    schema_count: usize,
) {
    info!(
        review_id = %review_id,
        listing_source_id = %listing_source_id,
        schema_count,
        "Approved product schema edited from review console"
    );
}

async fn persist_product_schemas(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    listing_source_id: &ListingSourceId,
    schemas: &[ProductCssSelectorSchema],
) -> Result<(), ReviewRepositoryError> {
    let product_schema = serde_json::to_value(schemas)?;
    sqlx::query(
        "INSERT INTO listing_source_product_schemas (listing_source_id, product_schema, created, updated) \
         VALUES ($1, $2, NOW(), NOW()) \
         ON CONFLICT (listing_source_id) DO UPDATE \
         SET product_schema = EXCLUDED.product_schema, updated = NOW()",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(product_schema)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn parse_product_schema_payload(
    payload: serde_json::Value,
) -> Result<Vec<ProductCssSelectorSchema>, ReviewRepositoryError> {
    match serde_json::from_value::<Vec<ProductCssSelectorSchema>>(payload.clone()) {
        Ok(schemas) => Ok(schemas),
        Err(_) => Ok(vec![serde_json::from_value(payload)?]),
    }
}

fn row_to_review(row: sqlx::postgres::PgRow) -> Result<CrawlerReview, sqlx::Error> {
    Ok(CrawlerReview {
        review_id: crawler_review_id_from_uuid(row.try_get("review_id")?)?,
        listing_source_id: listing_source_id_from_uuid(
            row.try_get::<uuid::Uuid, _>("listing_source_id")?,
        )?,
        listing_source_name: row.try_get("listing_source_name")?,
        domain_id: row
            .try_get::<Option<uuid::Uuid>, _>("domain_id")?
            .map(crawler_domain_id_from_uuid)
            .transpose()?,
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
        review_page_id: crawler_review_page_id_from_uuid(row.try_get("review_page_id")?)?,
        review_id: crawler_review_id_from_uuid(row.try_get("review_id")?)?,
        url: row.try_get("url")?,
        role: row.try_get("role")?,
        html_hash: row.try_get("html_hash")?,
        fetched: row.try_get("fetched")?,
    })
}

fn row_to_review_url(row: sqlx::postgres::PgRow) -> Result<CrawlerReviewUrl, sqlx::Error> {
    Ok(CrawlerReviewUrl {
        review_url_id: crawler_review_url_id_from_uuid(row.try_get("review_url_id")?)?,
        review_id: crawler_review_id_from_uuid(row.try_get("review_id")?)?,
        url: row.try_get("url")?,
        previous_class: row.try_get("previous_class")?,
        current_pattern_match: row.try_get("current_pattern_match")?,
        candidate_pattern_match: row.try_get("candidate_pattern_match")?,
        candidate_class: row.try_get("candidate_class")?,
    })
}

#[derive(Debug, thiserror::Error)]
#[error("invalid persisted {object_type} UUID `{value}`")]
struct PersistedObjectIdError {
    object_type: &'static str,
    value: uuid::Uuid,
    #[source]
    source: domain_primitives::object_id::ObjectIdError,
}

fn persisted_object_id_error(
    object_type: &'static str,
    value: uuid::Uuid,
    source: domain_primitives::object_id::ObjectIdError,
) -> sqlx::Error {
    sqlx::Error::Decode(Box::new(PersistedObjectIdError {
        object_type,
        value,
        source,
    }))
}

fn listing_source_id_from_uuid(value: uuid::Uuid) -> Result<ListingSourceId, sqlx::Error> {
    ListingSourceId::try_from(value)
        .map_err(|source| persisted_object_id_error("ListingSourceId", value, source))
}

fn crawler_domain_id_from_uuid(value: uuid::Uuid) -> Result<CrawlerDomainId, sqlx::Error> {
    CrawlerDomainId::try_from(value)
        .map_err(|source| persisted_object_id_error("CrawlerDomainId", value, source))
}

fn crawler_review_id_from_uuid(value: uuid::Uuid) -> Result<CrawlerReviewId, sqlx::Error> {
    CrawlerReviewId::try_from(value)
        .map_err(|source| persisted_object_id_error("CrawlerReviewId", value, source))
}

fn crawler_review_page_id_from_uuid(value: uuid::Uuid) -> Result<CrawlerReviewPageId, sqlx::Error> {
    CrawlerReviewPageId::try_from(value)
        .map_err(|source| persisted_object_id_error("CrawlerReviewPageId", value, source))
}

fn crawler_review_url_id_from_uuid(value: uuid::Uuid) -> Result<CrawlerReviewUrlId, sqlx::Error> {
    CrawlerReviewUrlId::try_from(value)
        .map_err(|source| persisted_object_id_error("CrawlerReviewUrlId", value, source))
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
