//! Where the sample URLs a URL-pattern candidate takes lead when the console fetches them, the
//! evidence the approval guard reads to refuse a pattern whose URLs redirect to no product page,
//! such as a site builder's `goto` links. The console fetches the [redirect probe
//! URLs](crate::review::approval_guard::redirect_probe_urls) that have no recent fetch recorded and
//! records each final URL on its sample row. The contract is in
//! `docs/crawler/extraction-and-review.md` (approval guard).

use super::reviewer_page_url::ReviewPageIdentity;
use super::sample_labels::url_pattern_review_crawl_host;
use super::url_pattern_sample::fetch_sample_urls;
use super::{CrawlerReviewRepository, ReviewRepositoryError};
use crate::CrawlerReviewId;
use crate::review::approval_guard::{SampleRedirect, redirect_probe_urls};
use crate::review::model::{
    ARTIFACT_URL_PATTERN, STATUS_PENDING_REVIEW, UrlPatternReviewCandidate,
};
use crate::scraper::candidate_service::{RedirectDestination, stored_redirect_destination};
use listing_source_core::ListingSourceId;
use serde::Deserialize;
use url::Url;

/// How long a recorded fetch of a sample URL stands as evidence of where it leads, in seconds: a
/// day, so the console fetches a review's probe URLs at most about daily however often its approval
/// checks are read.
const REDIRECT_EVIDENCE_MAX_AGE_SECS: f64 = 86_400.0;

impl CrawlerReviewRepository {
    /// The redirect probe URLs of the pending URL-pattern review `review_id`'s current candidate
    /// without a fetch recorded within [`REDIRECT_EVIDENCE_MAX_AGE_SECS`], which the console
    /// fetches before the guard reads them. None for any other review, and for a candidate that
    /// does not decode or has no pattern, which fail elsewhere.
    pub async fn redirect_probes_due(
        &self,
        review_id: CrawlerReviewId,
    ) -> Result<Vec<Url>, ReviewRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let candidate: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT candidate_payload FROM crawler_reviews \
             WHERE review_id = $1 AND artifact_type = $2 AND status = $3",
        )
        .bind(review_id.as_uuid())
        .bind(ARTIFACT_URL_PATTERN)
        .bind(STATUS_PENDING_REVIEW)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(candidate) =
            candidate.and_then(|payload| UrlPatternReviewCandidate::deserialize(&payload).ok())
        else {
            return Ok(Vec::new());
        };
        let crawl_host = url_pattern_review_crawl_host(&mut transaction, review_id).await?;
        let sample = fetch_sample_urls(&mut *transaction, review_id).await?;
        let mut due = Vec::new();
        for url in redirect_probe_urls(&candidate, &crawl_host, &sample) {
            if recorded_redirect_target(&mut transaction, review_id, &url)
                .await?
                .is_none()
            {
                due.push(url);
            }
        }
        Ok(due)
    }

    /// Records that the console fetched the sample URL `url` of the review `review_id` now and
    /// that it led to `final_url`, on its sample row under any spelling the crawler stores it
    /// under. A URL outside the review's sample records nothing.
    pub async fn record_sample_redirect(
        &self,
        review_id: CrawlerReviewId,
        url: &Url,
        final_url: &Url,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE crawler_review_urls \
             SET redirect_target = $3, redirect_checked = clock_timestamp() \
             WHERE review_id = $1 AND url = ANY($2)",
        )
        .bind(review_id.as_uuid())
        .bind(ReviewPageIdentity::spellings(url))
        .bind(final_url.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// The recorded fetches of the review's `probe_urls`, as the approval guard reads them: each
/// with whether the crawler stores its redirect target as no product URL of the ListingSource
/// `listing_source_id`. A probe URL without a recent fetch is left out.
pub(super) async fn fetch_sample_redirects(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    listing_source_id: &ListingSourceId,
    probe_urls: &[Url],
) -> Result<Vec<SampleRedirect>, sqlx::Error> {
    let mut redirects = Vec::with_capacity(probe_urls.len());
    for url in probe_urls {
        let Some(final_url) = recorded_redirect_target(transaction, review_id, url).await? else {
            continue;
        };
        let final_url_stored_as_non_product = final_url != *url
            && stored_redirect_destination(&mut **transaction, listing_source_id, &final_url)
                .await?
                == RedirectDestination::NotAProductOfListingSource;
        redirects.push(SampleRedirect {
            url: url.clone(),
            final_url,
            final_url_stored_as_non_product,
        });
    }
    Ok(redirects)
}

/// Where the sample URL `url` led when the console fetched it within
/// [`REDIRECT_EVIDENCE_MAX_AGE_SECS`]; a stored target that is no URL fails closed.
async fn recorded_redirect_target(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    review_id: CrawlerReviewId,
    url: &Url,
) -> Result<Option<Url>, sqlx::Error> {
    let target: Option<String> = sqlx::query_scalar(
        "SELECT redirect_target FROM crawler_review_urls \
         WHERE review_id = $1 AND url = ANY($2) AND redirect_target IS NOT NULL \
           AND redirect_checked > NOW() - make_interval(secs => $3) \
         ORDER BY redirect_checked DESC LIMIT 1",
    )
    .bind(review_id.as_uuid())
    .bind(ReviewPageIdentity::spellings(url))
    .bind(REDIRECT_EVIDENCE_MAX_AGE_SECS)
    .fetch_optional(&mut **transaction)
    .await?;
    target
        .map(|target| {
            Url::parse(&target).map_err(|error| {
                sqlx::Error::Decode(
                    format!("invalid persisted crawler_review_urls.redirect_target: {error}")
                        .into(),
                )
            })
        })
        .transpose()
}
