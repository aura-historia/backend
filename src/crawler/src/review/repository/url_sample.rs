//! The URL sample of a URL-pattern review as the crawler knows it, and its live evaluation: the
//! sample URLs a refresh fetches for schema.org `Product` markup for the review's current
//! decision, and storing what their pages showed while the review still holds that decision.

use super::{CrawlerReviewRepository, ReviewRepositoryError, listing_source_id_from_uuid};
use crate::CrawlerReviewId;
use crate::review::model::{
    ARTIFACT_URL_PATTERN, EvaluatedSampleUrl, LiveMatrixEvaluation, UrlPatternReviewCandidate,
    UrlSampleEvaluation,
};
use crate::review::url_sample_evidence::{SampleUrl, SampleUrlToFetch, sample_urls_to_fetch};
use listing_source_core::ListingSourceId;
use serde::Deserialize;
use sqlx::Row;
use time::OffsetDateTime;

/// The review's URL sample in its stored order, each URL with whether the crawler captured a
/// product from it.
pub(super) async fn fetch_url_sample<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    review_id: CrawlerReviewId,
    listing_source_id: &ListingSourceId,
) -> Result<Vec<SampleUrl>, sqlx::Error> {
    Ok(sqlx::query_as::<_, (String, bool)>(
        "SELECT u.url, EXISTS ( \
             SELECT 1 FROM listing_source_urls s \
             WHERE s.listing_source_id = $2 AND s.url = u.url \
               AND s.last_source_listing_id IS NOT NULL \
         ) \
         FROM crawler_review_urls u WHERE u.review_id = $1 \
         ORDER BY u.created, u.url",
    )
    .bind(review_id.as_uuid())
    .bind(uuid::Uuid::from(*listing_source_id))
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(|(url, captured_product)| SampleUrl {
        url,
        captured_product,
    })
    .collect())
}

/// What a live evaluation of a URL-pattern review's sample fetches, chosen for the review's
/// decision at its candidate version.
pub struct UrlSampleToEvaluate {
    candidate_version: i64,
    decision_sha256: String,
    urls: Vec<SampleUrlToFetch>,
}

impl UrlSampleToEvaluate {
    /// The sample URLs to fetch, each with whether the evaluated decision classifies it as a
    /// product page.
    pub fn urls(&self) -> impl Iterator<Item = (&str, bool)> {
        self.urls
            .iter()
            .map(|url| (url.url.as_str(), url.decision_matches))
    }

    pub fn url_count(&self) -> usize {
        self.urls.len()
    }

    /// The evaluation of the review `review_id` whose sample URLs, fetched from `fetched_at` on,
    /// showed what `urls` lists.
    pub fn evaluated(
        self,
        review_id: CrawlerReviewId,
        urls: Vec<EvaluatedSampleUrl>,
        fetched_at: OffsetDateTime,
    ) -> UrlSampleEvaluation {
        UrlSampleEvaluation {
            review_id,
            live_evaluation: LiveMatrixEvaluation {
                candidate_version: self.candidate_version,
                configuration_sha256: self.decision_sha256,
                fetched_at,
            },
            urls,
        }
    }
}

impl CrawlerReviewRepository {
    /// The sample URLs a live evaluation of the URL-pattern review fetches for its current
    /// decision. A review of another artifact has no URL sample to evaluate.
    pub async fn url_sample_to_evaluate(
        &self,
        review_id: CrawlerReviewId,
    ) -> Result<UrlSampleToEvaluate, ReviewRepositoryError> {
        let review = sqlx::query(
            "SELECT listing_source_id, artifact_type, candidate_payload, candidate_version \
             FROM crawler_reviews WHERE review_id = $1",
        )
        .bind(review_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(ReviewRepositoryError::NotFound(review_id))?;
        let artifact_type: String = review.try_get("artifact_type")?;
        if artifact_type != ARTIFACT_URL_PATTERN {
            return Err(ReviewRepositoryError::UnsupportedArtifact(
                review_id,
                artifact_type,
            ));
        }
        let candidate = url_pattern_candidate(&review.try_get("candidate_payload")?)?;
        let listing_source_id = listing_source_id_from_uuid(review.try_get("listing_source_id")?)?;
        let sample = fetch_url_sample(&self.pool, review_id, &listing_source_id).await?;
        Ok(UrlSampleToEvaluate {
            candidate_version: review.try_get("candidate_version")?,
            decision_sha256: candidate.decision_sha256(),
            urls: sample_urls_to_fetch(&candidate, &sample),
        })
    }

    /// Stores the evaluation as the review's live evaluation of its sample while the review still
    /// holds the decision, at the candidate version, it was made for, and returns it. A candidate
    /// change during the evaluation is refused as for a matrix refresh, and stores nothing.
    pub async fn store_url_sample_evaluation_if_current(
        &self,
        review_id: CrawlerReviewId,
        evaluation: UrlSampleEvaluation,
    ) -> Result<UrlSampleEvaluation, ReviewRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        // The row lock waits for edits in flight, which lock the review first.
        let (candidate_version, candidate_payload) = sqlx::query_as::<_, (i64, serde_json::Value)>(
            "SELECT candidate_version, candidate_payload FROM crawler_reviews \
                 WHERE review_id = $1 FOR UPDATE",
        )
        .bind(review_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(ReviewRepositoryError::NotFound(review_id))?;
        let evaluated = &evaluation.live_evaluation;
        if evaluated.candidate_version != candidate_version
            || evaluated.configuration_sha256
                != url_pattern_candidate(&candidate_payload)?.decision_sha256()
        {
            return Err(ReviewRepositoryError::CandidateChangedDuringEvaluation {
                candidate_version,
            });
        }
        sqlx::query(
            "UPDATE crawler_reviews
             SET validation_summary = jsonb_set(
                     CASE jsonb_typeof(validation_summary)
                         WHEN 'object' THEN validation_summary
                         ELSE jsonb_build_object('summary', validation_summary)
                     END,
                     ARRAY[$3::text],
                     $2::jsonb,
                     true
                 ),
                 updated = NOW()
             WHERE review_id = $1",
        )
        .bind(review_id.as_uuid())
        .bind(serde_json::to_value(&evaluation)?)
        .bind(UrlSampleEvaluation::STORED_KEY)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(evaluation)
    }
}

fn url_pattern_candidate(
    payload: &serde_json::Value,
) -> Result<UrlPatternReviewCandidate, ReviewRepositoryError> {
    UrlPatternReviewCandidate::deserialize(payload)
        .map_err(|_| ReviewRepositoryError::InvalidUrlPatternCandidate)
}
