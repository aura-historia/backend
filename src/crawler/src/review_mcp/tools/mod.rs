//! The review tools and the role each belongs to. Each tool decodes its `schemars` arguments,
//! shapes one or a few console requests, and maps the console's answer to its own output type; the
//! console decides everything. A role's tool set holds only its own tools, so the other role's
//! tools are neither listed nor callable.

mod author;
mod reviewer;
mod shared;

use super::bounds::{MAX_TOOL_OUTPUT_BYTES, bound_answer};
use super::console::{ConsoleAnswer, ConsoleClient, ConsoleError};
use super::errors::ToolError;
use super::settings::AgentRole;
use super::wire;
use crate::CrawlerReviewId;
use crate::review::model::{ARTIFACT_PRODUCT_SCHEMA, ARTIFACT_URL_PATTERN};
use crate::review::source_url::is_review_source_url;
use rmcp::model::{CallToolResult, JsonObject, Tool, ToolAnnotations};
use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, warn};
use url::Url;

/// What the tools of one `review-mcp` process share: the console client acting with the role's
/// token, and how long a tool waits for a background evaluation.
pub struct ToolContext {
    pub(super) console: ConsoleClient,
    pub(super) polling: Polling,
    /// The `inspect_page` calls this process has left.
    inspections: shared::InspectionBudget,
}

impl ToolContext {
    pub fn new(console: ConsoleClient, polling: Polling) -> Self {
        Self {
            console,
            polling,
            inspections: shared::InspectionBudget::default(),
        }
    }
}

/// How a tool waits for a matrix refresh or a candidate evaluation that the console runs in the
/// background: it polls at `interval` for at most `max_wait`.
#[derive(Debug, Clone, Copy)]
pub struct Polling {
    pub interval: Duration,
    pub max_wait: Duration,
}

impl Default for Polling {
    /// A refresh fetches every page at the shop's crawl delay, up to a minute per page.
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(3),
            max_wait: Duration::from_secs(15 * 60),
        }
    }
}

type ToolFuture = Pin<Box<dyn Future<Output = CallToolResult> + Send>>;
type ToolHandler = dyn Fn(JsonObject, Arc<ToolContext>) -> ToolFuture + Send + Sync;

/// What a tool does to the review, for its MCP annotations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Effect {
    /// Reads, fetches or evaluates without changing the review.
    Reads,
    /// Changes the review in a way the same call repeats harmlessly.
    ChangesIdempotently,
    /// Changes the review.
    Changes,
    /// Decides the review, which only a human can take back.
    Decides,
}

/// The reviews a tool works on, by their artifact type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Artifact {
    /// Reviews of every artifact type.
    Any,
    ProductSchema,
    UrlPattern,
}

impl Artifact {
    fn includes(self, artifact_type: &str) -> bool {
        match self {
            Self::Any => true,
            Self::ProductSchema => artifact_type == ARTIFACT_PRODUCT_SCHEMA,
            Self::UrlPattern => artifact_type == ARTIFACT_URL_PATTERN,
        }
    }
}

/// One tool: its MCP description with input and output schemas, the reviews it works on, and its
/// handler.
#[derive(Clone)]
pub struct ToolDefinition {
    tool: Tool,
    artifact: Artifact,
    handler: Arc<ToolHandler>,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolSchemaError {
    #[error("the {schema} schema of tool {tool} is not a JSON object schema")]
    NotAnObject {
        tool: &'static str,
        schema: &'static str,
    },
}

impl ToolDefinition {
    /// A tool named `name` that decodes its arguments as `I`, runs `handler` and answers its `O`
    /// within [`MAX_TOOL_OUTPUT_BYTES`].
    pub(super) fn new<I, O, F, Fut>(
        name: &'static str,
        description: &'static str,
        effect: Effect,
        handler: F,
    ) -> Result<Self, ToolSchemaError>
    where
        I: DeserializeOwned + JsonSchema + Send + 'static,
        O: Serialize + JsonSchema + Send + 'static,
        F: Fn(I, Arc<ToolContext>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<O, ToolError>> + Send + 'static,
    {
        let input = object_schema::<I>(SchemaSettings::default().for_deserialize()).ok_or(
            ToolSchemaError::NotAnObject {
                tool: name,
                schema: "input",
            },
        )?;
        let output = object_schema::<O>(SchemaSettings::default().for_serialize()).ok_or(
            ToolSchemaError::NotAnObject {
                tool: name,
                schema: "output",
            },
        )?;
        let mut tool = Tool::new(name, description, input);
        tool.output_schema = Some(Arc::new(output));
        tool.annotations = Some(
            ToolAnnotations::new()
                .read_only(effect == Effect::Reads)
                .destructive(effect == Effect::Decides)
                .idempotent(matches!(
                    effect,
                    Effect::Reads | Effect::ChangesIdempotently
                ))
                .open_world(false),
        );
        let handler = Arc::new(handler);
        let handler: Arc<ToolHandler> = Arc::new(move |arguments, context| {
            let handler = Arc::clone(&handler);
            Box::pin(async move {
                let answer = match serde_json::from_value::<I>(Value::Object(arguments)) {
                    Ok(input) => handler(input, context).await,
                    Err(error) => Err(ToolError::InvalidArguments(error.to_string())),
                };
                tool_result(name, answer)
            })
        });
        Ok(Self {
            tool,
            artifact: Artifact::Any,
            handler,
        })
    }

    /// The tool as one that works only on reviews of `artifact`.
    pub(super) fn only_for(mut self, artifact: Artifact) -> Self {
        self.artifact = artifact;
        self
    }

    /// The tool as a server scoped to a review of `artifact_type` lists it: `None` for one that
    /// works on no such review. A candidate argument that takes every artifact kind keeps the
    /// review's kind only, with the definitions only the other kinds used dropped, since every
    /// definition is in the agent's context on every turn.
    pub fn for_artifact(mut self, artifact_type: &str) -> Option<Self> {
        if !self.artifact.includes(artifact_type) {
            return None;
        }
        let mut input = Value::Object((*self.tool.input_schema).clone());
        if keep_candidate_kind(&mut input, artifact_type)
            && let Value::Object(mut input) = input
        {
            drop_unreferenced_definitions(&mut input);
            self.tool.input_schema = Arc::new(input);
        }
        Some(self)
    }

    /// The tool with the descriptions of its input's shared definitions (`$defs`) left out, for
    /// a tool whose argument types another tool's schema documents already.
    pub(super) fn without_nested_descriptions(mut self) -> Self {
        let mut input = (*self.tool.input_schema).clone();
        if let Some(definitions) = input.get_mut("$defs") {
            remove_descriptions(definitions);
        }
        self.tool.input_schema = Arc::new(input);
        self
    }

    pub fn tool(&self) -> &Tool {
        &self.tool
    }

    pub fn name(&self) -> &str {
        &self.tool.name
    }

    /// Whether the tool works on one review, the one its required `review_id` argument names.
    pub fn takes_review(&self) -> bool {
        self.tool
            .input_schema
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|required| required.iter().any(|name| name == "review_id"))
    }

    /// Runs the tool on `arguments` and answers its result or its tool error. The call is logged
    /// with its outcome, never with its arguments or answer.
    pub async fn call(&self, arguments: JsonObject, context: Arc<ToolContext>) -> CallToolResult {
        let started = Instant::now();
        let result = (self.handler)(arguments, context).await;
        debug!(
            tool = %self.tool.name,
            is_error = result.is_error.unwrap_or(false),
            elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "Review tool call finished"
        );
        result
    }
}

/// The tool result of a handler's answer, bounded, or of its failure. A failure that is no
/// refusal of the console, such as an unreachable console, is logged with its cause for the
/// operator; the agent's answer names only its kind.
fn tool_result<O: Serialize>(name: &str, answer: Result<O, ToolError>) -> CallToolResult {
    let answer = answer.and_then(|output| {
        let value = serde_json::to_value(output).map_err(|_| ToolError::AnswerTooLarge)?;
        bound_answer(value, MAX_TOOL_OUTPUT_BYTES).map_err(|_| ToolError::AnswerTooLarge)
    });
    match answer {
        Ok(value) => CallToolResult::structured(value),
        Err(error) => {
            if let ToolError::Console(console_error) = &error
                && !matches!(console_error, ConsoleError::Refused(_))
            {
                warn!(
                    tool = name,
                    error = %console_error,
                    cause = ?std::error::Error::source(console_error),
                    "Review tool call failed at the console"
                );
            }
            error.into_tool_result()
        }
    }
}

/// Keeps, of every `oneOf` whose variants are told apart by a constant `kind`, the variant of
/// `kind`; answers whether any variant was dropped.
fn keep_candidate_kind(schema: &mut Value, kind: &str) -> bool {
    let mut dropped = false;
    match schema {
        Value::Object(object) => {
            if let Some(Value::Array(variants)) = object.get_mut("oneOf") {
                let before = variants.len();
                variants.retain(|variant| {
                    variant["properties"]["kind"]["const"]
                        .as_str()
                        .is_none_or(|variant_kind| variant_kind == kind)
                });
                dropped |= variants.len() < before;
            }
            for value in object.values_mut() {
                dropped |= keep_candidate_kind(value, kind);
            }
        }
        Value::Array(items) => {
            for item in items {
                dropped |= keep_candidate_kind(item, kind);
            }
        }
        _ => {}
    }
    dropped
}

/// Drops the `$defs` nothing in `schema` refers to any more, directly or through another
/// definition.
fn drop_unreferenced_definitions(schema: &mut JsonObject) {
    let Some(Value::Object(definitions)) = schema.remove("$defs") else {
        return;
    };
    let mut referenced = std::collections::BTreeSet::new();
    let mut pending: Vec<String> = Vec::new();
    for (name, value) in schema.iter() {
        if name != "$defs" {
            collect_references(value, &mut pending);
        }
    }
    while let Some(name) = pending.pop() {
        if referenced.insert(name.clone())
            && let Some(definition) = definitions.get(&name)
        {
            collect_references(definition, &mut pending);
        }
    }
    let kept: JsonObject = definitions
        .into_iter()
        .filter(|(name, _)| referenced.contains(name))
        .collect();
    if !kept.is_empty() {
        schema.insert("$defs".to_owned(), Value::Object(kept));
    }
}

fn collect_references(schema: &Value, names: &mut Vec<String>) {
    match schema {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str)
                && let Some(name) = reference.strip_prefix("#/$defs/")
            {
                names.push(name.to_owned());
            }
            object
                .values()
                .for_each(|value| collect_references(value, names));
        }
        Value::Array(items) => items
            .iter()
            .for_each(|item| collect_references(item, names)),
        _ => {}
    }
}

fn remove_descriptions(schema: &mut Value) {
    match schema {
        Value::Object(object) => {
            if object.get("description").is_some_and(Value::is_string) {
                object.remove("description");
            }
            object.values_mut().for_each(remove_descriptions);
        }
        Value::Array(items) => items.iter_mut().for_each(remove_descriptions),
        _ => {}
    }
}

fn object_schema<T: JsonSchema>(settings: SchemaSettings) -> Option<JsonObject> {
    let schema =
        serde_json::to_value(settings.into_generator().into_root_schema_for::<T>()).ok()?;
    let mut schema = schema.as_object()?.clone();
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return None;
    }
    // A tool's schemas are plain objects; the draft marker adds nothing a client uses.
    schema.remove("$schema");
    Some(schema)
}

/// The tools of `role`, in the order they are listed.
pub fn tools_for(role: AgentRole) -> Result<Vec<ToolDefinition>, ToolSchemaError> {
    let mut tools = shared::tools()?;
    tools.extend(match role {
        AgentRole::Author => author::tools()?,
        AgentRole::Reviewer => reviewer::tools()?,
    });
    Ok(tools)
}

/// The tools of `role` that work on a review of `artifact_type`, each as
/// [`ToolDefinition::for_artifact`] shapes it.
pub fn tools_for_artifact(
    role: AgentRole,
    artifact_type: &str,
) -> Result<Vec<ToolDefinition>, ToolSchemaError> {
    Ok(tools_for(role)?
        .into_iter()
        // Neither agent reads a page of a URL-pattern review; the console refuses it as well.
        .filter(|tool| !(artifact_type == ARTIFACT_URL_PATTERN && tool.name() == "inspect_page"))
        .filter_map(|tool| tool.for_artifact(artifact_type))
        .collect())
}

/// The artifact type of the review `review_id`, as the console answers it.
pub async fn review_artifact_type(
    context: &ToolContext,
    review_id: CrawlerReviewId,
) -> Result<String, ToolError> {
    Ok(review_detail(context, review_id)
        .await?
        .review
        .artifact_type)
}

fn review_path(review_id: CrawlerReviewId, suffix: &str) -> String {
    format!("/api/reviews/{review_id}{suffix}")
}

async fn review_detail(
    context: &ToolContext,
    review_id: CrawlerReviewId,
) -> Result<wire::ReviewDetail, ToolError> {
    Ok(context
        .console
        .get(&review_path(review_id, ""))
        .await?
        .decode()?)
}

/// The review's candidate, decoded from the payload its artifact type stores.
fn candidate_of(detail: &wire::ReviewDetail) -> Result<wire::Candidate, ToolError> {
    let payload = detail.review.candidate_payload.clone();
    match detail.review.artifact_type.as_str() {
        ARTIFACT_PRODUCT_SCHEMA => serde_json::from_value(payload)
            .map(wire::Candidate::ProductSchema)
            .map_err(|_| ToolError::UndecodableCandidate("product-schema")),
        ARTIFACT_URL_PATTERN => serde_json::from_value(payload)
            .map(wire::Candidate::UrlPattern)
            .map_err(|_| ToolError::UndecodableCandidate("URL-pattern")),
        _ => Err(ToolError::NotApplicable(format!(
            "the review has artifact type {}, which no review tool works on",
            detail.review.artifact_type
        ))),
    }
}

/// The crawl host of the review's ListingSource, the only host a URL given to a tool may name.
async fn review_source_crawl_host(
    context: &ToolContext,
    review_id: CrawlerReviewId,
) -> Result<Option<String>, ToolError> {
    Ok(review_detail(context, review_id)
        .await?
        .review
        .listing_source_crawl_host)
}

/// `raw_url` when it is a plain http(s) URL on the review's ListingSource `crawl_host`, as the
/// console itself requires; any other URL is refused before a request is sent.
fn review_source_url(raw_url: &str, crawl_host: Option<&str>) -> Result<Url, ToolError> {
    let raw_url = raw_url.trim();
    // The sample lists the ListingSource's URLs as path and query; they name the crawl host.
    let resolved = match crawl_host {
        Some(host) if raw_url.starts_with('/') && !raw_url.starts_with("//") => {
            format!("https://{host}{raw_url}")
        }
        _ => raw_url.to_owned(),
    };
    let Ok(url) = Url::parse(&resolved) else {
        return Err(ToolError::UrlOutsideReviewSource(raw_url.to_owned()));
    };
    if is_review_source_url(&url, crawl_host) {
        return Ok(url);
    }
    let mut named = url;
    // Both only fail for URLs without a host, which carry no credentials.
    let _ = named.set_username("");
    let _ = named.set_password(None);
    Err(ToolError::UrlOutsideReviewSource(named.to_string()))
}

/// Checks each of `raw_urls` with [`review_source_url`] against the review's ListingSource.
async fn review_source_urls(
    context: &ToolContext,
    review_id: CrawlerReviewId,
    raw_urls: &[&str],
) -> Result<Vec<Url>, ToolError> {
    if raw_urls.is_empty() {
        return Ok(Vec::new());
    }
    let crawl_host = review_source_crawl_host(context, review_id).await?;
    raw_urls
        .iter()
        .map(|raw_url| review_source_url(raw_url, crawl_host.as_deref()))
        .collect()
}

/// The console's final answer to a request it may answer with `202` while it runs in the
/// background: the answer itself, or the first answer of `poll_path` that is no `202`, polled
/// for at most the context's wait.
async fn settled(
    context: &ToolContext,
    answer: ConsoleAnswer,
    poll_path: &str,
    what: &'static str,
) -> Result<ConsoleAnswer, ToolError> {
    let mut answer = answer;
    let started = Instant::now();
    while answer.status == 202 {
        if started.elapsed() >= context.polling.max_wait {
            return Err(ToolError::StillRunning(what));
        }
        tokio::time::sleep(context.polling.interval).await;
        answer = context.console.get(poll_path).await?;
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(role: AgentRole) -> Vec<String> {
        tools_for(role)
            .unwrap_or_else(|error| panic!("{error}"))
            .iter()
            .map(|tool| tool.name().to_owned())
            .collect()
    }

    #[test]
    fn should_register_only_each_role_s_own_tools() {
        let shared = [
            "list_pending_reviews",
            "get_review",
            "inspect_page",
            "refresh_matrix",
            "check_approval",
            "mark_needs_repair",
        ];
        let author = [
            "evaluate_candidate",
            "update_candidate",
            "update_schema_field",
            "add_review_page",
            "remove_review_page",
            "mark_page_not_product",
            "declare_removed_page",
            "record_no_sold_page",
            "label_sample_urls",
        ];
        let reviewer = ["approve", "reject"];

        assert_eq!(
            names(AgentRole::Author),
            shared
                .iter()
                .chain(&author)
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            names(AgentRole::Reviewer),
            shared
                .iter()
                .chain(&reviewer)
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn should_tell_the_tools_that_work_on_one_review_by_their_required_review_id() {
        for role in [AgentRole::Author, AgentRole::Reviewer] {
            for tool in tools_for(role).unwrap_or_else(|error| panic!("{error}")) {
                assert_eq!(
                    tool.takes_review(),
                    tool.name() != "list_pending_reviews",
                    "{}",
                    tool.name()
                );
            }
        }
    }

    #[test]
    fn should_describe_every_tool_with_object_schemas_and_annotations() {
        for role in [AgentRole::Author, AgentRole::Reviewer] {
            for definition in tools_for(role).unwrap_or_else(|error| panic!("{error}")) {
                let tool = definition.tool();
                assert!(
                    tool.description
                        .as_deref()
                        .is_some_and(|text| text.len() > 80),
                    "{}",
                    tool.name
                );
                assert_eq!(tool.input_schema.get("type"), Some(&Value::from("object")));
                assert_eq!(
                    tool.output_schema
                        .as_ref()
                        .and_then(|schema| schema.get("type").cloned()),
                    Some(Value::from("object")),
                    "{}",
                    tool.name
                );
                let annotations = tool.annotations.clone().unwrap_or_default();
                assert_eq!(annotations.open_world_hint, Some(false), "{}", tool.name);
            }
        }
    }

    #[test]
    fn should_take_only_urls_of_the_review_source() {
        let crawl_host = Some("antiques.example");

        for url in [
            "https://antiques.example/item/1",
            " http://www.antiques.example/shop?page=2 ",
        ] {
            assert!(review_source_url(url, crawl_host).is_ok(), "{url}");
        }
        assert_eq!(
            review_source_url("/item/1?page=2", crawl_host)
                .map(|url| url.to_string())
                .ok()
                .as_deref(),
            Some("https://antiques.example/item/1?page=2"),
            "a sample URL as the author reads it"
        );
        for url in [
            "https://elsewhere.example/item/1",
            "https://antiques.example.evil.example/item/1",
            "https://user:secret@antiques.example/item/1",
            "https://antiques.example:8443/item/1",
            "file:///etc/passwd",
            "//elsewhere.example/item/1",
        ] {
            assert!(
                matches!(
                    review_source_url(url, crawl_host),
                    Err(ToolError::UrlOutsideReviewSource(_))
                ),
                "{url}"
            );
        }
    }

    fn description_texts(schema: &Value) -> usize {
        match schema {
            Value::Object(object) => {
                usize::from(object.get("description").is_some_and(Value::is_string))
                    + object.values().map(description_texts).sum::<usize>()
            }
            Value::Array(items) => items.iter().map(description_texts).sum(),
            _ => 0,
        }
    }

    #[test]
    fn should_list_only_the_tools_that_work_on_the_review_s_artifact_type() {
        let names = |role, artifact_type| {
            tools_for_artifact(role, artifact_type)
                .unwrap_or_else(|error| panic!("{error}"))
                .iter()
                .map(|tool| tool.name().to_owned())
                .collect::<Vec<_>>()
        };

        assert_eq!(
            names(AgentRole::Author, ARTIFACT_URL_PATTERN),
            [
                "list_pending_reviews",
                "get_review",
                "check_approval",
                "mark_needs_repair",
                "update_candidate",
                "label_sample_urls",
            ]
        );
        assert_eq!(
            names(AgentRole::Author, ARTIFACT_PRODUCT_SCHEMA),
            [
                "list_pending_reviews",
                "get_review",
                "inspect_page",
                "refresh_matrix",
                "check_approval",
                "mark_needs_repair",
                "evaluate_candidate",
                "update_candidate",
                "update_schema_field",
                "add_review_page",
                "remove_review_page",
                "mark_page_not_product",
                "declare_removed_page",
                "record_no_sold_page",
            ]
        );
        assert_eq!(
            names(AgentRole::Reviewer, ARTIFACT_URL_PATTERN),
            [
                "list_pending_reviews",
                "get_review",
                "check_approval",
                "mark_needs_repair",
                "approve",
                "reject",
            ]
        );
        assert_eq!(
            names(AgentRole::Author, "OTHER"),
            [
                "list_pending_reviews",
                "get_review",
                "inspect_page",
                "check_approval",
                "mark_needs_repair",
                "update_candidate"
            ]
        );
    }

    #[test]
    fn should_narrow_update_candidate_to_the_review_s_candidate_kind() {
        let input = |artifact_type| {
            let tools = tools_for_artifact(AgentRole::Author, artifact_type)
                .unwrap_or_else(|e| panic!("{e}"));
            let tool = tools
                .iter()
                .find(|tool| tool.name() == "update_candidate")
                .unwrap_or_else(|| panic!("no update_candidate"));
            Value::Object(tool.tool().input_schema.as_ref().clone())
        };
        let whole = Value::Object(
            tools_for(AgentRole::Author)
                .unwrap_or_else(|e| panic!("{e}"))
                .iter()
                .find(|tool| tool.name() == "update_candidate")
                .unwrap_or_else(|| panic!("no update_candidate"))
                .tool()
                .input_schema
                .as_ref()
                .clone(),
        );
        let kinds = |schema: &Value| {
            schema["$defs"]["CandidateArgument"]["oneOf"]
                .as_array()
                .map_or(0, Vec::len)
        };

        let url_pattern = input(ARTIFACT_URL_PATTERN);
        let product_schema = input(ARTIFACT_PRODUCT_SCHEMA);

        assert_eq!(kinds(&whole), 2);
        assert_eq!(kinds(&url_pattern), 1);
        assert_eq!(
            url_pattern["$defs"]["CandidateArgument"]["oneOf"][0]["properties"]["kind"]["const"],
            ARTIFACT_URL_PATTERN
        );
        assert!(url_pattern["$defs"]["ProductCssSelectorSchema"].is_null());
        assert!(url_pattern["$defs"]["ExtractionRule"].is_null());
        assert!(url_pattern["$defs"]["UrlPatternDecisionArgument"].is_object());
        assert_eq!(kinds(&product_schema), 1);
        assert!(product_schema["$defs"]["ProductCssSelectorSchema"].is_object());
        assert!(product_schema["$defs"]["ExtractionRule"].is_object());
        assert!(product_schema["$defs"]["UrlPatternDecisionArgument"].is_null());
        assert!(
            url_pattern.to_string().len() * 3 < whole.to_string().len(),
            "{} of {}",
            url_pattern.to_string().len(),
            whole.to_string().len()
        );
    }

    #[test]
    fn should_document_the_schema_rules_once_in_update_candidate() {
        let tools = tools_for(AgentRole::Author).unwrap_or_else(|error| panic!("{error}"));
        let definitions = |name: &str| {
            let tool = tools
                .iter()
                .find(|tool| tool.name() == name)
                .unwrap_or_else(|| panic!("no tool {name}"));
            Value::Object(tool.tool().input_schema.as_ref().clone())["$defs"].clone()
        };

        assert!(description_texts(&definitions("update_candidate")) > 10);
        for name in ["evaluate_candidate", "update_schema_field"] {
            let definitions = definitions(name);
            assert_eq!(description_texts(&definitions), 0, "{name}");
            assert!(
                definitions["ProductCssSelectorSchema"]["properties"]["description"].is_object()
                    || name == "update_schema_field",
                "{name} lost the schema's description field"
            );
        }
    }
}
