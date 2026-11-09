//! The author agent's tools: evaluating, staging and labelling candidates and the review's pages.

use super::shared::encoded;
use super::{
    Effect, ToolContext, ToolDefinition, ToolSchemaError, candidate_of, review_detail, review_path,
    review_source_crawl_host, review_source_url, review_source_urls, settled,
};
use crate::review::model::CrawlerReviewPage;
use crate::review_mcp::errors::ToolError;
use crate::review_mcp::views::{
    CandidateChanged, MatrixAnswer, MatrixView, NoSoldPageRecorded, PageAdded,
    PageMarkedNotProduct, PageRemoved, SampleLabelKind,
};
use crate::review_mcp::wire;
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crate::scraper::css_selector::rule::ExtractionRule;
use crate::{CrawlerReviewId, CrawlerReviewPageId};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::sync::Arc;

const EVALUATE_CANDIDATE: &str = "Test a product-schema candidate before you stage it, without storing anything: the console fetches the review's pages and up to four further `urls` of the review's ListingSource live, at the shop's crawl delay, and evaluates on them what approving this candidate would make live. The answer has the matrix refresh_matrix answers; pages from `urls` are named by input_index, their position in `urls`. Use it to test every candidate on sold, price-on-application and photo-less pages of the shop before update_candidate. schemas is the whole schema list; removed_page_schemas, when given, replaces the candidate's removed-page templates, otherwise the stored ones are evaluated. The candidate is checked like an edit (INVALID_REQUEST names what is wrong), a URL off the review's ListingSource is refused with URL_OUTSIDE_REVIEW_SOURCE, and on a review with human_required true the evaluation is refused with CONFLICT and reason HUMAN_REQUIRED before anything is fetched. An evaluation is one of the console's live jobs, its matrix refreshes, candidate evaluations and page projections of all reviews together, of which it runs only a few at once: RATE_LIMITED with reason LIVE_JOBS_BUSY means retry after retry_after_secs. Waits while the console fetches and answers STILL_RUNNING after 15 minutes. When the shop answers a page with a bot challenge instead of the page, the call fails with CONSOLE_FAILED, status 502 and reason BOT_CHALLENGE, and its error names the page: the page was not served, so it says nothing about the candidate; retry later and leave the candidate as it is.";

const UPDATE_CANDIDATE: &str = "Stage a whole candidate on a pending review, replacing the current one. For a PRODUCT_SCHEMA review give kind PRODUCT_SCHEMA with the whole `schemas` list (add, change, reorder or drop schemas here; every core field rule needs selector and type, raw attributes only with keys of the raw attribute registry) and optionally `removed_page_schemas`, which replaces the removed-page templates (left out, they stay; an empty list retires them all). For a URL_PATTERN review give kind URL_PATTERN with the decision: type PATTERN with a regex `value` that matches every URL labelled PRODUCT and no URL labelled NOT_PRODUCT (the spider matches it anywhere in the URL, with or without the URL's trailing slash, so anchor it as needed), and optionally a regex `exclude` for the few non-items that share the product URL shape, such as gift cards, vouchers, services or the dealer's own profile page: a URL is a product URL when it matches `value` and does not match `exclude`, which is matched anywhere in the URL like `value` (the regex engine has no lookahead, so exclude them here instead of writing slug-prefix tricks into `value`; `exclude` must not match any URL labelled PRODUCT and must match or be missed by `value` for every URL labelled NOT_PRODUCT), or type NO_PATTERN for a ListingSource without product pages, which an agent approval accepts on at least the minimum of NOT_PRODUCT labels, no PRODUCT label and no inspected sample page that describes one product without a NOT_PRODUCT label (NO_PATTERN_CONTRADICTED); its sample labels stay as label_sample_urls set them. expected_candidate_version is the candidate_version you read; when the review changed since, the edit is refused with CONFLICT and the current candidate_version: read the review again and redo the edit on its current candidate. Answers the new candidate_version. Any change drops the stored matrix: refresh_matrix afterwards.";

const UPDATE_SCHEMA_FIELD: &str = "Change one rule of one schema of a pending product-schema review's candidate. schema_index is the schema's position in the candidate's schemas; field is a core field (source_listing_id, title, description, price, price_estimate_min, price_estimate_max, state, images) or `raw_attributes.<key>` with a key of the raw attribute registry; rule is the whole new rule, or null to delete an optional rule (title, state and images cannot be deleted). expected_candidate_version is the candidate_version you read; a stale one is refused with CONFLICT and the current candidate_version. Answers the new candidate_version. The change drops the stored matrix: refresh_matrix afterwards.";

const ADD_REVIEW_PAGE: &str = "Add a page of the review's ListingSource to a pending product-schema review, so every later matrix evaluates the candidate on it too: use it for sold, price-on-application or photo-less product pages the candidate must cover. url must be an http(s) URL on the crawl host of the review's ListingSource (otherwise URL_OUTSIDE_REVIEW_SOURCE, before anything is fetched). The console fetches the page now; a redirect to a page that is no product page, a page already on the review, and more than four added pages are refused with INVALID_REQUEST or CONFLICT. A page the shop answers with a bot challenge is not added: the call fails with CONSOLE_FAILED, status 502 and reason BOT_CHALLENGE; retry later. Answers the new review_page_id. Drops the stored matrix: refresh_matrix afterwards.";

const REMOVE_REVIEW_PAGE: &str = "Remove a page that was added to the review, or that waits on it, from a pending product-schema review, for example once the shop deleted the page and every matrix refresh fails on it. Seed and triggering pages cannot be removed (INVALID_REQUEST). review_page_id must be a page of the given review. Drops the stored matrix: refresh_matrix afterwards.";

const MARK_PAGE_NOT_PRODUCT: &str = "Mark a review page of a pending product-schema review as no product page: the dealer's profile, a category or article page, a gift card, anything the URL pattern takes that is no product of the catalogue. Mark it instead of bending the schema to extract it or guessing at the page; the reviewer judges your reason and gives NEEDS_REPAIR when the page is a product after all. reason, 1 to 500 characters, says what the page shows. A marked page is left out of the matrix and of the review's coverage, so no schema has to resolve it, and once the review is approved its URL stops being scraped and stays a non-product page. review_page_id must be a page of the given review, a seed or triggering page included. Drops the stored matrix: refresh_matrix afterwards. A review with human_required true is refused with CONFLICT and reason HUMAN_REQUIRED.";

const DECLARE_REMOVED_PAGE: &str = "Declare a kind of review page removed instead of authoring a schema for it: for a sold or withdrawn item the shop still serves with HTTP 200, a removal notice and no product. Adds one removed-page template to the pending product-schema candidate: selector selects the element holding the notice, and exactly one of text (the notice's text) or regex (a regex the notice's text matches; one that matches any text is refused). Declare a page removed only where the page itself shows a removal notice. The matrix then lists the pages the template matches and whether a schema still extracts a product from them (PRODUCT_STILL_EXTRACTS, a template too broad for that page). expected_candidate_version is the candidate_version you read; a stale one is refused with CONFLICT. Answers the new candidate_version; refresh_matrix afterwards.";

const RECORD_NO_SOLD_PAGE: &str = "Record that you looked for a sold or unavailable product page of the review's ListingSource and found none, so the console accepts a candidate tested on none. An approval needs a review page whose extracted state is sold, out of stock or unavailable (or that a removed-page template captures), or this record. Use it only after you searched: open the shop's listing pages, its sold or archive section and filters, and product pages with inspect_page; when you find a sold or unavailable page, add it with add_review_page instead. urls are 3 to 50 distinct pages of the review's ListingSource you opened with inspect_page on this review since a human last changed or judged it (a URL off its crawl host is refused with URL_OUTSIDE_REVIEW_SOURCE; fewer or more with INVALID_REQUEST; a page not inspected so with CONFLICT and reason PAGES_NOT_INSPECTED, whose error names those pages: inspect them, or leave them out), notes say how you searched and what you saw, in a few sentences. The record changes no candidate and does not move candidate_version. A review agent's approval is judged on it as the trail shows it until a human changes or judges the review; after that, search and record again.";
const LABEL_SAMPLE_URLS: &str = "Set the sample labels of a pending URL-pattern review: for URLs on the crawl host of the review's ListingSource, its URL sample's among them, say whether each is the page of one product (PRODUCT) or not (NOT_PRODUCT: a category, search, article, imprint or other page), each with a short reason saying what the page shows. Open the pages with inspect_page before labelling them; describes_one_product is a hint, not proof. The list replaces every label the review has, so always send all labels; an empty list removes them. An approval needs at least the console's minimum of each label, and a pattern that takes every PRODUCT URL and no NOT_PRODUCT URL. A URL off the crawl host of the review's ListingSource is refused with URL_OUTSIDE_REVIEW_SOURCE; a URL labelled twice and a missing reason with INVALID_REQUEST. expected_candidate_version is the candidate_version you read; labels belong to the candidate, so they move its version. Answers the new candidate_version.";

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
    /// A regex `value`: a URL it matches anywhere is a product URL unless the optional regex
    /// `exclude` matches it anywhere too.
    Pattern {
        value: String,
        /// Takes the non-items that share the product URL shape (gift cards, vouchers, services,
        /// the dealer's own profile page) out of `value`'s matches; left out, none is taken out.
        #[serde(default)]
        exclude: Option<String>,
    },
    /// The ListingSource has no product pages. A struct variant, so a stray key such as an
    /// `exclude` is refused instead of ignored.
    NoPattern {},
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
    /// The pages of the review's ListingSource you checked: 3 to 50 distinct URLs on its crawl host.
    urls: Vec<String>,
    /// How you searched and what you saw, in a few sentences.
    notes: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct LabelSampleUrlsArguments {
    /// The review's id, such as `cr_01...`, as list_pending_reviews names it.
    #[schemars(with = "String")]
    review_id: CrawlerReviewId,
    /// The candidate_version you read, as get_review answers it.
    expected_candidate_version: i64,
    /// Every label of the review; it replaces the labels the review has.
    labels: Vec<SampleLabelArgument>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SampleLabelArgument {
    /// A URL on the crawl host of the review's ListingSource.
    url: String,
    label: SampleLabelKind,
    /// What the page shows that makes it a product page or not, in a few words.
    reason: String,
}

pub(super) fn tools() -> Result<Vec<ToolDefinition>, ToolSchemaError> {
    Ok(vec![
        ToolDefinition::new(
            "evaluate_candidate",
            EVALUATE_CANDIDATE,
            Effect::Reads,
            evaluate_candidate,
        )?,
        ToolDefinition::new(
            "update_candidate",
            UPDATE_CANDIDATE,
            Effect::Changes,
            update_candidate,
        )?,
        ToolDefinition::new(
            "update_schema_field",
            UPDATE_SCHEMA_FIELD,
            Effect::Changes,
            update_schema_field,
        )?,
        ToolDefinition::new(
            "add_review_page",
            ADD_REVIEW_PAGE,
            Effect::Changes,
            add_review_page,
        )?,
        ToolDefinition::new(
            "remove_review_page",
            REMOVE_REVIEW_PAGE,
            Effect::Changes,
            remove_review_page,
        )?,
        ToolDefinition::new(
            "mark_page_not_product",
            MARK_PAGE_NOT_PRODUCT,
            Effect::Changes,
            mark_page_not_product,
        )?,
        ToolDefinition::new(
            "declare_removed_page",
            DECLARE_REMOVED_PAGE,
            Effect::Changes,
            declare_removed_page,
        )?,
        ToolDefinition::new(
            "record_no_sold_page",
            RECORD_NO_SOLD_PAGE,
            Effect::Changes,
            record_no_sold_page,
        )?,
        ToolDefinition::new(
            "label_sample_urls",
            LABEL_SAMPLE_URLS,
            Effect::ChangesIdempotently,
            label_sample_urls,
        )?,
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
            let (current_pattern, current_exclude) =
                match candidate_of(&review_detail(&context, review_id).await?)? {
                    wire::Candidate::UrlPattern(candidate) => {
                        (candidate.current_pattern, candidate.current_exclude)
                    }
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
                    UrlPatternDecisionArgument::Pattern { value, exclude } => {
                        let mut decision = json!({ "type": "PATTERN", "value": value });
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

async fn label_sample_urls(
    arguments: LabelSampleUrlsArguments,
    context: Arc<ToolContext>,
) -> Result<CandidateChanged, ToolError> {
    let review_id = arguments.review_id;
    let raw_urls: Vec<&str> = arguments
        .labels
        .iter()
        .map(|label| label.url.as_str())
        .collect();
    let urls = review_source_urls(&context, review_id, &raw_urls).await?;
    let labels: Vec<Value> = arguments
        .labels
        .iter()
        .zip(&urls)
        .map(|(label, url)| {
            json!({ "url": url.as_str(), "label": label.label, "reason": label.reason })
        })
        .collect();
    candidate_changed(
        &context,
        review_id,
        &review_path(review_id, "/sample-labels"),
        &json!({
            "expected_candidate_version": arguments.expected_candidate_version,
            "labels": labels,
        }),
    )
    .await
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
        assert!(
            update(json!({ "kind": "URL_PATTERN", "decision": { "type": "NO_PATTERN" } })).is_ok()
        );
        assert!(update(json!({ "kind": "PRODUCT_SCHEMA", "schemas": [] })).is_ok());
        for candidate in [
            json!({ "kind": "URL_PATTERN", "decision": { "type": "UNDECIDED" } }),
            json!({ "kind": "URL_PATTERN", "decision": { "type": "PATTERN", "value": "/item/" }, "sample_labels": [] }),
            json!({ "kind": "URL_PATTERN", "decision": { "type": "PATTERN", "value": "/item/", "excluding": "gift-card" } }),
            json!({ "kind": "URL_PATTERN", "decision": { "type": "NO_PATTERN", "exclude": "gift-card" } }),
            json!({ "schemas": [] }),
        ] {
            assert!(update(candidate.clone()).is_err(), "{candidate}");
        }
    }

    #[test]
    fn should_take_only_known_sample_labels_with_a_reason() {
        let review_id = CrawlerReviewId::new().to_string();
        let labels = |label: Value| {
            serde_json::from_value::<LabelSampleUrlsArguments>(json!({
                "review_id": review_id,
                "expected_candidate_version": 1,
                "labels": [label],
            }))
        };

        assert!(labels(json!({ "url": "https://shop.example/item/1", "label": "PRODUCT", "reason": "one chair" })).is_ok());
        assert!(labels(json!({ "url": "https://shop.example/item/1", "label": "NOT_PRODUCT", "reason": "category" })).is_ok());
        for label in [
            json!({ "url": "https://shop.example/item/1", "label": "product", "reason": "x" }),
            json!({ "url": "https://shop.example/item/1", "label": "CATEGORY", "reason": "x" }),
            json!({ "url": "https://shop.example/item/1", "label": "PRODUCT" }),
        ] {
            assert!(labels(label.clone()).is_err(), "{label}");
        }
    }
}
