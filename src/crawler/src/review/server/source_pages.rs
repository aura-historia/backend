//! The console's fetches of pages of a review's ListingSource for those who author and review it:
//! a review agent's live previews stay on the review's ListingSource, a page is projected as the
//! schema generation prompt sees it, and a proposed candidate is evaluated on the review's pages and
//! on URLs of its ListingSource without being stored. A projection and an evaluation are live jobs
//! under the console's limit, and refused to a review agent on a review that waits for a human. The
//! contract is in `docs/crawler/extraction-and-review.md` (review rail) and
//! `docs/crawler/operations.md` (review service, review agents).

use super::schema_matrix::{matrix_refresh_deadline, matrix_refresh_timeout};
use super::{
    ReviewServer, internal_error, live_jobs_busy, live_url_fetch_error, review_error,
    review_page_url,
};
use crate::review::candidate_evaluations::CandidateEvaluationOutcome;
use crate::review::http::{HttpResponse, ParsedRequest};
use crate::review::model::{PAGE_ROLE_EVALUATED_URL, ReviewActor, SchemaReviewPageInput};
use crate::review::page_projection::{PageProjection, ProjectedHtml};
use crate::review::repository::{CandidateProposal, ReviewRepositoryError};
use crate::{CrawlerReviewId, CrawlerReviewPageId};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use url::Url;

impl ReviewServer {
    /// The URL a live preview (`/api/live-inspect`, `/api/live-html`) fetches: any URL for a
    /// human, and for a review agent only a URL of the ListingSource of the review its `review`
    /// query names, so an agent's token never fetches another host.
    pub(super) async fn live_preview_url(
        &self,
        request: &ParsedRequest<'_>,
        actor: ReviewActor,
    ) -> Result<Url, HttpResponse> {
        let Some(raw_url) = request.query.get("url") else {
            return Err(HttpResponse::json(400, &json!({ "error": "missing url" })));
        };
        if !actor.is_agent() {
            return Url::parse(raw_url).map_err(|err| {
                HttpResponse::json(
                    400,
                    &json!({
                        "error": "invalid live preview url",
                        "url": raw_url,
                        "details": err.to_string(),
                    }),
                )
            });
        }
        let Some(review_id) = request
            .query
            .get("review")
            .and_then(|review_id| review_id.parse::<CrawlerReviewId>().ok())
        else {
            return Err(HttpResponse::json(
                400,
                &json!({
                    "error": "a review agent's live fetch names the review whose ListingSource the url belongs to as `review`",
                }),
            ));
        };
        self.repository
            .review_source_url(review_id, raw_url)
            .await
            .map_err(review_error)
    }

    /// Fetches the review page or the URL of the review's ListingSource that `request`'s `page` or
    /// `url` query names and answers its projection, as a live job of `actor`'s.
    pub(super) async fn project_review_source_page(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
        request: &ParsedRequest<'_>,
    ) -> Result<PageProjection, HttpResponse> {
        self.repository
            .check_live_job(review_id, actor)
            .await
            .map_err(review_error)?;
        let (review_page_id, url) = match (request.query.get("page"), request.query.get("url")) {
            (Some(page), None) => {
                let page_id = page
                    .parse::<CrawlerReviewPageId>()
                    .map_err(|_| HttpResponse::json(400, &json!({ "error": "invalid page id" })))?;
                let page = self
                    .repository
                    .get_review_page(page_id)
                    .await
                    .map_err(internal_error)?
                    .filter(|page| page.review_id == review_id)
                    .ok_or_else(|| {
                        review_error(ReviewRepositoryError::ReviewPageNotFound(page_id))
                    })?;
                (Some(page_id), review_page_url(&page)?)
            }
            (None, Some(url)) => (
                None,
                self.repository
                    .review_source_url(review_id, url)
                    .await
                    .map_err(review_error)?,
            ),
            _ => {
                return Err(HttpResponse::json(
                    400,
                    &json!({ "error": "name either a review page as `page` or a url of the review's ListingSource as `url`" }),
                ));
            }
        };
        let live_job = self.live_jobs.try_start().map_err(live_jobs_busy)?;
        let fetched = self
            .fetch_live(&url)
            .await
            .map_err(|err| live_url_fetch_error(url.as_str(), &err))?;
        let html = fetched.html;
        let projected = tokio::task::spawn_blocking(move || {
            // The projection keeps its place under the limit while it parses, even when the
            // request stops waiting for it at its deadline.
            let _live_job = live_job;
            ProjectedHtml::of(&html)
        })
        .await
        .map_err(internal_error)?;
        Ok(PageProjection {
            review_id,
            review_page_id,
            url: url.to_string(),
            final_url: fetched.final_url.to_string(),
            projection: projected.projection,
            describes_one_product: projected.describes_one_product,
        })
    }

    /// Checks a candidate proposed for the review and starts its evaluation on the review's pages
    /// and the proposed URLs as a live job of `actor`'s, answering with the matrix when it finishes
    /// in time, otherwise with `202` and the evaluation to poll.
    pub(super) async fn evaluate_candidate(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
        payload: EvaluationPayload,
    ) -> HttpResponse {
        if let Err(error) = self.repository.check_live_job(review_id, actor).await {
            return review_error(error);
        }
        let proposal = match self
            .repository
            .propose_candidate(
                review_id,
                &serde_json::Value::Object(payload.candidate),
                &payload.urls,
            )
            .await
        {
            Ok(proposal) => proposal,
            Err(error) => return review_error(error),
        };
        let evaluation = self.clone().evaluate_proposal(proposal);
        match self
            .candidate_evaluations
            .start(review_id, &self.live_jobs, evaluation)
        {
            Ok((evaluation_id, started)) => {
                match started.outcome_within(self.matrix_refresh_wait).await {
                    Some(outcome) => candidate_evaluated(outcome),
                    None => candidate_evaluating(review_id, evaluation_id),
                }
            }
            Err(busy) => live_jobs_busy(busy),
        }
    }

    /// Answers an evaluation started for the review: `202` while it runs, then its outcome.
    pub(super) async fn candidate_evaluation(
        &self,
        review_id: CrawlerReviewId,
        evaluation_id: uuid::Uuid,
    ) -> HttpResponse {
        let Some(evaluation) = self.candidate_evaluations.find(review_id, evaluation_id) else {
            return HttpResponse::json(
                404,
                &json!({ "error": format!("candidate evaluation not found: {evaluation_id}; it finished more than ten minutes ago or was never started for this review") }),
            );
        };
        match evaluation.outcome_within(Duration::ZERO).await {
            Some(outcome) => candidate_evaluated(outcome),
            None => candidate_evaluating(review_id, evaluation_id),
        }
    }

    /// Fetches the proposal's review pages and URLs live within a deadline sized for the shop's
    /// pacing, and evaluates the proposed candidate on them.
    async fn evaluate_proposal(
        self,
        mut proposal: CandidateProposal,
    ) -> CandidateEvaluationOutcome {
        let review_id = proposal.review_id();
        let review_pages = std::mem::take(&mut proposal.pages);
        let urls = std::mem::take(&mut proposal.urls);
        let deadline = matrix_refresh_deadline(review_pages.len() + urls.len());
        let fetch_all = async {
            let pages = self.fetch_review_pages(review_pages).await?;
            let mut url_pages = Vec::with_capacity(urls.len());
            for url in urls {
                let html = self
                    .html_fetcher
                    .fetch(&url)
                    .await
                    .map(|fetched| fetched.html)
                    .map_err(|err| live_url_fetch_error(url.as_str(), &err))?;
                url_pages.push(SchemaReviewPageInput {
                    url: url.to_string(),
                    role: PAGE_ROLE_EVALUATED_URL.to_owned(),
                    raw_html: html,
                });
            }
            Ok((pages, url_pages))
        };
        let (pages, url_pages) = tokio::time::timeout(deadline, fetch_all)
            .await
            .unwrap_or_else(|_| Err(matrix_refresh_timeout(review_id, deadline)))?;
        self.repository
            .evaluate_candidate_proposal(proposal, pages, url_pages)
            .await
            .map_err(review_error)
    }
}

/// A candidate proposed for evaluation by `POST /api/reviews/{id}/evaluate`: the keys of a whole
/// product-schema candidate edit, and the URLs of the review's ListingSource to evaluate it on
/// next to the review's pages.
#[derive(Deserialize)]
pub(super) struct EvaluationPayload {
    #[serde(default)]
    urls: Vec<String>,
    #[serde(flatten)]
    candidate: serde_json::Map<String, serde_json::Value>,
}

/// The review and the evaluation that `/api/reviews/{review_id}/evaluations/{evaluation_id}` names.
pub(super) fn parse_review_evaluation_id(path: &str) -> Option<(CrawlerReviewId, uuid::Uuid)> {
    let rest = path.strip_prefix("/api/reviews/")?;
    let (review_id, evaluation_id) = rest.split_once("/evaluations/")?;
    Some((
        review_id.parse().ok()?,
        uuid::Uuid::parse_str(evaluation_id).ok()?,
    ))
}

/// Answers a finished candidate evaluation: the matrix, or why it failed.
fn candidate_evaluated(outcome: CandidateEvaluationOutcome) -> HttpResponse {
    match outcome {
        Ok(matrix) => HttpResponse::json(200, &matrix),
        Err(response) => response,
    }
}

fn candidate_evaluating(review_id: CrawlerReviewId, evaluation_id: uuid::Uuid) -> HttpResponse {
    HttpResponse::json(
        202,
        &json!({
            "status": "evaluating",
            "review_id": review_id,
            "evaluation_id": evaluation_id,
            "message": "pages are fetched at the shop's robots.txt Crawl-delay; poll GET /api/reviews/{review_id}/evaluations/{evaluation_id}",
        }),
    )
}
