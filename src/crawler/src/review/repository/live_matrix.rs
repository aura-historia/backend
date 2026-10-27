//! The live schema matrix of a product-schema review: the review's candidate evaluated on its
//! pages as the shop serves them now, and stored as the review's cached matrix while it still
//! describes the review.

use super::reviewer_page_url::ReviewPageIdentity;
use super::schema_payload::ProductSchemaReviewCandidate;
use super::{CrawlerReviewRepository, ReviewRepositoryError};
use crate::CrawlerReviewId;
use crate::review::model::{CrawlerReviewPage, SchemaMatrix};
use crate::review::schema_evaluation::evaluate_schema_matrix_for_live_review_pages;
use sqlx::Row;
use std::collections::HashMap;
use url::Url;

pub struct EvaluatedSchemaMatrix {
    pub matrix: SchemaMatrix,
    candidate_version: i64,
    candidate_hash: String,
    /// The review pages the matrix evaluated.
    page_ids: Vec<uuid::Uuid>,
}

impl CrawlerReviewRepository {
    pub async fn evaluate_schema_matrix_for_live_pages(
        &self,
        review_id: CrawlerReviewId,
        pages: Vec<(CrawlerReviewPage, String)>,
    ) -> Result<EvaluatedSchemaMatrix, ReviewRepositoryError> {
        let review = sqlx::query(
            "SELECT listing_source_id, candidate_payload, candidate_version,
                    encode(digest(candidate_payload::text, 'sha256'), 'hex') AS candidate_hash
             FROM crawler_reviews
             WHERE review_id = $1",
        )
        .bind(review_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(ReviewRepositoryError::NotFound(review_id))?;
        let candidate_payload: serde_json::Value = review.try_get("candidate_payload")?;
        let candidate = ProductSchemaReviewCandidate::read(&candidate_payload)?;
        let captured_identities = self
            .captured_identities(review.try_get("listing_source_id")?, &pages)
            .await?;
        let page_ids = pages
            .iter()
            .map(|(page, _)| *page.review_page_id.as_uuid())
            .collect();
        Ok(EvaluatedSchemaMatrix {
            matrix: evaluate_schema_matrix_for_live_review_pages(
                review_id,
                &candidate.schemas,
                &candidate.removed_page_schemas,
                &pages,
                &captured_identities,
            ),
            candidate_version: review.try_get("candidate_version")?,
            candidate_hash: review.try_get("candidate_hash")?,
            page_ids,
        })
    }

    /// The source listing id each review page's listing was captured under, by page URL,
    /// for the pages whose listing has been captured. A page is matched with the crawler URL
    /// that names it, however the two spell it: a reviewer page is stored as fetched, often
    /// with the trailing slash the crawler URL's normalization drops. The exact spelling
    /// wins over another one.
    async fn captured_identities(
        &self,
        listing_source_id: uuid::Uuid,
        pages: &[(CrawlerReviewPage, String)],
    ) -> Result<HashMap<String, String>, ReviewRepositoryError> {
        let pages: Vec<(&str, Option<ReviewPageIdentity>)> = pages
            .iter()
            .map(|(page, _)| (page.url.as_str(), ReviewPageIdentity::of_stored(&page.url)))
            .collect();
        let spellings: Vec<String> = pages
            .iter()
            .flat_map(|(url, _)| {
                let spellings = Url::parse(url)
                    .map(|url| ReviewPageIdentity::spellings(&url))
                    .unwrap_or_default();
                std::iter::once((*url).to_owned()).chain(spellings)
            })
            .collect();
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT url, last_source_listing_id
             FROM listing_source_urls
             WHERE listing_source_id = $1
               AND url = ANY($2)
               AND last_source_listing_id IS NOT NULL
             ORDER BY url",
        )
        .bind(listing_source_id)
        .bind(&spellings)
        .fetch_all(&self.pool)
        .await?;
        Ok(pages
            .into_iter()
            .filter_map(|(page_url, identity)| {
                rows.iter()
                    .find(|(url, _)| url == page_url)
                    .or_else(|| {
                        rows.iter().find(|(url, _)| {
                            identity.is_some() && ReviewPageIdentity::of_stored(url) == identity
                        })
                    })
                    .map(|(_, captured)| (page_url.to_owned(), captured.clone()))
            })
            .collect())
    }

    /// Stores the matrix as the review's cached matrix while the review still holds the
    /// candidate it evaluated and every page it evaluated. Pages that joined the review while it
    /// was evaluated are not in it until the next evaluation: they must not keep a refresh from
    /// ever storing its matrix while waiting pages keep joining.
    pub async fn store_schema_matrix_if_current(
        &self,
        review_id: CrawlerReviewId,
        evaluated: &EvaluatedSchemaMatrix,
    ) -> Result<(), ReviewRepositoryError> {
        let matrix = serde_json::to_value(&evaluated.matrix)?;
        let mut transaction = self.pool.begin().await?;
        // The row lock waits for page joins and edits in flight, which lock the review first.
        let (candidate_version, candidate_hash) = sqlx::query_as::<_, (i64, String)>(
            "SELECT candidate_version, encode(digest(candidate_payload::text, 'sha256'), 'hex') \
             FROM crawler_reviews WHERE review_id = $1 FOR UPDATE",
        )
        .bind(review_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(ReviewRepositoryError::NotFound(review_id))?;
        let page_ids = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT review_page_id FROM crawler_review_pages WHERE review_id = $1",
        )
        .bind(review_id.as_uuid())
        .fetch_all(&mut *transaction)
        .await?;
        if candidate_version != evaluated.candidate_version
            || candidate_hash != evaluated.candidate_hash
            || !evaluated.page_ids.iter().all(|id| page_ids.contains(id))
        {
            return Err(ReviewRepositoryError::CandidateChangedDuringEvaluation {
                candidate_version,
            });
        }
        // `schema_matrix_summary` judges the matrix schema generation stored, so it leaves with
        // that matrix instead of standing next to a refreshed one.
        sqlx::query(
            "UPDATE crawler_reviews
             SET validation_summary = jsonb_set(
                     CASE jsonb_typeof(validation_summary)
                         WHEN 'object' THEN validation_summary - 'schema_matrix_summary'
                         ELSE jsonb_build_object('summary', validation_summary)
                     END,
                     '{schema_matrix}',
                     $2::jsonb,
                     true
                 ),
                 updated = NOW()
             WHERE review_id = $1",
        )
        .bind(review_id.as_uuid())
        .bind(matrix)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }
}
