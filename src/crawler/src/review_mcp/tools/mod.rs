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
use tracing::{info, warn};
use url::Url;

/// What the tools of one `review-mcp` process share: the console client acting with the role's
/// token, and how long a tool waits for a background evaluation.
pub struct ToolContext {
    pub(super) console: ConsoleClient,
    pub(super) polling: Polling,
}

impl ToolContext {
    pub fn new(console: ConsoleClient, polling: Polling) -> Self {
        Self { console, polling }
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

/// One tool: its MCP description with input and output schemas, and its handler.
#[derive(Clone)]
pub struct ToolDefinition {
    tool: Tool,
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
        Ok(Self { tool, handler })
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
        info!(
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

/// The domains of the review's ListingSource, the only hosts a URL given to a tool may name.
async fn review_source_domains(
    context: &ToolContext,
    review_id: CrawlerReviewId,
) -> Result<Vec<String>, ToolError> {
    let detail = review_detail(context, review_id).await?;
    let domains: Vec<wire::ListingSourceDomain> = context
        .console
        .get(&format!(
            "/api/listing-sources/{}/domains",
            detail.review.listing_source_id
        ))
        .await?
        .decode()?;
    Ok(domains.into_iter().map(|domain| domain.domain).collect())
}

/// `raw_url` when it is a plain http(s) URL on one of the review's ListingSource `domains`, as
/// the console itself requires; any other URL is refused before a request is sent.
fn review_source_url(raw_url: &str, domains: &[String]) -> Result<Url, ToolError> {
    let raw_url = raw_url.trim();
    let Ok(url) = Url::parse(raw_url) else {
        return Err(ToolError::UrlOutsideReviewSource(raw_url.to_owned()));
    };
    if is_review_source_url(&url, domains) {
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
    let domains = review_source_domains(context, review_id).await?;
    raw_urls
        .iter()
        .map(|raw_url| review_source_url(raw_url, &domains))
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
            "mark_needs_repair",
        ];
        let author = [
            "evaluate_candidate",
            "update_candidate",
            "update_schema_field",
            "add_review_page",
            "remove_review_page",
            "declare_removed_page",
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
        let domains = vec!["antiques.example".to_owned()];

        for url in [
            "https://antiques.example/item/1",
            " http://www.antiques.example/shop?page=2 ",
        ] {
            assert!(review_source_url(url, &domains).is_ok(), "{url}");
        }
        for url in [
            "https://elsewhere.example/item/1",
            "https://antiques.example.evil.example/item/1",
            "https://user:secret@antiques.example/item/1",
            "https://antiques.example:8443/item/1",
            "file:///etc/passwd",
            "/item/1",
        ] {
            assert!(
                matches!(
                    review_source_url(url, &domains),
                    Err(ToolError::UrlOutsideReviewSource(_))
                ),
                "{url}"
            );
        }
    }
}
