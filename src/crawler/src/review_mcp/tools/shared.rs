//! The tools both agents have: reading the queue, a review and a page, refreshing a review's live
//! matrix, and the needs-repair verdict.

use super::{
    Effect, ToolContext, ToolDefinition, ToolSchemaError, candidate_of, review_detail, review_path,
    review_source_domains, review_source_url, settled,
};
use crate::review::model::PendingReviewPage;
use crate::review_mcp::errors::ToolError;
use crate::review_mcp::views::{
    MAX_PENDING_REVIEWS, MatrixAnswer, MatrixView, PageProjectionView, PendingReviews, ReviewView,
    VerdictTaken,
};
use crate::review_mcp::wire;
use crate::{CrawlerReviewId, CrawlerReviewPageId};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

const LIST_PENDING_REVIEWS: &str = "List the reviews you can work in the crawler's review console: the pending reviews of crawl-enabled ListingSources that do not wait for a human, oldest first, at most 50 per answer. Each entry gives the review_id the other tools take, the artifact type (PRODUCT_SCHEMA: the CSS schemas that extract products from a shop's product pages; URL_PATTERN: the regex that tells one domain's product URLs apart), why the crawler opened it, its candidate_version, the needs-repair note it waits on, if any, and agent_rounds (the agents' needs-repair verdicts on it since a human last changed or judged it). total_pending counts all of them, on every page together. When more is true, or truncations names the reviews list, call the tool again with after set to the review_id of the last review listed to read on; an after that names no review answers NOT_FOUND, so start again without it. waiting_for_human counts the pending reviews that wait for a human, which are not listed: the agents' back-and-forth on them reached the cap, and every change, verdict and page fetch of yours on such a review (refresh_matrix, evaluate_candidate and inspect_page among them) is refused with HUMAN_REQUIRED until a human changes or judges it, so leave them. Start here, then read the review you work on with get_review. Reads only.";

const GET_REVIEW: &str = "Read one review: its status, its candidate (the product CSS schemas and removed-page templates, or the URL-pattern decision with its sample labels), candidate_version, the review pages a product-schema candidate is evaluated on, the URL sample of a URL-pattern review with how the candidate classifies each URL, the schema matrix the console stored for the current candidate with not_evaluated_pages (null after any candidate change; refresh_matrix evaluates a new one), the latest events of the review's trail with their actors, agent_rounds (as list_pending_reviews names them) and human_required (leave a review with human_required true to a human). Read the review again after every change, yours or anyone else's: each edit and approval names the candidate_version it is based on, and one based on an older version is refused with CONFLICT and the current candidate_version. Reads only; fetches nothing from the shop.";

const INSPECT_PAGE: &str = "Fetch one page live and read it as the YAML projection the crawler's schema generation uses: the page's elements with their tag, attributes (class, id, itemprop, data-*, href, src and others) and text, without scripts, styles, navigation, header and footer, long texts shortened and at most 2,000 elements. Write selectors only from elements the projection shows. Give review_id and either review_page_id (a page of the review from get_review) or url (an http(s) URL on a domain of the review's ListingSource; any other URL is refused with URL_OUTSIDE_REVIEW_SOURCE before anything is fetched). describes_one_product says whether the page's JSON-LD or microdata describes exactly one schema.org Product: a hint when you label URLs, never proof, and the projection does not show that markup. An answer holds at most 36,000 characters of the projection, fewer for text in non-Latin scripts: projection_truncated and a marker line show where it stops, and offset reads on from there, fetching the page again. The fetch is paced at the shop's crawl delay and can take up to half a minute. On a review with human_required true it is refused with CONFLICT and reason HUMAN_REQUIRED before anything is fetched. Each projection is one of the console's live jobs, its matrix refreshes, candidate evaluations and page projections of all reviews together, of which it runs only a few at once: RATE_LIMITED with reason LIVE_JOBS_BUSY means retry after retry_after_secs. Reads only.";

const REFRESH_MATRIX: &str = "Evaluate a product-schema review's current candidate live: the console fetches every review page at the shop's crawl delay, evaluates on them what approving the candidate would make live, stores this matrix for the review and returns it. Per schema and page: applies (the schema extracts a plausible product from the page), the extracted values, how each field's selectors matched (matches 0 on a required field fails the page; an optional field may miss on some pages, but a rule that matches nothing on any page is wrong), the plausibility findings, and which pages the removed-page templates match. not_evaluated_pages lists review pages the matrix did not evaluate. An approval of product schemas rests on the stored matrix of the candidate version it approves, fetched recently, so refresh after the last change. The tool waits while the console fetches, minutes for a slow shop, and answers STILL_RUNNING after 15 minutes; call it again to keep waiting. Only this tool fetches the pages: get_review shows the stored matrix and fetches nothing. A refresh is one of the console's live jobs, its matrix refreshes, candidate evaluations and page projections of all reviews together, of which it runs only a few at once: RATE_LIMITED with reason LIVE_JOBS_BUSY means retry after retry_after_secs. On a review with human_required true it is refused with CONFLICT and reason HUMAN_REQUIRED, also when the review comes to wait for a human while the pages are fetched. NOT_FOUND with reason NO_MATRIX means the matrix was dropped after it was stored, because the candidate or the review pages changed: read the review and call refresh_matrix again. A URL-pattern review has no matrix (INVALID_REQUEST). Changes no candidate.";

const MARK_NEEDS_REPAIR: &str = "Give the review the needs-repair verdict: it stays pending, and your notes become its repair request, which tells the author agent or a human what to fix. Use it when the candidate is wrong but can be fixed. notes are required and must be concrete: name the page URL, the schema index or field, and what is wrong with it. Each needs-repair verdict of an agent is one round of the agents' back-and-forth on the review: the verdict that reaches the cap is taken, and from then on the review waits for a human (human_required true), so this and every other change or verdict of yours on it is refused with CONFLICT and reason HUMAN_REQUIRED. Changes no candidate; refused for a review that is no longer pending.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ListArguments {
    /// The review_id of the last review a previous answer listed, to list the reviews after it.
    /// Leave it out to start with the oldest.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    after: Option<CrawlerReviewId>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ReviewArguments {
    /// The review's id, such as `cr_01...`, as list_pending_reviews names it.
    #[schemars(with = "String")]
    pub(super) review_id: CrawlerReviewId,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct InspectPageArguments {
    /// The review's id, such as `cr_01...`, as list_pending_reviews names it.
    #[schemars(with = "String")]
    review_id: CrawlerReviewId,
    /// A page of the review, as get_review names it. Give this or `url`.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    review_page_id: Option<CrawlerReviewPageId>,
    /// An http(s) URL on a domain of the review's ListingSource. Give this or `review_page_id`.
    #[serde(default)]
    url: Option<String>,
    /// The character of the projection to start at, to read on where a truncated answer stopped.
    #[serde(default)]
    offset: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct NotesArguments {
    /// The review's id, such as `cr_01...`, as list_pending_reviews names it.
    #[schemars(with = "String")]
    review_id: CrawlerReviewId,
    /// What is wrong and where: the page URL, the schema index or field, and why.
    notes: String,
}

pub(super) fn tools() -> Result<Vec<ToolDefinition>, ToolSchemaError> {
    Ok(vec![
        ToolDefinition::new(
            "list_pending_reviews",
            LIST_PENDING_REVIEWS,
            Effect::Reads,
            list_pending_reviews,
        )?,
        ToolDefinition::new("get_review", GET_REVIEW, Effect::Reads, get_review)?,
        ToolDefinition::new("inspect_page", INSPECT_PAGE, Effect::Reads, inspect_page)?,
        ToolDefinition::new(
            "refresh_matrix",
            REFRESH_MATRIX,
            Effect::ChangesIdempotently,
            refresh_matrix,
        )?,
        ToolDefinition::new(
            "mark_needs_repair",
            MARK_NEEDS_REPAIR,
            Effect::Changes,
            mark_needs_repair,
        )?,
    ])
}

async fn list_pending_reviews(
    arguments: ListArguments,
    context: Arc<ToolContext>,
) -> Result<PendingReviews, ToolError> {
    let mut path = format!("/api/pending-reviews?human_required=false&limit={MAX_PENDING_REVIEWS}");
    if let Some(after) = arguments.after {
        path.push_str(&format!("&after={after}"));
    }
    let page: PendingReviewPage = context.console.get(&path).await?.decode()?;
    Ok(PendingReviews::of(page))
}

async fn get_review(
    arguments: ReviewArguments,
    context: Arc<ToolContext>,
) -> Result<ReviewView, ToolError> {
    let detail = review_detail(&context, arguments.review_id).await?;
    let candidate = candidate_of(&detail)?;
    Ok(ReviewView::of(detail, candidate))
}

async fn inspect_page(
    arguments: InspectPageArguments,
    context: Arc<ToolContext>,
) -> Result<PageProjectionView, ToolError> {
    let review_id = arguments.review_id;
    let query = match (arguments.review_page_id, arguments.url) {
        (Some(page_id), None) => format!("page={page_id}"),
        (None, Some(url)) => {
            let domains = review_source_domains(&context, review_id).await?;
            let url = review_source_url(&url, &domains)?;
            format!("url={}", encoded(url.as_str()))
        }
        _ => {
            return Err(ToolError::InvalidArguments(
                "give either review_page_id or url".to_owned(),
            ));
        }
    };
    let projection: wire::PageProjection = context
        .console
        .get(&review_path(
            review_id,
            &format!("/page-projection?{query}"),
        ))
        .await?
        .decode()?;
    Ok(PageProjectionView::of(projection, arguments.offset))
}

async fn refresh_matrix(
    arguments: ReviewArguments,
    context: Arc<ToolContext>,
) -> Result<MatrixAnswer, ToolError> {
    let review_id = arguments.review_id;
    let answer = context
        .console
        .get(&review_path(review_id, "/matrix?refresh=true"))
        .await?;
    let answer = settled(
        &context,
        answer,
        &review_path(review_id, "/matrix"),
        "matrix refresh",
    )
    .await?;
    Ok(MatrixAnswer {
        matrix: MatrixView::of(answer.decode()?),
        truncations: Vec::new(),
    })
}

async fn mark_needs_repair(
    arguments: NotesArguments,
    context: Arc<ToolContext>,
) -> Result<VerdictTaken, ToolError> {
    context
        .console
        .post(
            &review_path(arguments.review_id, "/needs-repair"),
            &json!({ "notes": arguments.notes }),
        )
        .await?;
    Ok(VerdictTaken {
        review_id: arguments.review_id.to_string(),
        verdict: "NEEDS_REPAIR".to_owned(),
    })
}

pub(super) fn encoded(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_read_on_after_a_review_id_and_refuse_any_other_argument() {
        let after = CrawlerReviewId::new();

        let decoded: Result<ListArguments, _> =
            serde_json::from_value(json!({ "after": after.to_string() }));
        assert_eq!(
            decoded.ok().and_then(|arguments| arguments.after),
            Some(after)
        );
        assert!(
            serde_json::from_value::<ListArguments>(json!({}))
                .is_ok_and(|arguments| arguments.after.is_none())
        );
        for arguments in [
            json!({ "after": after.as_uuid().to_string() }),
            json!({ "offset": 50 }),
        ] {
            assert!(
                serde_json::from_value::<ListArguments>(arguments.clone()).is_err(),
                "{arguments}"
            );
        }
    }

    #[test]
    fn should_decode_review_ids_through_their_type_and_refuse_unknown_arguments() {
        let review_id = CrawlerReviewId::new();

        let decoded: Result<ReviewArguments, _> =
            serde_json::from_value(json!({ "review_id": review_id.to_string() }));
        assert_eq!(
            decoded.ok().map(|arguments| arguments.review_id),
            Some(review_id)
        );

        for arguments in [
            json!({}),
            json!({ "review_id": review_id.as_uuid().to_string() }),
            json!({ "review_id": CrawlerReviewPageId::new().to_string() }),
            json!({ "review_id": review_id.to_string(), "token": "x" }),
        ] {
            assert!(
                serde_json::from_value::<ReviewArguments>(arguments.clone()).is_err(),
                "{arguments}"
            );
        }
    }
}
