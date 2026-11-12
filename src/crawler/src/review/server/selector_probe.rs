//! The console's selector probe: a few selectors applied to a few review pages or URLs of the
//! review's ListingSource, answering only what each matches (`POST
//! /api/reviews/{id}/selector-probe`). Like a page projection it is a live job under the
//! console's limit, refused to a review agent on a review that waits for a human and on a
//! URL-pattern review, and each page it fetches is recorded as inspected. The contract is in
//! `docs/crawler/operations.md` (review MCP server).

use super::{ReviewServer, internal_error, live_jobs_busy, review_error, review_page_url};
use crate::network::policy::NetworkErrorKind;
use crate::review::http::HttpResponse;
use crate::review::model::ReviewActor;
use crate::review::product_markup::describes_one_product;
use crate::review::repository::ReviewRepositoryError;
use crate::review::selector_probe::{
    MAX_PROBED_PAGES, MAX_PROBED_SELECTORS, ProbeResult, SelectorProbe, probe,
};
use crate::{CrawlerReviewId, CrawlerReviewPageId};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;
use tokio::time::Instant;
use tracing::warn;
use url::Url;

/// How long a probe fetches pages: within the console's request deadline, so a probe of a slowly
/// paced shop answers the pages it fetched and names the others.
const PROBE_FETCH_DEADLINE: Duration = Duration::from_secs(22);

/// A probe's request: the pages to fetch, each a review page or a URL of the review's
/// ListingSource, and the selectors to apply to each.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SelectorProbePayload {
    pages: Vec<ProbedPageRequest>,
    selectors: Vec<SelectorProbe>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbedPageRequest {
    #[serde(default)]
    page: Option<CrawlerReviewPageId>,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct SelectorProbeAnswer {
    review_id: CrawlerReviewId,
    pages: Vec<ProbedPage>,
}

/// What the selectors matched on one page, or why the page was not probed.
#[derive(Debug, Serialize)]
struct ProbedPage {
    #[serde(skip_serializing_if = "Option::is_none")]
    review_page_id: Option<CrawlerReviewPageId>,
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    final_url: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    results: Vec<ProbeResult>,
    /// Why the page was not probed: the shop did not serve it, or the probe's time ran out.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// `BOT_CHALLENGE` when the shop answered with a bot challenge, `TIMEOUT` when the probe ran
    /// out of time before it reached the page.
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

impl ReviewServer {
    /// Fetches the pages `payload` names, one after the other at the shop's pacing, and applies its
    /// selectors to each, as a live job of `actor`'s.
    pub(super) async fn probe_selectors(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
        payload: SelectorProbePayload,
    ) -> HttpResponse {
        if let Err(response) = check_size(&payload) {
            return response;
        }
        if let Err(error) = self.repository.check_live_job(review_id, actor).await {
            return review_error(error);
        }
        if let Err(error) = self
            .repository
            .check_page_inspection(review_id, actor)
            .await
        {
            return review_error(error);
        }
        let pages = match self.probed_pages(review_id, payload.pages).await {
            Ok(pages) => pages,
            Err(response) => return response,
        };
        let live_job = match self.live_jobs.try_start() {
            Ok(live_job) => live_job,
            Err(busy) => return live_jobs_busy(busy),
        };
        let selectors = std::sync::Arc::new(payload.selectors);
        let deadline = Instant::now() + PROBE_FETCH_DEADLINE;
        let mut probed = Vec::with_capacity(pages.len());
        for (review_page_id, url) in pages {
            probed.push(
                self.probe_page(review_id, review_page_id, url, &selectors, deadline)
                    .await,
            );
        }
        drop(live_job);
        HttpResponse::json(
            200,
            &SelectorProbeAnswer {
                review_id,
                pages: probed,
            },
        )
    }

    /// The URL of each requested page: a review page of the review, or a URL of its ListingSource.
    async fn probed_pages(
        &self,
        review_id: CrawlerReviewId,
        requested: Vec<ProbedPageRequest>,
    ) -> Result<Vec<(Option<CrawlerReviewPageId>, Url)>, HttpResponse> {
        let mut pages = Vec::with_capacity(requested.len());
        for page in requested {
            pages.push(match (page.page, page.url) {
                (Some(page_id), None) => {
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
                        .review_source_url(review_id, &url)
                        .await
                        .map_err(review_error)?,
                ),
                _ => {
                    return Err(HttpResponse::json(
                        400,
                        &json!({ "error": "name each page either as a review page `page` or as a `url` of the review's ListingSource" }),
                    ));
                }
            });
        }
        Ok(pages)
    }

    /// Fetches `url` and applies `selectors` to it on a blocking thread, or names why it could not
    /// before `deadline`. A fetched page is recorded as inspected.
    async fn probe_page(
        &self,
        review_id: CrawlerReviewId,
        review_page_id: Option<CrawlerReviewPageId>,
        url: Url,
        selectors: &std::sync::Arc<Vec<SelectorProbe>>,
        deadline: Instant,
    ) -> ProbedPage {
        let unprobed = |error: String, reason: Option<&'static str>| ProbedPage {
            review_page_id,
            url: url.to_string(),
            final_url: None,
            results: Vec::new(),
            error: Some(error),
            reason,
        };
        let fetched = match tokio::time::timeout_at(deadline, self.fetch_live(&url)).await {
            Err(_) => {
                return unprobed(
                    "the probe ran out of time before it fetched this page; probe fewer pages at once".to_owned(),
                    Some("TIMEOUT"),
                );
            }
            Ok(Err(error)) if error.kind() == NetworkErrorKind::BotChallenge => {
                return unprobed(
                    "the shop answered with a bot challenge instead of the page; retry later"
                        .to_owned(),
                    Some("BOT_CHALLENGE"),
                );
            }
            Ok(Err(error)) => return unprobed(format!("failed to fetch the page: {error}"), None),
            Ok(Ok(fetched)) => fetched,
        };
        let html = fetched.html;
        let probes = std::sync::Arc::clone(selectors);
        let probed = tokio::task::spawn_blocking(move || {
            let product = describes_one_product(&html);
            let results = probe(&scraper::Html::parse_document(&html), &probes);
            (results, product)
        })
        .await;
        let (results, product) = match probed {
            Ok(probed) => probed,
            Err(error) => return unprobed(format!("the probe failed: {error}"), None),
        };
        // A probed page counts as seen, for a no-sold-page record too; a failed write loses only
        // that evidence, so the record refuses more.
        if let Err(error) = self
            .repository
            .record_inspected_page(review_id, &url, product)
            .await
        {
            warn!(
                review_id = %review_id,
                error = %error,
                cause = ?std::error::Error::source(&error),
                "Failed to record a probed page"
            );
        }
        ProbedPage {
            review_page_id,
            url: url.to_string(),
            final_url: Some(fetched.final_url.to_string()),
            results,
            error: None,
            reason: None,
        }
    }
}

/// Refuses a probe of no page or selector, or of more than the probe takes.
fn check_size(payload: &SelectorProbePayload) -> Result<(), HttpResponse> {
    let refuse = |error: String| Err(HttpResponse::json(400, &json!({ "error": error })));
    if payload.pages.is_empty() || payload.pages.len() > MAX_PROBED_PAGES {
        return refuse(format!(
            "name 1 to {MAX_PROBED_PAGES} pages, not {}",
            payload.pages.len()
        ));
    }
    if payload.selectors.is_empty() || payload.selectors.len() > MAX_PROBED_SELECTORS {
        return refuse(format!(
            "name 1 to {MAX_PROBED_SELECTORS} selectors, not {}",
            payload.selectors.len()
        ));
    }
    Ok(())
}
