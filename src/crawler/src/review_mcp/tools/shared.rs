//! The tools both agents have: reading the queue, a review and a page, refreshing a review's live
//! matrix, and the needs-repair verdict.

use super::{
    Artifact, Effect, ToolContext, ToolDefinition, ToolSchemaError, candidate_of, review_detail,
    review_path, review_source_crawl_host, review_source_url, settled,
};
use crate::review::model::PendingReviewPage;
use crate::review::page_projection::PageView;
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
use std::sync::atomic::{AtomicUsize, Ordering};

/// The most `inspect_page` calls one `review-mcp` process, one agent process, sends to the
/// console, in either view and offset reads included: every page an agent reads stays in its
/// context, so it chooses the pages it reads.
pub(super) const MAX_INSPECTIONS_PER_PROCESS: usize = 12;

/// The `inspect_page` calls a process has made, at most [`MAX_INSPECTIONS_PER_PROCESS`].
#[derive(Debug, Default)]
pub(super) struct InspectionBudget(AtomicUsize);

impl InspectionBudget {
    /// Takes one call of the budget, or refuses when the process made them all.
    pub(super) fn take(&self) -> Result<(), ToolError> {
        self.0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |made| {
                (made < MAX_INSPECTIONS_PER_PROCESS).then_some(made + 1)
            })
            .map(|_| ())
            .map_err(|_| ToolError::InspectLimitReached(MAX_INSPECTIONS_PER_PROCESS))
    }
}

const LIST_PENDING_REVIEWS: &str = "List the reviews you can work in the crawler's review console: the pending reviews of crawl-enabled ListingSources that do not wait for a human, oldest first, at most 50 per answer. Each entry gives the review_id the other tools take, the artifact type (PRODUCT_SCHEMA: the CSS schemas that extract products from a shop's product pages; URL_PATTERN: the regex that tells one ListingSource's product URLs apart), why the crawler opened it, its candidate_version, the needs-repair note it waits on, if any, and agent_rounds (the agents' needs-repair verdicts on it since a human last changed or judged it). total_pending counts all of them, on every page together. When more is true, or truncations names the reviews list, call the tool again with after set to the review_id of the last review listed to read on; an after that names no review answers NOT_FOUND, so start again without it. waiting_for_human counts the pending reviews that wait for a human, which are not listed: the agents' back-and-forth on them reached the cap or an agent handed them over, and every change, verdict and page fetch of yours on such a review (refresh_matrix, evaluate_candidate and inspect_page among them) is refused with HUMAN_REQUIRED until a human changes or judges it, so leave them. Start here, then read the review you work on with get_review. Reads only.";

const GET_REVIEW: &str = "Read one review: its status, its candidate (the product CSS schemas and removed-page templates, or the URL-pattern decision with its sample labels), candidate_version, the review pages a product-schema candidate is evaluated on, source_product_urls (a product-schema review only: up to 40 product URLs of the ListingSource that are not review pages, each with last_scraped and last_error_kind, those that scraped without an error first, most recent first; they are there for picking live, sold, price-on-application or photo-less pages to test with evaluate_candidate or to add with add_review_page), the URL sample of a URL-pattern review with how the candidate classifies each URL (candidate_pattern_match and candidate_class; the sample holds up to 1,000 of the ListingSource's URLs: at least 200 when the review opens unless the crawl completed with fewer, and, refreshed at most once a minute, ones it stored while the review waits; each URL is listed as its path and ?query without scheme and host, the string a pattern is matched against, and the sample in a fixed pseudorandom order by a hash of the URL, so the first page is a shuffle of the whole sample rather than the crawl's first URLs; the agents see only the sample's first 200 URLs in that order, the seed a pattern is induced from, and never the rest, which the approval guard still checks a pattern against; sample_urls_total is the whole sample and sample_urls_next_offset, when not null (only when a seed of long URLs passes the answer's byte bound), is the sample_offset that reads the rest of the seed; the sample can grow while the review waits, and a URL it gains can enter the seed; an answer with a sample_offset above 0 carries only that page of the sample with the review's status and candidate_version, not the candidate, pages, source URLs, matrix and events the first answer gave, which stay valid while candidate_version is the same, so read the review again without sample_offset when it changed), the schema matrix the console stored for the current candidate with not_evaluated_pages (null after any candidate change; refresh_matrix evaluates a new one), the latest 10 events of the review's trail with their actors and notes cut to 400 characters (the needs-repair request you repair comes whole as repair_request), agent_rounds (as list_pending_reviews names them) and human_required (leave a review with human_required true to a human). Each edit and approval names the candidate_version it is based on; the tools that change the candidate answer the new one, so name that and read the review again only when a call is refused with CONFLICT, which carries the current candidate_version. Reads only; fetches nothing from the shop.";

const INSPECT_PAGE: &str = "Fetch one page live and read its outline: one element per line in CSS notation, indented one space per level, with its text after ` | `, as in `span.price[itemprop='price'] | £120`; scripts, styles, navigation, header and footer are left out, long texts shortened. A line `… N more li.option with the same structure` counts siblings like the ones shown above it; siblings whose text holds a digit are always shown. Write selectors only from elements the outline shows. Two notations are not page content, so never put them in a selector: a srcset or data-srcset shows only its widest candidate followed by (+N more srcset candidates), so match it with a contains selector such as img[srcset*='/files/vase']; a data: URI shows only its media type, as in data:image/svg+xml,…, so select that element by its other attributes. Give review_id and either review_page_id (a page of the review) or url (an http(s) URL on the crawl host of the review's ListingSource, otherwise URL_OUTSIDE_REVIEW_SOURCE before anything is fetched). describes_one_product says whether the page's JSON-LD or microdata describes exactly one schema.org Product: a hint for labelling, never proof. json_ld_product_ids holds the productID and sku of the page's top-level JSON-LD Product, the values a json_ld_product rule reads; look at them, and at hidden inputs and data attributes, before leaving source_listing_id null. view MARKDOWN answers the page's readable content instead, a fraction of the outline's size: headings as #, paragraphs, list items, table rows as | a | b |, the text of links and buttons, image alt text as ![alt], whole texts and no tags, attributes or URLs; elements the page hides are left out. Use it to read content: to find sold or live pages, to check whether a page shows a price or a description, to decide whether a page is no product page (mark_page_not_product), or, as reviewer, to confirm an extracted value. Write selectors only from the default view OUTLINE, never from MARKDOWN. Either view counts as inspecting the page, for record_no_sold_page too. One agent process gets at most 12 inspect_page calls, in either view and offset reads included; later calls answer INSPECT_LIMIT_REACHED without fetching, so choose the pages you read and work with what you have read. An answer holds at most 24,000 characters of the view: projection_truncated and a marker line show where it stops, and offset with the same view reads on, fetching the page again. A URL-pattern review has no inspect_page for the agents: the console refuses it with AGENTS_CANNOT_INSPECT_URL_PATTERN_PAGES. Every answer stays in your context, so read on only when the part you need is not shown yet, and prefer refresh_matrix or evaluate_candidate to check what a selector extracts. The fetch is paced at the shop's crawl delay. Refused with CONFLICT and reason HUMAN_REQUIRED on a review with human_required true; RATE_LIMITED with reason LIVE_JOBS_BUSY means retry after retry_after_secs; CONSOLE_FAILED with status 502 and reason BOT_CHALLENGE means the shop did not serve the page, which says nothing about the candidate: retry later. Reads only.";

const REFRESH_MATRIX: &str = "Evaluate a product-schema review's current candidate live: the console fetches every review page at the shop's crawl delay, evaluates on them what approving the candidate would make live, stores this matrix for the review and returns it. Per schema and page: applies (the schema extracts a plausible product from the page), the extracted values (state_availability is what the normalization resolves the extracted state to, the only thing that counts for availability at runtime and for the sold-page check, null when it resolves to nothing), how each field's selectors matched (matches 0 on a required field fails the page; an optional field may miss on some pages, but a rule that matches nothing on any page is wrong), the plausibility findings, notices that block no approval (a price in another currency than the fallback currency), and which pages the removed-page templates match. not_evaluated_pages lists review pages the matrix did not evaluate. An approval of product schemas rests on the stored matrix of the candidate version it approves, fetched recently, so refresh after the last change. The tool waits while the console fetches, minutes for a slow shop, and answers STILL_RUNNING after 15 minutes; call it again to keep waiting. Only this tool fetches the pages: get_review shows the stored matrix and fetches nothing. A refresh is one of the console's live jobs, its matrix refreshes, candidate evaluations and page projections of all reviews together, of which it runs only a few at once: RATE_LIMITED with reason LIVE_JOBS_BUSY means retry after retry_after_secs. On a review with human_required true it is refused with CONFLICT and reason HUMAN_REQUIRED, also when the review comes to wait for a human while the pages are fetched. NOT_FOUND with reason NO_MATRIX means the matrix was dropped after it was stored, because the candidate or the review pages changed: read the review and call refresh_matrix again. A URL-pattern review has no matrix (INVALID_REQUEST). When the shop answers a page with a bot challenge instead of the page, the call fails with CONSOLE_FAILED, status 502 and reason BOT_CHALLENGE, and its error names the page: the page was not served, so it says nothing about the candidate; retry later and leave the candidate as it is. Changes no candidate.";

const CHECK_APPROVAL: &str = "Run the console's approval guard on a review's current candidate without approving it: failed_checks lists every check an agent approval would fail now, by its code (such as PATTERN_MATCHES_NO_SAMPLE_URL, PATTERN_MATCHES_EVERY_SAMPLE_URL, PATTERN_TAKES_PROBE_PATHS or NO_PATTERN_NEEDS_HUMAN on a URL-pattern review, PAGES_NOT_COVERED, CONTENT_RULE_MISSING or NO_SOLD_PAGE_TESTED on a product-schema review) with what it names, the same answer an approval would get; an empty list means the guard would pass. A product-schema review is judged on its stored matrix, so refresh_matrix first: LIVE_MATRIX_MISSING, LIVE_MATRIX_OUTDATED and LIVE_MATRIX_EXPIRED say that the matrix is absent, for another candidate or too old, and are the only checks then worth reading. The four-eyes rule and the hourly cap of agent approvals are not part of the answer. Reads only; fetches nothing from the shop.";

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
pub(super) struct GetReviewArguments {
    /// The review's id, such as `cr_01...`, as list_pending_reviews names it.
    #[schemars(with = "String")]
    review_id: CrawlerReviewId,
    /// The position in the URL sample of a URL-pattern review to start the sample page at: 0 for
    /// the first page, which carries the whole review, then the `sample_urls_next_offset` of the
    /// answer before, which carries only that page of the sample.
    #[serde(default)]
    sample_offset: usize,
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
    /// An http(s) URL on the crawl host of the review's ListingSource. Give this or `review_page_id`.
    #[serde(default)]
    url: Option<String>,
    /// The character of the projection to start at, to read on where a truncated answer stopped,
    /// in the same view.
    #[serde(default)]
    offset: usize,
    /// `OUTLINE` (the default): the elements in CSS notation, to write selectors from;
    /// `MARKDOWN`: the readable content only, to read what the page shows.
    #[serde(default)]
    view: PageView,
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
        )?
        .only_for(Artifact::ProductSchema),
        ToolDefinition::new(
            "check_approval",
            CHECK_APPROVAL,
            Effect::Reads,
            check_approval,
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
    arguments: GetReviewArguments,
    context: Arc<ToolContext>,
) -> Result<ReviewView, ToolError> {
    let detail = review_detail(&context, arguments.review_id).await?;
    let candidate = candidate_of(&detail)?;
    Ok(ReviewView::of(detail, candidate, arguments.sample_offset))
}

async fn inspect_page(
    arguments: InspectPageArguments,
    context: Arc<ToolContext>,
) -> Result<PageProjectionView, ToolError> {
    if arguments.review_page_id.is_some() == arguments.url.is_some() {
        return Err(ToolError::InvalidArguments(
            "give either review_page_id or url".to_owned(),
        ));
    }
    // Counted before anything reaches the console, the crawl host's lookup included.
    context.inspections.take()?;
    page_projection(arguments, &context).await
}

/// Asks the console for the projection `arguments` name.
async fn page_projection(
    arguments: InspectPageArguments,
    context: &ToolContext,
) -> Result<PageProjectionView, ToolError> {
    let review_id = arguments.review_id;
    let query = match (arguments.review_page_id, arguments.url) {
        (Some(page_id), None) => format!("page={page_id}"),
        (None, Some(url)) => {
            let crawl_host = review_source_crawl_host(context, review_id).await?;
            let url = review_source_url(&url, crawl_host.as_deref())?;
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
            &format!(
                "/page-projection?{query}&view={}",
                arguments.view.query_name()
            ),
        ))
        .await?
        .decode()?;
    Ok(PageProjectionView::of(projection, arguments.offset))
}

/// The approval guard's dry run of a review's current candidate.
#[derive(Debug, serde::Serialize, Deserialize, JsonSchema)]
pub(super) struct ApprovalChecks {
    /// Every check an agent approval would fail now, each with its `check` code and what it names;
    /// empty when the guard would pass, null for a product-schema review.
    failed_checks: Option<Vec<serde_json::Value>>,
}

async fn check_approval(
    arguments: ReviewArguments,
    context: Arc<ToolContext>,
) -> Result<ApprovalChecks, ToolError> {
    Ok(context
        .console
        .get(&review_path(arguments.review_id, "/approval-checks"))
        .await?
        .decode()?)
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

    #[test]
    fn should_take_twelve_inspections_per_process_and_refuse_the_thirteenth() {
        let budget = InspectionBudget::default();

        for call in 0..MAX_INSPECTIONS_PER_PROCESS {
            assert!(budget.take().is_ok(), "call {call}");
        }

        for _ in 0..2 {
            assert!(matches!(
                budget.take(),
                Err(ToolError::InspectLimitReached(MAX_INSPECTIONS_PER_PROCESS))
            ));
        }
    }

    /// A process that made its twelve calls answers the thirteenth without a request: the console
    /// here would refuse every request as unreachable.
    #[tokio::test]
    async fn should_refuse_an_inspection_past_the_budget_without_contacting_the_console() {
        use crate::review_mcp::console::ConsoleClient;
        use crate::review_mcp::settings::AgentRole;
        use crate::review_mcp::token_file::RoleTokenFile;
        use crate::review_mcp::token_file::tests::TokenDirectory;

        let directory = TokenDirectory::new();
        directory.write_tokens("author", "reviewer");
        let unreachable = url::Url::parse("http://127.0.0.1:9").unwrap_or_else(|e| panic!("{e}"));
        let console = ConsoleClient::new(
            &unreachable,
            RoleTokenFile::new(directory.path(), AgentRole::Author),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let context = Arc::new(super::super::ToolContext::new(
            console,
            super::super::Polling::default(),
        ));
        let arguments = || {
            serde_json::from_value::<InspectPageArguments>(json!({
                "review_id": CrawlerReviewId::new().to_string(),
                "review_page_id": CrawlerReviewPageId::new().to_string(),
                "view": "MARKDOWN",
            }))
            .unwrap_or_else(|error| panic!("{error}"))
        };

        let reached = inspect_page(arguments(), context.clone()).await;
        assert!(matches!(reached, Err(ToolError::Console(_))), "{reached:?}");
        for _ in 1..MAX_INSPECTIONS_PER_PROCESS {
            context
                .inspections
                .take()
                .unwrap_or_else(|error| panic!("{error}"));
        }
        let refused = inspect_page(arguments(), context.clone()).await;
        let neither = serde_json::from_value::<InspectPageArguments>(json!({
            "review_id": CrawlerReviewId::new().to_string(),
        }))
        .unwrap_or_else(|error| panic!("{error}"));

        assert!(
            matches!(
                refused,
                Err(ToolError::InspectLimitReached(MAX_INSPECTIONS_PER_PROCESS))
            ),
            "{refused:?}"
        );
        assert!(matches!(
            inspect_page(neither, context).await,
            Err(ToolError::InvalidArguments(_))
        ));
    }

    #[test]
    fn should_inspect_the_outline_unless_the_markdown_view_is_named() {
        let review_id = CrawlerReviewId::new().to_string();
        let view = |arguments: serde_json::Value| {
            serde_json::from_value::<InspectPageArguments>(arguments)
                .map(|arguments| arguments.view)
                .ok()
        };

        assert_eq!(
            view(json!({ "review_id": review_id, "url": "https://antiques.example/a" })),
            Some(PageView::Outline)
        );
        assert_eq!(
            view(
                json!({ "review_id": review_id, "url": "https://antiques.example/a", "view": "OUTLINE" })
            ),
            Some(PageView::Outline)
        );
        assert_eq!(
            view(
                json!({ "review_id": review_id, "url": "https://antiques.example/a", "view": "MARKDOWN", "offset": 24000 })
            ),
            Some(PageView::Markdown)
        );
        for refused in ["markdown", "TEXT", ""] {
            assert_eq!(
                view(
                    json!({ "review_id": review_id, "url": "https://antiques.example/a", "view": refused })
                ),
                None,
                "{refused}"
            );
        }
    }
}
