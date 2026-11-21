//! The console's routes for the qualification of a ListingSource as a real art and antiques e-shop:
//! the agents' queue of the sources that wait for one, the pages of a queued source as a live job,
//! and a decision; a human decides or clears any source's qualification on the source's own route.
//! The contract is in `docs/crawler/operations.md` (review service).

use super::query_params::named;
use super::{
    ReviewServer, internal_error, live_jobs_busy, live_url_fetch_error,
    parse_listing_source_id_with_suffix, review_error, verdict_answer,
};
use crate::review::http::{HttpResponse, ParsedRequest};
use crate::review::model::{QualificationDecision, ReviewActor};
use crate::review::page_projection::{PageView, ProjectedHtml, SourcePageProjection, page_links};
use crate::review::readers::QualificationQueue;
use listing_source_core::ListingSourceId;
use serde::Serialize;
use serde_json::json;

const QUALIFICATIONS: &str = "/api/source-qualifications";
/// Sends the sources whose agent park label expired back to qualification.
const RETRY_PARKED: &str = "/api/source-qualifications/retry-parked";
const QUALIFICATION_SUFFIX: &str = "/qualification";
const PAGE_SUFFIX: &str = "/page";

/// `GET /api/source-qualifications`: the queue and the decisions the review agents have left this
/// hour.
#[derive(Serialize)]
struct QueueAnswer<'a> {
    #[serde(flatten)]
    queue: &'a QualificationQueue,
    agent_decisions_left: u32,
}

impl ReviewServer {
    /// Answers the qualification routes, or `None` for any other request:
    /// `GET /api/source-qualifications`, `POST /api/source-qualifications/retry-parked`,
    /// `GET /api/source-qualifications/{id}/page`,
    /// `POST /api/source-qualifications/{id}` and `PUT` and `DELETE`
    /// `/api/listing-sources/{id}/qualification`.
    pub(super) async fn route_source_qualification(
        &self,
        request: &ParsedRequest<'_>,
        actor: ReviewActor,
    ) -> Option<HttpResponse> {
        let method = request.method.as_str();
        if method == "GET" && request.path == QUALIFICATIONS {
            return Some(self.qualification_queue_answer().await);
        }
        if method == "POST" && request.path == RETRY_PARKED {
            return Some(match self.repository.retry_parked_sources(actor).await {
                Ok(retried) => HttpResponse::json(200, &json!({ "retried": retried })),
                Err(error) => review_error(error),
            });
        }
        if let Some(rest) = request
            .path
            .strip_prefix(QUALIFICATIONS)
            .and_then(|rest| rest.strip_prefix('/'))
        {
            return Some(match (method, rest.strip_suffix(PAGE_SUFFIX)) {
                ("GET", Some(id)) => match id.parse::<ListingSourceId>().ok() {
                    Some(listing_source_id) => self
                        .project_source_page(listing_source_id, request)
                        .await
                        .unwrap_or_else(|response| response),
                    None => invalid_listing_source_id(),
                },
                ("POST", None) => match rest.parse::<ListingSourceId>().ok() {
                    Some(listing_source_id) => {
                        self.decide_qualification(listing_source_id, actor, request.body)
                            .await
                    }
                    None => invalid_listing_source_id(),
                },
                _ => return None,
            });
        }
        if request.path.starts_with("/api/listing-sources/")
            && request.path.ends_with(QUALIFICATION_SUFFIX)
            && matches!(method, "PUT" | "DELETE")
        {
            let Some(listing_source_id) =
                parse_listing_source_id_with_suffix(request.path, QUALIFICATION_SUFFIX)
            else {
                return Some(invalid_listing_source_id());
            };
            return Some(if method == "PUT" {
                self.decide_qualification(listing_source_id, actor, request.body)
                    .await
            } else {
                verdict_answer(
                    self.repository
                        .clear_qualification(&listing_source_id, actor)
                        .await,
                )
            });
        }
        None
    }

    /// The qualification queue with how many decisions the review agents have left this hour, so
    /// the runner starts no more qualifiers than can decide.
    async fn qualification_queue_answer(&self) -> HttpResponse {
        let queue = match self.qualification_queue.read().await {
            Ok(queue) => queue,
            Err(error) => return internal_error(error),
        };
        match self.repository.agent_decisions_left().await {
            Ok(agent_decisions_left) => HttpResponse::json(
                200,
                &QueueAnswer {
                    queue: &queue,
                    agent_decisions_left,
                },
            ),
            Err(error) => review_error(error),
        }
    }

    async fn decide_qualification(
        &self,
        listing_source_id: ListingSourceId,
        actor: ReviewActor,
        body: &str,
    ) -> HttpResponse {
        let decision: QualificationDecision = match serde_json::from_str(body) {
            Ok(decision) => decision,
            Err(error) => return HttpResponse::json(400, &json!({ "error": error.to_string() })),
        };
        match self
            .repository
            .decide_qualification(&listing_source_id, actor, &decision)
            .await
        {
            Ok(outcome) => HttpResponse::json(200, &outcome),
            Err(error) => review_error(error),
        }
    }

    /// Fetches the URL of the ListingSource that `request`'s `url` query names, its crawl root
    /// when it names none, and answers its projection in the `view` it names, the outline by
    /// default, with the page's links from `links_offset` on, as a live job. The fetch obeys robots.txt like every crawler fetch: a page
    /// robots.txt refuses answers `502` with `reason` `ROBOTS_DISALLOWED` and the refusing rule.
    async fn project_source_page(
        &self,
        listing_source_id: ListingSourceId,
        request: &ParsedRequest<'_>,
    ) -> Result<HttpResponse, HttpResponse> {
        let view = named::<PageView>(&request.query, "view")
            .map_err(|error| HttpResponse::json(400, &json!({ "error": error })))?
            .unwrap_or_default();
        let links_offset = match request.query.get("links_offset") {
            None => 0,
            Some(offset) => offset.trim().parse::<usize>().map_err(|_| {
                HttpResponse::json(
                    400,
                    &json!({ "error": "links_offset must be a whole number" }),
                )
            })?,
        };
        let crawl_root = self
            .repository
            .listing_source_crawl_root(&listing_source_id)
            .await
            .map_err(review_error)?
            .ok_or_else(|| {
                HttpResponse::json(
                    409,
                    &json!({ "error": "the ListingSource has no crawl root", "reason": "NOT_IN_CRAWL_SCOPE" }),
                )
            })?;
        // A path names a page of the crawl root's host.
        let raw_url = match request.query.get("url").map(|url| url.trim()) {
            None | Some("") => crawl_root,
            Some(path) if path.starts_with('/') && !path.starts_with("//") => {
                url::Url::parse(&crawl_root)
                    .and_then(|root| root.join(path))
                    .map(|url| url.to_string())
                    .unwrap_or_else(|_| path.to_owned())
            }
            Some(url) => url.to_owned(),
        };
        let url = self
            .repository
            .listing_source_url(&listing_source_id, &raw_url)
            .await
            .map_err(review_error)?;
        let live_job = self.live_jobs.try_start().map_err(live_jobs_busy)?;
        let fetched = self
            .fetch_live(&url)
            .await
            .map_err(|error| live_url_fetch_error(url.as_str(), &error))?;
        let html = fetched.html;
        let final_url = fetched.final_url.clone();
        let (projected, links) = tokio::task::spawn_blocking(move || {
            let _live_job = live_job;
            (
                ProjectedHtml::of(&html, view),
                page_links(&html, &final_url, links_offset),
            )
        })
        .await
        .map_err(internal_error)?;
        Ok(HttpResponse::json(
            200,
            &SourcePageProjection {
                listing_source_id,
                url: url.to_string(),
                final_url: fetched.final_url.to_string(),
                view: projected.view,
                projection: projected.projection,
                describes_one_product: projected.describes_one_product,
                links: links.links,
                links_offset,
                links_total: links.total,
                other_hosts: links.other_hosts,
            },
        ))
    }
}

fn invalid_listing_source_id() -> HttpResponse {
    HttpResponse::json(400, &json!({ "error": "invalid ListingSource id" }))
}
