//! The qualifier's tools: reading a queued ListingSource's own pages and deciding once whether it
//! is a real art and antiques e-shop (`docs/crawler/README.md#listingsource-qualification`).

use super::shared::encoded;
use super::{Effect, ToolContext, ToolDefinition, ToolSchemaError};
use crate::review::model::{Qualification, SourceLabel};
use crate::review::page_projection::PageView;
use crate::review_mcp::errors::ToolError;
use crate::review_mcp::views::{QualificationDecided, SourcePage, SourcePageAnswer};
use listing_source_core::ListingSourceId;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

const READ_SOURCE_PAGE: &str = "Read a page of the ListingSource you qualify, fetched now from the shop through the crawler's network: with its user agent AuraHistoriaBot and obeying the shop's robots.txt. Leave url out to read the crawl root, the start page; any other url must lie on the source's crawl host (a path such as /shop/ is resolved against it). view OUTLINE (the default) shows the page's elements one per line in CSS notation, MARKDOWN its readable text; both leave out scripts, styles, navigation, header and footer. links lists up to 80 shop pages the page links, with their texts, the page's own content first and its navigation after: read the category, shop and item pages from there instead of guessing URLs. links_total counts them all; when it is higher than links_offset plus the links listed, a large menu may hide the item pages, so read the same page again with links_offset set to the next position (each read fetches the page again). A page the shop's robots.txt refuses the crawler answers an error with reason ROBOTS_DISALLOWED, whose details quote the refusing rule, such as User-agent: *; Disallow: /. A shop whose robots.txt asks for a Crawl-delay above the 60 seconds the crawler honours answers every page with reason ROBOTS_CRAWL_DELAY_TOO_LONG, quoting the rule, such as User-agent: *; Crawl-delay: 120: the crawler requests nothing from it. A bot challenge answers reason BOT_CHALLENGE. Each call fetches the shop once; a process has a small budget of calls (INSPECT_LIMIT_REACHED when spent).";

const QUALIFY_SOURCE: &str = "Decide, once, whether the ListingSource is a real art and antiques e-shop the crawler may crawl, with a note of 1 to 500 characters naming your evidence and the URLs you read. QUALIFIED: it offers art and antiques for sale online (art, or antiques of any kind, including antique and vintage collectibles; not ordinary second-hand goods, new collectibles or reproductions) as a catalogue of individual items, each showing it can be bought or enquired about by a price, price on request, an estimate or an availability status, bought through a cart or buy button, by enquiry or by bidding, so a dealer's or gallery's enquiry-only catalogue, an auction house and a marketplace or auction platform of many sellers all qualify; a mixed shop qualifies when they are a significant part of what it sells (give a rough share and the categories seen). Add a label to a QUALIFIED decision only when the crawler cannot crawl this real art and antiques e-shop: URL_PATTERN_APPROACH_NOT_APPLICABLE, PRODUCT_SCHEMA_APPROACH_NOT_APPLICABLE, BOT_PROTECTION, JAVASCRIPT_RENDERED, LOGIN_REQUIRED or OTHER. DISQUALIFIED needs its label: ROBOTS_DISALLOWED when robots.txt refuses AuraHistoriaBot the crawl root or its product pages, by a Disallow rule or a Crawl-delay above 60 seconds (quote the rule), SHOP_CLOSED when the domain is parked or for sale or the site says the business closed (never for one failed fetch), NOT_AN_ESHOP when it offers no items for sale (such as a brochure site about the business or its services, a museum or collection, an artist's portfolio or exhibition pages whose works show no sign of being for sale, or a record of sold items only, such as an auction house's past results without lots to come), NOT_AN_ANTIQUE_SHOP when it offers items for sale but not art or antiques. NEEDS_HUMAN without a label when you cannot tell from what you could read, such as an unreachable site without proof that it closed or a bot challenge on every page. The decision's label replaces the source's label, which stays in its history; a QUALIFIED decision without a label keeps a label the source has, for a human (label_effect KEPT_LABEL). Refused with CONFLICT and reason ALREADY_QUALIFIED when the source has a qualification, HUMAN_LABEL when a human's label stands, NOT_IN_CRAWL_SCOPE when the scope does not admit it. After it, report.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadSourcePageArguments {
    /// The ListingSource's id, such as `ls_01...`, as your task names it.
    #[schemars(with = "String")]
    listing_source_id: ListingSourceId,
    /// A URL or path on the source's crawl host; the crawl root when left out.
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    view: Option<PageView>,
    /// The position of the first link to list, for the links after an earlier answer's; 0 when
    /// left out.
    #[serde(default)]
    links_offset: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct QualifySourceArguments {
    /// The ListingSource's id, such as `ls_01...`, as your task names it.
    #[schemars(with = "String")]
    listing_source_id: ListingSourceId,
    qualification: Qualification,
    /// The label the decision sets; required for DISQUALIFIED, none for NEEDS_HUMAN.
    #[serde(default)]
    label: Option<SourceLabel>,
    /// Your evidence: what you read and why, naming the URLs.
    note: String,
}

pub(super) fn tools() -> Result<Vec<ToolDefinition>, ToolSchemaError> {
    Ok(vec![
        ToolDefinition::new(
            "read_source_page",
            READ_SOURCE_PAGE,
            Effect::Reads,
            read_source_page,
        )?,
        ToolDefinition::new(
            "qualify_source",
            QUALIFY_SOURCE,
            Effect::Decides,
            qualify_source,
        )?,
    ])
}

fn qualification_path(listing_source_id: ListingSourceId, suffix: &str) -> String {
    format!("/api/source-qualifications/{listing_source_id}{suffix}")
}

async fn read_source_page(
    arguments: ReadSourcePageArguments,
    context: Arc<ToolContext>,
) -> Result<SourcePage, ToolError> {
    let mut query = format!(
        "/page?view={}",
        arguments.view.unwrap_or_default().query_name()
    );
    if let Some(url) = arguments
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        query.push_str(&format!("&url={}", encoded(url)));
    }
    if let Some(offset) = arguments.links_offset.filter(|offset| *offset > 0) {
        query.push_str(&format!("&links_offset={offset}"));
    }
    context.inspections.take()?;
    match context
        .console
        .get(&qualification_path(arguments.listing_source_id, &query))
        .await
    {
        Ok(answer) => Ok(SourcePage::from(answer.decode::<SourcePageAnswer>()?)),
        Err(error) => {
            context.inspections.refund();
            Err(error.into())
        }
    }
}

async fn qualify_source(
    arguments: QualifySourceArguments,
    context: Arc<ToolContext>,
) -> Result<QualificationDecided, ToolError> {
    Ok(context
        .console
        .post(
            &qualification_path(arguments.listing_source_id, ""),
            &json!({
                "qualification": arguments.qualification,
                "label": arguments.label,
                "note": arguments.note,
            }),
        )
        .await?
        .decode()?)
}
