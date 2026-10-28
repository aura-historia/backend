//! The live schema matrix of a product-schema review: what approving the review's candidate
//! would make live, evaluated on the review's pages as the shop serves them now, and stored as
//! the review's cached matrix while it still describes the review. A candidate proposed for the
//! review is evaluated the same way, on the review's pages and on further URLs of its
//! ListingSource, without being stored.

use super::review_source::{MAX_EVALUATED_URLS, listing_source_domains, review_source_url};
use super::reviewer_page_url::ReviewPageIdentity;
use super::schema_payload::{ProductSchemaCandidateEdit, ProductSchemaReviewCandidate};
use super::{
    CrawlerReviewRepository, ReviewRepositoryError, fetch_review_pages, listing_source_id_from_uuid,
};
use crate::CrawlerReviewId;
use crate::review::model::{
    ARTIFACT_PRODUCT_SCHEMA, CrawlerReviewPage, LiveMatrixEvaluation, ReviewSchemaMatrix,
    SchemaMatrix, SchemaReviewPageInput,
};
use crate::review::schema_evaluation::evaluate_schema_matrix_for_live_review_pages;
use crate::scraper::css_selector::product_schema_repository::{
    ListingSourceProductSchemaRepository, ListingSourceProductSchemaRepositoryImpl,
};
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crate::scraper::css_selector::removed_page_schema_repository::{
    RemovedPageSchemaRepository, RemovedPageSchemaRepositoryImpl,
};
use sqlx::Row;
use std::collections::HashMap;
use time::OffsetDateTime;
use url::Url;

pub struct EvaluatedSchemaMatrix {
    pub matrix: SchemaMatrix,
    candidate_hash: String,
    /// The review pages the matrix evaluated.
    page_ids: Vec<uuid::Uuid>,
}

impl EvaluatedSchemaMatrix {
    fn candidate_version(&self) -> Option<i64> {
        self.matrix
            .live_evaluation
            .as_ref()
            .map(|evaluation| evaluation.candidate_version)
    }
}

/// A product-schema candidate proposed for a review and not stored, ready to be evaluated on
/// the review's pages and on URLs of its ListingSource: what approving it, once stored, would make
/// live.
pub struct CandidateProposal {
    review_id: CrawlerReviewId,
    listing_source_id: uuid::Uuid,
    going_live: ProductSchemaReviewCandidate,
    /// The review's pages, to fetch live.
    pub pages: Vec<CrawlerReviewPage>,
    /// The URLs of the ListingSource to fetch live and evaluate next to the review's pages.
    pub urls: Vec<Url>,
}

impl CandidateProposal {
    pub fn review_id(&self) -> CrawlerReviewId {
        self.review_id
    }
}

/// A product-schema review as a live evaluation reads it.
struct ReviewUnderEvaluation {
    listing_source_id: uuid::Uuid,
    artifact_type: String,
    reason: String,
    candidate_payload: serde_json::Value,
    candidate_version: i64,
    candidate_hash: String,
    removed_page_schemas_base: serde_json::Value,
}

impl CrawlerReviewRepository {
    /// Evaluates, on the review `pages` fetched from `fetched_at` on, the product-schema
    /// configuration approving the review's current candidate would make live: its schemas,
    /// merged with the live list for a legacy single-schema append or repair, and its
    /// removed-page templates rebased onto the stored ones. The matrix records what it evaluated
    /// and when, so an approval can tell whether it still describes the review.
    pub async fn evaluate_schema_matrix_for_live_pages(
        &self,
        review_id: CrawlerReviewId,
        pages: Vec<(CrawlerReviewPage, String)>,
        fetched_at: OffsetDateTime,
    ) -> Result<EvaluatedSchemaMatrix, ReviewRepositoryError> {
        let review = self.review_under_evaluation(review_id).await?;
        let candidate = ProductSchemaReviewCandidate::read(&review.candidate_payload)?;
        let going_live = self.going_live(&review, &candidate).await?;
        let page_urls: Vec<&str> = pages.iter().map(|(page, _)| page.url.as_str()).collect();
        let captured_identities = self
            .captured_identities(review.listing_source_id, &page_urls)
            .await?;
        let page_ids = pages
            .iter()
            .map(|(page, _)| *page.review_page_id.as_uuid())
            .collect();
        let mut matrix = evaluate_schema_matrix_for_live_review_pages(
            review_id,
            &going_live.schemas,
            &going_live.removed_page_schemas,
            &pages,
            &[],
            &captured_identities,
        );
        matrix.live_evaluation = Some(LiveMatrixEvaluation {
            candidate_version: review.candidate_version,
            configuration_sha256: going_live.fingerprint()?,
            fetched_at,
        });
        Ok(EvaluatedSchemaMatrix {
            matrix,
            candidate_hash: review.candidate_hash,
            page_ids,
        })
    }

    /// Checks a product-schema candidate `proposed` for the review, in the shape of a whole
    /// candidate edit, and the ListingSource `urls` to evaluate it on, before anything is fetched:
    /// the proposal must be one the review would take as an edit, and every URL a plain http(s)
    /// URL of the review's ListingSource, at most [`MAX_EVALUATED_URLS`] of them. Nothing is
    /// stored, so the review's candidate, version and stored matrix stay as they are.
    pub async fn propose_candidate(
        &self,
        review_id: CrawlerReviewId,
        proposed: &serde_json::Value,
        urls: &[String],
    ) -> Result<CandidateProposal, ReviewRepositoryError> {
        let review = self.review_under_evaluation(review_id).await?;
        if review.artifact_type != ARTIFACT_PRODUCT_SCHEMA {
            return Err(ReviewRepositoryError::UnsupportedArtifact(
                review_id,
                review.artifact_type,
            ));
        }
        if urls.len() > MAX_EVALUATED_URLS {
            return Err(ReviewRepositoryError::TooManyEvaluatedUrls);
        }
        let current = ProductSchemaReviewCandidate::decode_stored(&review.candidate_payload)
            .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
        let candidate = ProductSchemaCandidateEdit::parse(proposed)?.apply_to(&current)?;
        let going_live = self.going_live(&review, &candidate).await?;
        let domains = listing_source_domains(&self.pool, review.listing_source_id).await?;
        let urls = urls
            .iter()
            .map(|url| review_source_url(url, &domains))
            .collect::<Result<Vec<_>, _>>()?;
        let pages = fetch_review_pages(&self.pool, review_id).await?;
        Ok(CandidateProposal {
            review_id,
            listing_source_id: review.listing_source_id,
            going_live,
            pages,
            urls,
        })
    }

    /// Evaluates the `proposal` on its review `pages` and its `url_pages` as the shop serves them
    /// now. The matrix is no live evaluation of the review: it records none and is not stored, so
    /// it never stands in for the review's own live matrix.
    pub async fn evaluate_candidate_proposal(
        &self,
        proposal: CandidateProposal,
        pages: Vec<(CrawlerReviewPage, String)>,
        url_pages: Vec<SchemaReviewPageInput>,
    ) -> Result<SchemaMatrix, ReviewRepositoryError> {
        let page_urls: Vec<&str> = pages
            .iter()
            .map(|(page, _)| page.url.as_str())
            .chain(url_pages.iter().map(|page| page.url.as_str()))
            .collect();
        let captured_identities = self
            .captured_identities(proposal.listing_source_id, &page_urls)
            .await?;
        Ok(evaluate_schema_matrix_for_live_review_pages(
            proposal.review_id,
            &proposal.going_live.schemas,
            &proposal.going_live.removed_page_schemas,
            &pages,
            &url_pages,
            &captured_identities,
        ))
    }

    async fn review_under_evaluation(
        &self,
        review_id: CrawlerReviewId,
    ) -> Result<ReviewUnderEvaluation, ReviewRepositoryError> {
        let review = sqlx::query(
            "SELECT listing_source_id, artifact_type, reason, candidate_payload, candidate_version,
                    removed_page_schemas_base,
                    encode(digest(candidate_payload::text, 'sha256'), 'hex') AS candidate_hash
             FROM crawler_reviews
             WHERE review_id = $1",
        )
        .bind(review_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(ReviewRepositoryError::NotFound(review_id))?;
        Ok(ReviewUnderEvaluation {
            listing_source_id: review.try_get("listing_source_id")?,
            artifact_type: review.try_get("artifact_type")?,
            reason: review.try_get("reason")?,
            candidate_payload: review.try_get("candidate_payload")?,
            candidate_version: review.try_get("candidate_version")?,
            candidate_hash: review.try_get("candidate_hash")?,
            removed_page_schemas_base: review.try_get("removed_page_schemas_base")?,
        })
    }

    /// The product-schema configuration approving `candidate` on the `review` would make live
    /// now: see [`ProductSchemaReviewCandidate::going_live`].
    async fn going_live(
        &self,
        review: &ReviewUnderEvaluation,
        candidate: &ProductSchemaReviewCandidate,
    ) -> Result<ProductSchemaReviewCandidate, ReviewRepositoryError> {
        let listing_source_id = listing_source_id_from_uuid(review.listing_source_id)?;
        let base: Vec<RemovedPageSchema> =
            serde_json::from_value(review.removed_page_schemas_base.clone())
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
        let live = ListingSourceProductSchemaRepositoryImpl::new(&self.pool)
            .find_product_schema(&listing_source_id)
            .await?;
        let stored = RemovedPageSchemaRepositoryImpl::new(&self.pool)
            .find_removed_page_schema(&listing_source_id)
            .await?
            .map(|stored| stored.removed_page_schemas)
            .unwrap_or_default();
        candidate.going_live(&review.reason, live.as_ref(), &base, &stored)
    }

    /// The source listing id each page's listing was captured under, by page URL, for the
    /// `page_urls` whose listing has been captured. A page is matched with the crawler URL
    /// that names it, however the two spell it: a reviewer page is stored as fetched, often
    /// with the trailing slash the crawler URL's normalization drops. The exact spelling
    /// wins over another one.
    async fn captured_identities(
        &self,
        listing_source_id: uuid::Uuid,
        page_urls: &[&str],
    ) -> Result<HashMap<String, String>, ReviewRepositoryError> {
        let pages: Vec<(&str, Option<ReviewPageIdentity>)> = page_urls
            .iter()
            .map(|url| (*url, ReviewPageIdentity::of_stored(url)))
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
    /// candidate it evaluated and every page it evaluated, and returns it with the review's pages
    /// it did not evaluate. Pages that joined the review while it was evaluated are not in it
    /// until the next evaluation: they must not keep a refresh from ever storing its matrix while
    /// waiting pages keep joining.
    pub async fn store_schema_matrix_if_current(
        &self,
        review_id: CrawlerReviewId,
        evaluated: EvaluatedSchemaMatrix,
    ) -> Result<ReviewSchemaMatrix, ReviewRepositoryError> {
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
        let pages = fetch_review_pages(&mut *transaction, review_id).await?;
        if evaluated.candidate_version() != Some(candidate_version)
            || candidate_hash != evaluated.candidate_hash
            || !evaluated
                .page_ids
                .iter()
                .all(|id| pages.iter().any(|page| page.review_page_id.as_uuid() == id))
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
        Ok(ReviewSchemaMatrix::of(evaluated.matrix, &pages))
    }
}
