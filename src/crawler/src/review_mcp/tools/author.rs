//! The author agent's tools: evaluating, staging and labelling candidates and the review's pages.

use super::shared::{ApprovalChecks, approval_checks, encoded, refreshed_matrix};
use super::{
    Artifact, Effect, ToolContext, ToolDefinition, ToolSchemaError, candidate_of, review_detail,
    review_path, review_source_crawl_host, review_source_url, review_source_urls, settled,
};
use crate::review::model::{CrawlerReviewPage, SourceLabel};
use crate::review_mcp::errors::ToolError;
use crate::review_mcp::views::{
    CandidateChanged, MatrixAnswer, MatrixView, NoSoldPageRecorded, PageAdded,
    PageMarkedNotProduct, PageRemoved, SourceLabelled,
};
use crate::review_mcp::wire;
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crate::scraper::css_selector::rule::ExtractionRule;
use crate::spider::classification::product_url_pattern::UrlPatternMatching;
use crate::{CrawlerReviewId, CrawlerReviewPageId};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::sync::Arc;

const EVALUATE_CANDIDATE: &str = "Test a product-schema candidate before you stage it, without storing anything; schemas and removed_page_schemas take the shape update_candidate documents: the console fetches the review's pages and up to four further `urls` of the review's ListingSource live, at the shop's crawl delay, and evaluates on them what approving this candidate would make live. The answer has the matrix refresh_matrix answers; pages from `urls` are named by input_index, their position in `urls`. Use it to test every candidate on sold, price-on-application and photo-less pages of the shop before update_candidate. schemas is the whole schema list; removed_page_schemas, when given, replaces the candidate's removed-page templates, otherwise the stored ones are evaluated. The candidate is checked like an edit (INVALID_REQUEST names what is wrong), a URL off the review's ListingSource is refused with URL_OUTSIDE_REVIEW_SOURCE, and on a review with human_required true the evaluation is refused with CONFLICT and reason HUMAN_REQUIRED before anything is fetched. An evaluation is one of the console's live jobs, its matrix refreshes, candidate evaluations and page projections of all reviews together, of which it runs only a few at once: RATE_LIMITED with reason LIVE_JOBS_BUSY means retry after retry_after_secs. Waits while the console fetches and answers STILL_RUNNING after 15 minutes. When the shop answers a page with a bot challenge instead of the page, the call fails with CONSOLE_FAILED, status 502 and reason BOT_CHALLENGE, and its error names the page: the page was not served, so it says nothing about the candidate; retry later and leave the candidate as it is.";

const UPDATE_CANDIDATE: &str = "Stage a whole candidate on a pending review, replacing the current one. For a PRODUCT_SCHEMA review give kind PRODUCT_SCHEMA with the whole `schemas` list (add, change, reorder or drop schemas here; every core field rule needs selector and type, raw attributes only with keys of the raw attribute registry) and optionally `removed_page_schemas`, which replaces the removed-page templates (left out, they stay; an empty list retires them all). For a URL_PATTERN review give kind URL_PATTERN with the decision: type PATTERN with a regex `value` that matches the product URLs of the review's sample and no other URL (with the default `matching` PATH_AND_QUERY_FULL_MATCH the spider matches it against the whole of the URL's path and `?query`, without scheme, host, port or fragment, with or without the URL's trailing slash, as if anchored by ^ and $; a value naming a scheme or host is refused; FULL_URL_SEARCH matches it anywhere in the whole URL instead), and optionally a regex `exclude` for the few non-items that share the product URL shape, such as gift cards, vouchers, services or the dealer's own profile page: a URL is a product URL when it matches `value` and does not match `exclude`, which is matched anywhere in the path and query, or in the whole URL under FULL_URL_SEARCH (the regex engine has no lookahead, so exclude them here instead of writing slug-prefix tricks into `value`; `exclude` must not match a product URL), or type NO_PATTERN for a ListingSource without product pages, which only a human approves (NO_PATTERN_NEEDS_HUMAN). An agent approval of a pattern needs one that takes at least one sample URL (PATTERN_MATCHES_NO_SAMPLE_URL) and, on a sample of two or more, not every one (PATTERN_MATCHES_EVERY_SAMPLE_URL), and that takes none of the made-up paths the guard tries on the crawl host: the root /, /aura-historia-probe, /aura-historia-probe/aura-historia-probe and /aura-historia-probe?aura-historia=probe (PATTERN_TAKES_PROBE_PATHS), so a value that constrains no more than a URL's shape, such as /.+ or one path segment, is refused: anchor it on what only the product URLs share, such as a path prefix, an id or a file extension; it reads no sample label. The edit keeps the sample labels the review has. expected_candidate_version is the candidate_version you read; when the review changed since, the edit is refused with CONFLICT and the current candidate_version: read the review again and redo the edit on its current candidate. Answers the new candidate_version. Any change drops the stored matrix: refresh_matrix afterwards.";

const UPDATE_SCHEMA_FIELD: &str = "Change one rule of one schema of a pending product-schema review's candidate; a rule takes the shape update_candidate documents. schema_index is the schema's position in the candidate's schemas; field is a core field (source_listing_id, title, description, price, price_estimate_min, price_estimate_max, state, images) or `raw_attributes.<key>` with a key of the raw attribute registry; rule is the whole new rule, or null to delete an optional rule (title, state and images cannot be deleted). expected_candidate_version is the candidate_version you read; a stale one is refused with CONFLICT and the current candidate_version. Answers the new candidate_version. The change drops the stored matrix: refresh_matrix afterwards.";

const ADD_REVIEW_PAGE: &str = "Add a page of the review's ListingSource to a pending product-schema review, so every later matrix evaluates the candidate on it too: use it for sold, price-on-application or photo-less product pages the candidate must cover. url must be an http(s) URL on the crawl host of the review's ListingSource (otherwise URL_OUTSIDE_REVIEW_SOURCE, before anything is fetched). The console fetches the page now; a redirect to a page that is no product page, a page already on the review, and more than four added pages are refused with INVALID_REQUEST or CONFLICT. A page the shop answers with a bot challenge is not added: the call fails with CONSOLE_FAILED, status 502 and reason BOT_CHALLENGE; retry later. Answers the new review_page_id. Drops the stored matrix: refresh_matrix afterwards.";

const REMOVE_REVIEW_PAGE: &str = "Remove a page that was added to the review, or that waits on it, from a pending product-schema review, for example once the shop deleted the page and every matrix refresh fails on it. Seed and triggering pages cannot be removed (INVALID_REQUEST). review_page_id must be a page of the given review. Drops the stored matrix: refresh_matrix afterwards.";

const MARK_PAGE_NOT_PRODUCT: &str = "Mark a review page of a pending product-schema review as no product page: the dealer's profile, a category or article page, a gift card, anything the URL pattern takes that is no product of the catalogue. Mark it instead of bending the schema to extract it or guessing at the page; the reviewer judges your reason and gives NEEDS_REPAIR when the page is a product after all. reason, 1 to 500 characters, says what the page shows. A marked page is left out of the matrix and of the review's coverage, so no schema has to resolve it, and once the review is approved its URL stops being scraped and stays a non-product page. review_page_id must be a page of the given review, a seed or triggering page included. Drops the stored matrix: refresh_matrix afterwards. A review with human_required true is refused with CONFLICT and reason HUMAN_REQUIRED.";

const DECLARE_REMOVED_PAGE: &str = "Declare a kind of review page removed instead of authoring a schema for it: for a sold or withdrawn item the shop still serves with HTTP 200, a removal notice and no product. Adds one removed-page template to the pending product-schema candidate: selector selects the element holding the notice, and exactly one of text (the notice's text) or regex (a regex the notice's text matches; one that matches any text is refused). Declare a page removed only where the page itself shows a removal notice. The matrix then lists the pages the template matches and whether a schema still extracts a product from them (PRODUCT_STILL_EXTRACTS, a template too broad for that page). expected_candidate_version is the candidate_version you read; a stale one is refused with CONFLICT. Answers the new candidate_version; refresh_matrix afterwards.";

const RECORD_NO_SOLD_PAGE: &str = "Record that you looked for a sold or unavailable product page of the review's ListingSource and found none, so the console accepts a candidate tested on none. An approval needs a review page whose extracted state is sold, out of stock or unavailable (or that a removed-page template captures), or this record. Use it only after you searched: probe your state rule, and the elements where a sold label would sit, on the shop's product pages with probe_selectors, four pages per call, and open its listing pages, its sold or archive section and filters with inspect_page; when you find a sold or unavailable page, add it with add_review_page instead. urls are 8 to 50 distinct pages of the review's ListingSource you opened with inspect_page or probed with probe_selectors on this review since a human last changed or judged it (a URL off its crawl host is refused with URL_OUTSIDE_REVIEW_SOURCE; fewer or more with INVALID_REQUEST; a page not inspected so with CONFLICT and reason PAGES_NOT_INSPECTED, whose error names those pages: inspect them, or leave them out), notes say how you searched and what you saw, in a few sentences. The record changes no candidate and does not move candidate_version. A review agent's approval is judged on it as the trail shows it until a human changes or judges the review; after that, search and record again.";

const LABEL_SOURCE: &str = "Label the review's ListingSource when the crawler should not crawl the shop at all, instead of authoring a candidate for it. NOT_AN_ESHOP: the site shows items but sells nothing online, with no price, cart or buy button and only enquiries, such as a gallery or a dealer's catalogue. APPROACH_NOT_APPLICABLE: the shop sells online but the crawler's approach does not fit it, such as items without a page of their own. BOT_PROTECTION: the shop answers bot challenges instead of its pages, again and again. JAVASCRIPT_RENDERED: the product pages carry no product content without running JavaScript. LOGIN_REQUIRED: the products are behind a login. OTHER: none of these; the note names the reason. Label only on evidence you read for this review, never on a single failed fetch, and never because a candidate is hard to write. note is required, 1 to 500 characters: what you read and why the shop cannot be crawled, naming the URLs. The label switches crawling of the whole ListingSource off at once and takes its reviews out of the agents' queue; the review stays pending, and only a human clears the label. Refused with CONFLICT and reason HUMAN_REQUIRED on a review with human_required true. After it, change nothing more and report SOURCE_LABELLED.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct EvaluateCandidateArguments {
    /// The review's id, such as `cr_01...`, as list_pending_reviews names it.
    #[schemars(with = "String")]
    review_id: CrawlerReviewId,
    /// The whole schema list to evaluate.
    schemas: Vec<ProductCssSelectorSchema>,
    /// The removed-page templates to evaluate instead of the candidate's.
    #[serde(default)]
    removed_page_schemas: Option<Vec<RemovedPageSchema>>,
    /// Up to four further URLs of the review's ListingSource to evaluate on.
    #[serde(default)]
    urls: Vec<String>,
}

const STAGE_AND_CHECK: &str = "Stage a product-schema candidate, refresh its matrix and run the approval guard in one call: what update_candidate, refresh_matrix and check_approval answer one after the other, in one turn. With `schemas` it stages that whole schema list (and `removed_page_schemas` when given) as update_candidate does, naming expected_candidate_version; without `schemas` it stages nothing and refreshes and checks the current candidate, as after add_review_page, mark_page_not_product or record_no_sold_page. Answers candidate_version, the matrix refresh_matrix answers and the guard's failed_checks: empty when an approval would pass. Errors as those three tools answer them; on CONFLICT nothing was staged.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct StageAndCheckArguments {
    /// The review's id, such as `cr_01...`, as list_pending_reviews names it.
    #[schemars(with = "String")]
    review_id: CrawlerReviewId,
    /// The candidate_version you read; required with `schemas`.
    #[serde(default)]
    expected_candidate_version: Option<i64>,
    /// The whole schema list to stage; left out, the current candidate is checked.
    #[serde(default)]
    schemas: Option<Vec<ProductCssSelectorSchema>>,
    /// The removed-page templates that replace the candidate's; left out, they stay.
    #[serde(default)]
    removed_page_schemas: Option<Vec<RemovedPageSchema>>,
}

/// A candidate staged, its matrix refreshed and the approval guard run on it.
#[derive(Debug, serde::Serialize, JsonSchema)]
pub(super) struct StagedAndChecked {
    review_id: String,
    /// The candidate's version after the call; name it in the next edit.
    candidate_version: i64,
    matrix: MatrixView,
    #[serde(flatten)]
    approval: ApprovalChecks,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateCandidateArguments {
    /// The review's id, such as `cr_01...`, as list_pending_reviews names it.
    #[schemars(with = "String")]
    review_id: CrawlerReviewId,
    /// The candidate_version you read, as get_review answers it.
    expected_candidate_version: i64,
    candidate: CandidateArgument,
}

/// A whole candidate, of the review's artifact type.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub(super) enum CandidateArgument {
    ProductSchema {
        schemas: Vec<ProductCssSelectorSchema>,
        /// Replaces the removed-page templates; left out, they stay.
        #[serde(default)]
        removed_page_schemas: Option<Vec<RemovedPageSchema>>,
    },
    UrlPattern {
        decision: UrlPatternDecisionArgument,
    },
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub(super) enum UrlPatternDecisionArgument {
    /// A regex `value`: a URL whose path and query it matches as a whole is a product URL
    /// unless the optional regex `exclude` matches anywhere in them.
    Pattern {
        value: String,
        /// Takes the non-items that share the product URL shape (gift cards, vouchers, services,
        /// the dealer's own profile page) out of `value`'s matches; left out, none is taken out.
        #[serde(default)]
        exclude: Option<String>,
        /// What `value` and `exclude` are matched against; left out, the whole path and query.
        #[serde(default = "agent_url_pattern_matching")]
        matching: UrlPatternMatching,
    },
    /// The ListingSource has no product pages. A struct variant, so a stray key such as an
    /// `exclude` is refused instead of ignored.
    NoPattern {},
}

/// An agent's pattern is matched against the path and query unless it says otherwise.
fn agent_url_pattern_matching() -> UrlPatternMatching {
    UrlPatternMatching::PathAndQueryFullMatch
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct UpdateSchemaFieldArguments {
    /// The review's id, such as `cr_01...`, as list_pending_reviews names it.
    #[schemars(with = "String")]
    review_id: CrawlerReviewId,
    /// The candidate_version you read, as get_review answers it.
    expected_candidate_version: i64,
    schema_index: usize,
    /// A core field or `raw_attributes.<key>`.
    field: String,
    /// The new rule, or null to delete an optional rule.
    #[serde(deserialize_with = "Option::deserialize")]
    #[schemars(required)]
    rule: Option<ExtractionRule>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AddReviewPageArguments {
    /// The review's id, such as `cr_01...`, as list_pending_reviews names it.
    #[schemars(with = "String")]
    review_id: CrawlerReviewId,
    /// An http(s) URL on the crawl host of the review's ListingSource.
    url: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct RemoveReviewPageArguments {
    /// The review's id, such as `cr_01...`, as list_pending_reviews names it.
    #[schemars(with = "String")]
    review_id: CrawlerReviewId,
    /// A page of the review, as get_review names it.
    #[schemars(with = "String")]
    review_page_id: CrawlerReviewPageId,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct MarkPageNotProductArguments {
    /// The review's id, such as `cr_01...`, as list_pending_reviews names it.
    #[schemars(with = "String")]
    review_id: CrawlerReviewId,
    /// A page of the review, as get_review names it.
    #[schemars(with = "String")]
    review_page_id: CrawlerReviewPageId,
    /// What the page shows instead of a product, 1 to 500 characters.
    reason: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct DeclareRemovedPageArguments {
    /// The review's id, such as `cr_01...`, as list_pending_reviews names it.
    #[schemars(with = "String")]
    review_id: CrawlerReviewId,
    /// The candidate_version you read, as get_review answers it.
    expected_candidate_version: i64,
    /// A CSS selector of the element holding the removal notice.
    selector: String,
    /// The notice's text. Give this or `regex`.
    #[serde(default)]
    text: Option<String>,
    /// A regex the notice's text matches. Give this or `text`.
    #[serde(default)]
    regex: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct RecordNoSoldPageArguments {
    /// The review's id, such as `cr_01...`, as list_pending_reviews names it.
    #[schemars(with = "String")]
    review_id: CrawlerReviewId,
    /// The pages of the review's ListingSource you checked: 8 to 50 distinct URLs on its crawl host.
    urls: Vec<String>,
    /// How you searched and what you saw, in a few sentences.
    notes: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct LabelSourceArguments {
    /// The review's id, such as `cr_01...`, as list_pending_reviews names it.
    #[schemars(with = "String")]
    review_id: CrawlerReviewId,
    label: SourceLabel,
    /// Your evidence: what you read and why the shop cannot be crawled, naming the URLs.
    note: String,
}

pub(super) fn tools() -> Result<Vec<ToolDefinition>, ToolSchemaError> {
    Ok(vec![
        // update_candidate documents the schema rules; the tools taking the same rules carry
        // their structure only, since every definition is in the agent's context on every turn.
        ToolDefinition::new(
            "evaluate_candidate",
            EVALUATE_CANDIDATE,
            Effect::Reads,
            evaluate_candidate,
        )?
        .only_for(Artifact::ProductSchema)
        .without_nested_descriptions(),
        ToolDefinition::new(
            "update_candidate",
            UPDATE_CANDIDATE,
            Effect::Changes,
            update_candidate,
        )?,
        ToolDefinition::new(
            "stage_and_check",
            STAGE_AND_CHECK,
            Effect::Changes,
            stage_and_check,
        )?
        .only_for(Artifact::ProductSchema)
        .without_nested_descriptions(),
        ToolDefinition::new(
            "update_schema_field",
            UPDATE_SCHEMA_FIELD,
            Effect::Changes,
            update_schema_field,
        )?
        .only_for(Artifact::ProductSchema)
        .without_nested_descriptions(),
        ToolDefinition::new(
            "add_review_page",
            ADD_REVIEW_PAGE,
            Effect::Changes,
            add_review_page,
        )?
        .only_for(Artifact::ProductSchema),
        ToolDefinition::new(
            "remove_review_page",
            REMOVE_REVIEW_PAGE,
            Effect::Changes,
            remove_review_page,
        )?
        .only_for(Artifact::ProductSchema),
        ToolDefinition::new(
            "mark_page_not_product",
            MARK_PAGE_NOT_PRODUCT,
            Effect::Changes,
            mark_page_not_product,
        )?
        .only_for(Artifact::ProductSchema),
        ToolDefinition::new(
            "declare_removed_page",
            DECLARE_REMOVED_PAGE,
            Effect::Changes,
            declare_removed_page,
        )?
        .only_for(Artifact::ProductSchema),
        ToolDefinition::new(
            "record_no_sold_page",
            RECORD_NO_SOLD_PAGE,
            Effect::Changes,
            record_no_sold_page,
        )?
        .only_for(Artifact::ProductSchema),
        ToolDefinition::new("label_source", LABEL_SOURCE, Effect::Changes, label_source)?,
    ])
}

async fn evaluate_candidate(
    arguments: EvaluateCandidateArguments,
    context: Arc<ToolContext>,
) -> Result<MatrixAnswer, ToolError> {
    let review_id = arguments.review_id;
    let raw_urls: Vec<&str> = arguments.urls.iter().map(String::as_str).collect();
    let urls = review_source_urls(&context, review_id, &raw_urls).await?;
    let mut proposal = Map::new();
    proposal.insert("schemas".to_owned(), json!(arguments.schemas));
    if let Some(templates) = arguments.removed_page_schemas {
        proposal.insert("removed_page_schemas".to_owned(), json!(templates));
    }
    proposal.insert(
        "urls".to_owned(),
        json!(urls.iter().map(url::Url::as_str).collect::<Vec<_>>()),
    );
    let answer = context
        .console
        .post(
            &review_path(review_id, "/evaluate"),
            &Value::Object(proposal),
        )
        .await?;
    let poll_path = match answer.status {
        202 => {
            let running: wire::StillRunning = answer.clone().decode()?;
            let evaluation_id = running.evaluation_id.ok_or_else(|| {
                ToolError::NotApplicable(format!(
                    "the console answered {} without an evaluation to poll",
                    running.status
                ))
            })?;
            review_path(
                review_id,
                &format!("/evaluations/{}", encoded(&evaluation_id)),
            )
        }
        _ => String::new(),
    };
    let answer = settled(&context, answer, &poll_path, "candidate evaluation").await?;
    Ok(MatrixAnswer {
        matrix: MatrixView::of(answer.decode()?),
        truncations: Vec::new(),
    })
}

async fn update_candidate(
    arguments: UpdateCandidateArguments,
    context: Arc<ToolContext>,
) -> Result<CandidateChanged, ToolError> {
    let review_id = arguments.review_id;
    let mut edit = Map::new();
    edit.insert(
        "expected_candidate_version".to_owned(),
        json!(arguments.expected_candidate_version),
    );
    match arguments.candidate {
        CandidateArgument::ProductSchema {
            schemas,
            removed_page_schemas,
        } => {
            edit.insert("schemas".to_owned(), json!(schemas));
            if let Some(templates) = removed_page_schemas {
                edit.insert("removed_page_schemas".to_owned(), json!(templates));
            }
        }
        CandidateArgument::UrlPattern { decision } => {
            // The ListingSource's pattern when the review opened stays as the review stores it.
            let (current_pattern, current_exclude, current_matching) =
                match candidate_of(&review_detail(&context, review_id).await?)? {
                    wire::Candidate::UrlPattern(candidate) => (
                        candidate.current_pattern,
                        candidate.current_exclude,
                        candidate.current_matching,
                    ),
                    wire::Candidate::ProductSchema(_) => {
                        return Err(ToolError::NotApplicable(
                            "the review is a product-schema review; give kind PRODUCT_SCHEMA"
                                .to_owned(),
                        ));
                    }
                };
            edit.insert(
                "decision".to_owned(),
                match decision {
                    UrlPatternDecisionArgument::Pattern {
                        value,
                        exclude,
                        matching,
                    } => {
                        let mut decision =
                            json!({ "type": "PATTERN", "value": value, "matching": matching });
                        if let Some(exclude) = exclude {
                            decision["exclude"] = json!(exclude);
                        }
                        decision
                    }
                    UrlPatternDecisionArgument::NoPattern {} => json!({ "type": "NO_PATTERN" }),
                },
            );
            edit.insert("current_pattern".to_owned(), json!(current_pattern));
            edit.insert("current_exclude".to_owned(), json!(current_exclude));
            edit.insert("current_matching".to_owned(), json!(current_matching));
        }
    }
    candidate_changed(
        &context,
        review_id,
        &review_path(review_id, "/candidate"),
        &Value::Object(edit),
    )
    .await
}

async fn stage_and_check(
    arguments: StageAndCheckArguments,
    context: Arc<ToolContext>,
) -> Result<StagedAndChecked, ToolError> {
    let review_id = arguments.review_id;
    let candidate_version = match (arguments.schemas, arguments.expected_candidate_version) {
        (Some(schemas), Some(expected_candidate_version)) => {
            let mut edit = json!({
                "expected_candidate_version": expected_candidate_version,
                "schemas": schemas,
            });
            if let Some(templates) = arguments.removed_page_schemas {
                edit["removed_page_schemas"] = json!(templates);
            }
            candidate_changed(
                &context,
                review_id,
                &review_path(review_id, "/candidate"),
                &edit,
            )
            .await?
            .candidate_version
        }
        (Some(_), None) => {
            return Err(ToolError::InvalidArguments(
                "staging schemas needs the expected_candidate_version you read".to_owned(),
            ));
        }
        (None, _) if arguments.removed_page_schemas.is_some() => {
            return Err(ToolError::InvalidArguments(
                "removed_page_schemas are staged with the schemas they belong to".to_owned(),
            ));
        }
        (None, _) => {
            review_detail(&context, review_id)
                .await?
                .review
                .candidate_version
        }
    };
    let matrix = refreshed_matrix(&context, review_id).await?;
    let approval = approval_checks(&context, review_id).await?;
    Ok(StagedAndChecked {
        review_id: review_id.to_string(),
        candidate_version,
        matrix,
        approval,
    })
}

async fn update_schema_field(
    arguments: UpdateSchemaFieldArguments,
    context: Arc<ToolContext>,
) -> Result<CandidateChanged, ToolError> {
    let review_id = arguments.review_id;
    candidate_changed(
        &context,
        review_id,
        &review_path(review_id, "/schema-field"),
        &json!({
            "expected_candidate_version": arguments.expected_candidate_version,
            "schema_index": arguments.schema_index,
            "field": arguments.field,
            "rule": arguments.rule,
        }),
    )
    .await
}

async fn add_review_page(
    arguments: AddReviewPageArguments,
    context: Arc<ToolContext>,
) -> Result<PageAdded, ToolError> {
    let review_id = arguments.review_id;
    let crawl_host = review_source_crawl_host(&context, review_id).await?;
    let url = review_source_url(&arguments.url, crawl_host.as_deref())?;
    let page: CrawlerReviewPage = context
        .console
        .post(
            &review_path(review_id, "/pages"),
            &json!({ "url": url.as_str() }),
        )
        .await?
        .decode()?;
    Ok(PageAdded {
        review_id: review_id.to_string(),
        review_page_id: page.review_page_id.to_string(),
        url: page.url,
        role: page.role,
    })
}

async fn remove_review_page(
    arguments: RemoveReviewPageArguments,
    context: Arc<ToolContext>,
) -> Result<PageRemoved, ToolError> {
    let review_id = arguments.review_id;
    let page_id = arguments.review_page_id;
    let detail = review_detail(&context, review_id).await?;
    if !detail
        .pages
        .iter()
        .any(|page| page.review_page_id == page_id)
    {
        return Err(ToolError::InvalidArguments(format!(
            "review page {page_id} is not a page of review {review_id}"
        )));
    }
    context
        .console
        .delete(&format!("/api/review-pages/{page_id}"))
        .await?;
    Ok(PageRemoved {
        review_id: review_id.to_string(),
        review_page_id: page_id.to_string(),
    })
}

async fn mark_page_not_product(
    arguments: MarkPageNotProductArguments,
    context: Arc<ToolContext>,
) -> Result<PageMarkedNotProduct, ToolError> {
    let review_id = arguments.review_id;
    let page_id = arguments.review_page_id;
    let page: CrawlerReviewPage = context
        .console
        .post(
            &format!("/api/reviews/{review_id}/pages/{page_id}/not-product"),
            &json!({ "reason": arguments.reason }),
        )
        .await?
        .decode()?;
    Ok(PageMarkedNotProduct {
        review_id: review_id.to_string(),
        review_page_id: page.review_page_id.to_string(),
        url: page.url,
        reason: page.not_product_reason.unwrap_or_default(),
    })
}

async fn declare_removed_page(
    arguments: DeclareRemovedPageArguments,
    context: Arc<ToolContext>,
) -> Result<CandidateChanged, ToolError> {
    let review_id = arguments.review_id;
    let template = removed_page_template(arguments.selector, arguments.text, arguments.regex)?;
    let wire::Candidate::ProductSchema(candidate) =
        candidate_of(&review_detail(&context, review_id).await?)?
    else {
        return Err(ToolError::NotApplicable(
            "only a product-schema review declares removed pages".to_owned(),
        ));
    };
    let mut templates = candidate.removed_page_schemas;
    templates.push(template);
    // The edit names the version it read the candidate at, so a candidate changed since is
    // refused instead of being overwritten with this one's schemas.
    candidate_changed(
        &context,
        review_id,
        &review_path(review_id, "/candidate"),
        &json!({
            "expected_candidate_version": arguments.expected_candidate_version,
            "schemas": candidate.schemas,
            "removed_page_schemas": templates,
        }),
    )
    .await
}

/// The removed-page template of a declaration, which names exactly one of `text` and `regex`;
/// the console validates the template itself.
fn removed_page_template(
    selector: String,
    text: Option<String>,
    regex: Option<String>,
) -> Result<RemovedPageSchema, ToolError> {
    match (text, regex) {
        (Some(text), None) => Ok(RemovedPageSchema {
            selector: selector.into(),
            text: Some(text),
            regex: None,
        }),
        (None, Some(regex)) => Ok(RemovedPageSchema {
            selector: selector.into(),
            text: None,
            regex: Some(regex),
        }),
        _ => Err(ToolError::InvalidArguments(
            "give exactly one of text and regex".to_owned(),
        )),
    }
}

async fn record_no_sold_page(
    arguments: RecordNoSoldPageArguments,
    context: Arc<ToolContext>,
) -> Result<NoSoldPageRecorded, ToolError> {
    let review_id = arguments.review_id;
    let raw_urls: Vec<&str> = arguments.urls.iter().map(String::as_str).collect();
    let urls: Vec<String> = review_source_urls(&context, review_id, &raw_urls)
        .await?
        .iter()
        .map(|url| url.as_str().to_owned())
        .collect();
    let recorded: wire::NoSoldPageRecorded = context
        .console
        .post(
            &review_path(review_id, "/no-sold-page"),
            &json!({ "urls": urls, "notes": arguments.notes }),
        )
        .await?
        .decode()?;
    Ok(NoSoldPageRecorded {
        review_id: review_id.to_string(),
        recorded_urls: recorded.recorded_urls,
    })
}

async fn label_source(
    arguments: LabelSourceArguments,
    context: Arc<ToolContext>,
) -> Result<SourceLabelled, ToolError> {
    let review_id = arguments.review_id;
    let labelled: wire::SourceLabelled = context
        .console
        .post(
            &review_path(review_id, "/source-label"),
            &json!({ "label": arguments.label, "note": arguments.note }),
        )
        .await?
        .decode()?;
    Ok(SourceLabelled {
        review_id: review_id.to_string(),
        label: labelled.label,
        labelled_at: labelled.labelled_at,
    })
}

/// Posts a candidate change and answers the candidate version after it.
async fn candidate_changed(
    context: &ToolContext,
    review_id: CrawlerReviewId,
    path: &str,
    body: &Value,
) -> Result<CandidateChanged, ToolError> {
    let changed: wire::CandidateEdited = context.console.post(path, body).await?.decode()?;
    Ok(CandidateChanged {
        review_id: review_id.to_string(),
        candidate_version: changed.candidate_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_declare_a_removed_page_with_exactly_one_of_text_and_regex() {
        let text = removed_page_template("p.notice".to_owned(), Some("Sold".to_owned()), None);
        let regex = removed_page_template("p.notice".to_owned(), None, Some("(?i)sold".to_owned()));

        assert_eq!(
            text.ok().and_then(|template| template.text),
            Some("Sold".to_owned())
        );
        assert_eq!(
            regex.ok().and_then(|template| template.regex),
            Some("(?i)sold".to_owned())
        );
        for (text, regex) in [(None, None), (Some("Sold"), Some("sold"))] {
            assert!(matches!(
                removed_page_template(
                    "p.notice".to_owned(),
                    text.map(str::to_owned),
                    regex.map(str::to_owned)
                ),
                Err(ToolError::InvalidArguments(_))
            ));
        }
    }

    #[test]
    fn should_require_the_rule_of_a_field_edit_and_take_null_as_its_deletion() {
        let review_id = CrawlerReviewId::new().to_string();
        let edit = |rule: Option<Value>| {
            let mut arguments = json!({
                "review_id": review_id,
                "expected_candidate_version": 3,
                "schema_index": 0,
                "field": "price",
            });
            if let Some(rule) = rule {
                arguments["rule"] = rule;
            }
            serde_json::from_value::<UpdateSchemaFieldArguments>(arguments)
        };

        assert!(edit(None).is_err());
        assert!(edit(Some(Value::Null)).is_ok_and(|arguments| arguments.rule.is_none()));
        assert!(
            edit(Some(json!({ "selector": ".price", "type": "text" })))
                .is_ok_and(|arguments| arguments.rule.is_some())
        );
    }

    /// An agent's pattern is matched against the path and query unless it names another scope.
    #[test]
    fn should_match_an_agent_pattern_against_the_path_and_query_by_default() {
        let matching = |decision: Value| match serde_json::from_value::<UrlPatternDecisionArgument>(
            decision,
        ) {
            Ok(UrlPatternDecisionArgument::Pattern { matching, .. }) => Some(matching),
            _ => None,
        };

        assert_eq!(
            matching(json!({ "type": "PATTERN", "value": "/item/" })),
            Some(UrlPatternMatching::PathAndQueryFullMatch)
        );
        assert_eq!(
            matching(
                json!({ "type": "PATTERN", "value": "/item/", "matching": "FULL_URL_SEARCH" })
            ),
            Some(UrlPatternMatching::FullUrlSearch)
        );
    }

    #[test]
    fn should_take_a_candidate_of_either_kind_and_refuse_an_undecided_one() {
        let review_id = CrawlerReviewId::new().to_string();
        let update = |candidate: Value| {
            serde_json::from_value::<UpdateCandidateArguments>(json!({
                "review_id": review_id,
                "expected_candidate_version": 2,
                "candidate": candidate,
            }))
        };

        assert!(update(json!({ "kind": "URL_PATTERN", "decision": { "type": "PATTERN", "value": "/item/" } })).is_ok());
        assert!(update(json!({ "kind": "URL_PATTERN", "decision": { "type": "PATTERN", "value": "/item/", "exclude": "gift-card" } })).is_ok());
        assert!(update(json!({ "kind": "URL_PATTERN", "decision": { "type": "PATTERN", "value": "/item/", "matching": "FULL_URL_SEARCH" } })).is_ok());
        assert!(
            update(json!({ "kind": "URL_PATTERN", "decision": { "type": "NO_PATTERN" } })).is_ok()
        );
        assert!(update(json!({ "kind": "PRODUCT_SCHEMA", "schemas": [] })).is_ok());
        for candidate in [
            json!({ "kind": "URL_PATTERN", "decision": { "type": "UNDECIDED" } }),
            json!({ "kind": "URL_PATTERN", "decision": { "type": "PATTERN", "value": "/item/" }, "sample_labels": [] }),
            json!({ "kind": "URL_PATTERN", "decision": { "type": "PATTERN", "value": "/item/", "excluding": "gift-card" } }),
            json!({ "kind": "URL_PATTERN", "decision": { "type": "NO_PATTERN", "exclude": "gift-card" } }),
            json!({ "kind": "URL_PATTERN", "decision": { "type": "PATTERN", "value": "/item/", "matching": "PATH" } }),
            json!({ "schemas": [] }),
        ] {
            assert!(update(candidate.clone()).is_err(), "{candidate}");
        }
    }
}
