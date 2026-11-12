//! The console's structured-data draft of a product-schema review (`POST
//! /api/reviews/{id}/draft`): it fetches the review's pages, drafts a schema from what they show
//! ([`draft_schema`]) and stages it in place of the review's untouched starter schema as the
//! requesting actor's edit. When no review page then shows a sold or unavailable product, it
//! applies the draft's state rule to a few product URLs of the review's ListingSource and adds the
//! first sold one as a review page. It is a live job under the console's limit, refused to a
//! review agent on a review that waits for a human; the draft decides nothing, the approval guard
//! and the reviewer judge it as any candidate. The contract is in
//! `docs/crawler/extraction-and-review.md` (structured-data draft) and
//! `docs/crawler/operations.md` (review MCP server).

use super::{ReviewServer, live_jobs_busy, review_error};
use crate::CrawlerReviewId;
use crate::review::http::HttpResponse;
use crate::review::model::{ARTIFACT_PRODUCT_SCHEMA, CrawlerReviewPage, ReviewActor};
use crate::review::product_markup::describes_one_product;
use crate::review::repository::ReviewRepositoryError;
use crate::review::structured_draft::{StructuredDraft, UndraftedField, draft_schema, shows_sold};
use crate::scraper::css_selector::rule::ExtractionRule;
use serde::Serialize;
use std::time::Duration;
use tokio::time::Instant;
use tracing::{info, warn};
use url::Url;

/// How long a draft fetches pages, review pages and sold-page candidates together: within the
/// console's request deadline.
const DRAFT_FETCH_DEADLINE: Duration = Duration::from_secs(24);

/// The most ListingSource URLs a draft fetches to find a sold page.
pub(crate) const MAX_SOLD_PAGE_CANDIDATES: usize = 8;

/// Why no draft was staged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum NotDrafted {
    /// The review holds no untouched starter schema: an agent or a human already wrote one.
    NoStarterSchema,
    /// No title rule finds the product's name on every review page, so no schema would apply.
    NoTemplateAnchor,
}

#[derive(Debug, Serialize)]
struct DraftAnswer {
    review_id: CrawlerReviewId,
    drafted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<NotDrafted>,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidate_version: Option<i64>,
    /// Fields the pages need that no drafted rule fits, which the author writes.
    undrafted: Vec<UndraftedField>,
    /// Whether every field the pages need has a rule.
    complete: bool,
    /// The sold page the draft added as a review page, if it searched and found one.
    #[serde(skip_serializing_if = "Option::is_none")]
    sold_page_added: Option<String>,
    /// How many ListingSource URLs the sold-page search fetched.
    sold_page_candidates_checked: usize,
}

impl ReviewServer {
    pub(super) async fn draft_candidate(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
    ) -> HttpResponse {
        let detail = match self.repository.get_review(review_id).await {
            Ok(detail) => detail,
            Err(error) => return review_error(error),
        };
        if detail.review.artifact_type != ARTIFACT_PRODUCT_SCHEMA {
            return review_error(ReviewRepositoryError::UnsupportedArtifact(
                review_id,
                detail.review.artifact_type,
            ));
        }
        if !actor.may_change_candidates() {
            return review_error(ReviewRepositoryError::ReviewerMayNotChangeCandidates);
        }
        if let Err(error) = self.repository.check_live_job(review_id, actor).await {
            return review_error(error);
        }
        let live_job = match self.live_jobs.try_start() {
            Ok(live_job) => live_job,
            Err(busy) => return live_jobs_busy(busy),
        };
        let deadline = Instant::now() + DRAFT_FETCH_DEADLINE;
        let pages: Vec<CrawlerReviewPage> = detail
            .pages
            .into_iter()
            .filter(|page| !page.is_marked_not_product())
            .collect();
        let fetched = match tokio::time::timeout_at(deadline, self.fetch_review_pages(pages)).await
        {
            Ok(Ok(fetched)) => fetched,
            Ok(Err(response)) => return response,
            Err(_) => return draft_timeout(review_id),
        };
        let html: Vec<String> = fetched.into_iter().map(|(_, html)| html).collect();
        let drafted = tokio::task::spawn_blocking(move || {
            let draft = draft_schema(html.iter().map(String::as_str));
            let sold_shown = draft.as_ref().is_some_and(|draft| {
                draft
                    .schema
                    .state
                    .as_ref()
                    .is_some_and(|state| html.iter().any(|page| shows_sold(state, page)))
            });
            (draft, sold_shown)
        })
        .await;
        let (draft, sold_shown) = match drafted {
            Ok(drafted) => drafted,
            Err(error) => {
                return HttpResponse::json(
                    500,
                    &serde_json::json!({ "error": format!("the draft failed: {error}") }),
                );
            }
        };
        let Some(draft) = draft else {
            return answer(review_id, NotDrafted::NoTemplateAnchor);
        };
        let state = draft.schema.state.clone();
        let candidate_version = match self
            .repository
            .stage_structured_draft(review_id, actor, draft.schema.clone())
            .await
        {
            Ok(Some(version)) => version,
            Ok(None) => return answer(review_id, NotDrafted::NoStarterSchema),
            Err(error) => return review_error(error),
        };
        let (sold_page_added, checked) = match state {
            Some(state) if !sold_shown => {
                self.find_sold_page(review_id, actor, &state, deadline)
                    .await
            }
            _ => (None, 0),
        };
        drop(live_job);
        log_draft(review_id, &draft, sold_page_added.as_deref(), checked);
        HttpResponse::json(
            200,
            &DraftAnswer {
                review_id,
                drafted: true,
                reason: None,
                candidate_version: Some(candidate_version),
                complete: draft.is_complete(),
                undrafted: draft.undrafted,
                sold_page_added,
                sold_page_candidates_checked: checked,
            },
        )
    }

    /// Fetches up to [`MAX_SOLD_PAGE_CANDIDATES`] product URLs of the review's ListingSource,
    /// those that last failed to scrape first, until `state` reads a sold or unavailable product on
    /// one, which joins the review as a page. Each fetched page counts as inspected. Answers the
    /// added page's URL and how many URLs were fetched.
    async fn find_sold_page(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
        state: &ExtractionRule,
        deadline: Instant,
    ) -> (Option<String>, usize) {
        let candidates = match self.source_product_urls.list(review_id).await {
            Ok(urls) => urls,
            Err(error) => {
                warn!(review_id = %review_id, error = %error, "Cannot list the ListingSource's product URLs to find a sold page for the draft");
                return (None, 0);
            }
        };
        let mut checked = 0;
        for candidate in candidates.iter().rev().take(MAX_SOLD_PAGE_CANDIDATES) {
            let Ok(url) = Url::parse(&candidate.url) else {
                continue;
            };
            let Ok(Ok(fetched)) = tokio::time::timeout_at(deadline, self.fetch_live(&url)).await
            else {
                if Instant::now() >= deadline {
                    break;
                }
                continue;
            };
            checked += 1;
            let html = fetched.html;
            let rule = state.clone();
            let page = tokio::task::spawn_blocking(move || {
                let sold = shows_sold(&rule, &html);
                let product = describes_one_product(&html);
                (html, sold, product)
            })
            .await;
            let Ok((html, sold, product)) = page else {
                continue;
            };
            if let Err(error) = self
                .repository
                .record_inspected_page(review_id, &url, product)
                .await
            {
                warn!(review_id = %review_id, url = %url, error = %error, "Cannot record a page the draft inspected");
            }
            if !sold {
                continue;
            }
            match self
                .repository
                .add_reviewer_page(review_id, actor, &url, &fetched.final_url, &html)
                .await
            {
                Ok(page) => return (Some(page.url), checked),
                Err(error) => {
                    warn!(review_id = %review_id, url = %url, error = %error, "Cannot add the sold page the draft found");
                }
            }
        }
        (None, checked)
    }
}

fn answer(review_id: CrawlerReviewId, reason: NotDrafted) -> HttpResponse {
    HttpResponse::json(
        200,
        &DraftAnswer {
            review_id,
            drafted: false,
            reason: Some(reason),
            candidate_version: None,
            undrafted: Vec::new(),
            complete: false,
            sold_page_added: None,
            sold_page_candidates_checked: 0,
        },
    )
}

fn draft_timeout(review_id: CrawlerReviewId) -> HttpResponse {
    HttpResponse::json(
        504,
        &serde_json::json!({
            "error": "the review pages could not be fetched within the draft's deadline; the shop's Crawl-delay may be queueing them",
            "reason": "TIMEOUT",
            "review_id": review_id,
        }),
    )
}

fn log_draft(
    review_id: CrawlerReviewId,
    draft: &StructuredDraft,
    sold_page_added: Option<&str>,
    checked: usize,
) {
    info!(
        event = "crawler.review.structured_draft_staged",
        review_id = %review_id,
        complete = draft.is_complete(),
        undrafted = ?draft.undrafted,
        sold_page_added,
        sold_page_candidates_checked = checked,
        "Staged a structured-data draft in place of the starter schema"
    );
}
