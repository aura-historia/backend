//! The console's live schema matrix of a product-schema review: reading the stored matrix, the
//! running refresh or its failure, and refreshing it from the review's pages fetched live. The
//! contract is in `docs/crawler/extraction-and-review.md` (review rail) and
//! `docs/crawler/networking.md`.

use super::{ReviewServer, live_fetch_error, review_error, review_page_url};
use crate::CrawlerReviewId;
use crate::network::robots::MAX_CRAWL_DELAY;
use crate::review::http::HttpResponse;
use crate::review::matrix_refresh::{MatrixRefreshOutcome, MatrixRefreshStatus};
use crate::review::model::{
    ARTIFACT_PRODUCT_SCHEMA, CrawlerReviewPage, ReviewActor, ReviewDetail, ReviewSchemaMatrix,
    SchemaMatrix,
};
use crate::review::repository::ReviewRepositoryError;
use crate::review::schema_evaluation::stored_schema_matrix;
use serde_json::json;
use std::time::Duration;
use tracing::warn;

/// A matrix refresh fetches every review page at the shop's `Crawl-delay`, so it runs in
/// the background with a budget per page: the longest honoured delay plus a full fetch.
const MATRIX_REFRESH_PAGE_BUDGET: Duration =
    MAX_CRAWL_DELAY.saturating_add(Duration::from_secs(30));

impl ReviewServer {
    /// `GET /api/reviews/{id}/matrix`: the running refresh, the last refresh's failure or the
    /// stored matrix, and a new refresh while the review stores none.
    pub(super) async fn read_schema_matrix(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
    ) -> HttpResponse {
        match self.matrix_refreshes.status(review_id) {
            MatrixRefreshStatus::Running => return matrix_refreshing(review_id),
            MatrixRefreshStatus::Failed(response) => return response,
            MatrixRefreshStatus::Idle => {}
        }
        let detail = match self.product_schema_review(review_id).await {
            Ok(detail) => detail,
            Err(response) => return response,
        };
        match cached_schema_matrix(&detail) {
            Some(matrix) => HttpResponse::json(200, &ReviewSchemaMatrix::of(matrix, &detail.pages)),
            None => self.start_schema_matrix_refresh(review_id, actor).await,
        }
    }

    /// `GET /api/reviews/{id}/matrix?refresh=true`: starts `actor`'s refresh of the review's
    /// matrix, or joins the running one.
    pub(super) async fn refresh_schema_matrix(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
    ) -> HttpResponse {
        if let Err(response) = self.product_schema_review(review_id).await {
            return response;
        }
        self.start_schema_matrix_refresh(review_id, actor).await
    }

    /// The review, refused unless it is a product-schema review: only it has pages to evaluate
    /// live, a URL-pattern review is judged on its sample labels. An unknown review is refused
    /// before a refresh starts, so requests for unknown ids leave no refresh behind in the
    /// registry.
    async fn product_schema_review(
        &self,
        review_id: CrawlerReviewId,
    ) -> Result<ReviewDetail, HttpResponse> {
        let detail = self
            .repository
            .get_review(review_id)
            .await
            .map_err(review_error)?;
        if detail.review.artifact_type != ARTIFACT_PRODUCT_SCHEMA {
            return Err(review_error(ReviewRepositoryError::UnsupportedArtifact(
                review_id,
                detail.review.artifact_type,
            )));
        }
        Ok(detail)
    }

    /// Starts `actor`'s background refresh, or joins the running one, and answers with the matrix
    /// when it finishes in time, otherwise with `202` for the console to poll.
    async fn start_schema_matrix_refresh(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
    ) -> HttpResponse {
        if let Err(error) = self.repository.check_matrix_refresh(review_id, actor).await {
            return review_error(error);
        }
        let refresh = self.clone().evaluate_live_schema_matrix(review_id, actor);
        match self
            .matrix_refreshes
            .start_or_join(review_id, refresh)
            .outcome_within(self.matrix_refresh_wait)
            .await
        {
            Some(Ok(matrix)) => HttpResponse::json(200, &matrix),
            Some(Err(response)) => response,
            None => matrix_refreshing(review_id),
        }
    }

    /// Fetches every review page live within a deadline sized for the shop's pacing.
    async fn live_review_pages(
        &self,
        review_id: CrawlerReviewId,
    ) -> Result<Vec<(CrawlerReviewPage, String)>, HttpResponse> {
        let detail = self
            .repository
            .get_review(review_id)
            .await
            .map_err(review_error)?;
        let deadline = matrix_refresh_deadline(detail.pages.len());
        tokio::time::timeout(deadline, self.fetch_review_pages(detail.pages))
            .await
            .unwrap_or_else(|_| Err(matrix_refresh_timeout(review_id, deadline)))
    }

    /// Fetches each of `pages` live, one after the other at the shop's pacing.
    pub(super) async fn fetch_review_pages(
        &self,
        pages: Vec<CrawlerReviewPage>,
    ) -> Result<Vec<(CrawlerReviewPage, String)>, HttpResponse> {
        let mut fetched = Vec::with_capacity(pages.len());
        for page in pages {
            let url = review_page_url(&page)?;
            let html = self
                .html_fetcher
                .fetch(&url)
                .await
                .map(|fetched| fetched.html)
                .map_err(|err| live_fetch_error(&page, &err))?;
            fetched.push((page, html));
        }
        Ok(fetched)
    }

    async fn evaluate_live_schema_matrix(
        self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
    ) -> MatrixRefreshOutcome {
        let fetched_at = time::OffsetDateTime::now_utc();
        let pages = self.live_review_pages(review_id).await?;
        let evaluated = self
            .repository
            .evaluate_schema_matrix_for_live_pages(review_id, pages, fetched_at)
            .await
            .map_err(review_error)?;
        self.repository
            .store_schema_matrix_if_current(review_id, actor, evaluated)
            .await
            .map_err(review_error)
    }
}

/// The schema matrix cached on the review with the cross-page findings of the current checks,
/// or `None` when it holds none, or one that no longer decodes, so the caller refreshes it from
/// the live pages.
fn cached_schema_matrix(detail: &ReviewDetail) -> Option<SchemaMatrix> {
    stored_schema_matrix(&detail.review.validation_summary)?
        .inspect_err(|err| {
            warn!(
                review_id = %detail.review.review_id,
                error = ?err,
                "Cached schema matrix is invalid; refreshing from live pages"
            );
        })
        .ok()
}

pub(super) fn matrix_refresh_deadline(page_count: usize) -> Duration {
    MATRIX_REFRESH_PAGE_BUDGET.saturating_mul(u32::try_from(page_count.max(1)).unwrap_or(u32::MAX))
}

fn matrix_refreshing(review_id: CrawlerReviewId) -> HttpResponse {
    HttpResponse::json(
        202,
        &json!({
            "status": "refreshing",
            "review_id": review_id,
            "message": "live review pages are fetched at the shop's robots.txt Crawl-delay; poll the matrix without refresh",
        }),
    )
}

pub(super) fn matrix_refresh_timeout(
    review_id: CrawlerReviewId,
    deadline: Duration,
) -> HttpResponse {
    warn!(review_id = %review_id, deadline_secs = deadline.as_secs(), "Schema matrix refresh timed out");
    HttpResponse::json(
        502,
        &json!({
            "error": "schema matrix refresh timed out",
            "details": format!("live review pages did not finish within {}s", deadline.as_secs()),
        }),
    )
}
