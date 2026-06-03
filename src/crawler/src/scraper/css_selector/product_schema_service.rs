use crate::google_llm::{GeminiRateLimiter, run_with_gemini_rate_limiter};
use crate::logging::llm_metrics;
use crate::scraper::css_selector::product_schema::{
    ApplySchemaError, ProductCssSelectorSchema, ShopsProductSchema,
};
use crate::scraper::css_selector::product_schema_repository::ShopsProductSchemaRepository;
use common::logging::{GeminiServiceTier, LlmModel, LlmOperation, LlmProvider, log_llm_invocation};
use common::shop_id::ShopId;
use kuchiki::traits::*;
use kuchiki::{NodeRef, parse_html};
use llm::{
    chat::{ChatMessage, ChatProvider},
    error::LLMError,
};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;
use time::OffsetDateTime;
use tracing::{debug, info};

#[derive(Debug, thiserror::Error)]
pub enum ProductSchemaServiceError {
    #[error("LLM error: {0}")]
    LLMError(#[from] LLMError),

    #[error("NoTextResponse: {0}")]
    NoTextResponse(String),

    #[error("JsonParsingTargetSchemaError: {0}")]
    JsonParsingTargetSchemaError(serde_json::Error),

    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SchemaLlmEvaluationDecision {
    Approve,
    NeedsHumanReview,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SchemaLlmEvaluationConfidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SchemaLlmPageFinding {
    pub role: String,
    pub schema_index: Option<usize>,
    pub finding: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SchemaLlmEvaluation {
    pub decision: SchemaLlmEvaluationDecision,
    pub confidence: SchemaLlmEvaluationConfidence,
    #[serde(default)]
    pub approved_by_llm: bool,
    pub summary: String,
    #[serde(default)]
    pub risks: Vec<String>,
    #[serde(default)]
    pub page_findings: Vec<SchemaLlmPageFinding>,
}

impl SchemaLlmEvaluation {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            decision: SchemaLlmEvaluationDecision::NeedsHumanReview,
            confidence: SchemaLlmEvaluationConfidence::Low,
            approved_by_llm: false,
            summary: reason.clone(),
            risks: vec![reason],
            page_findings: Vec::new(),
        }
    }

    pub fn is_high_confidence_approval(&self) -> bool {
        self.decision == SchemaLlmEvaluationDecision::Approve
            && self.confidence == SchemaLlmEvaluationConfidence::High
    }

    pub fn with_approved_by_llm(mut self, approved_by_llm: bool) -> Self {
        self.approved_by_llm = approved_by_llm;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedProductSchemas {
    pub schemas: Vec<ProductCssSelectorSchema>,
    pub evaluation: SchemaLlmEvaluation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
struct ProductSchemaGenerationResponse {
    pub schemas: Vec<ProductCssSelectorSchema>,
    pub confidence: SchemaLlmEvaluationConfidence,
    pub summary: String,
    #[serde(default)]
    pub risks: Vec<String>,
    #[serde(default)]
    pub page_findings: Vec<SchemaLlmPageFinding>,
}

impl ProductSchemaGenerationResponse {
    fn into_generated(self) -> GeneratedProductSchemas {
        let decision = if self.confidence == SchemaLlmEvaluationConfidence::High {
            SchemaLlmEvaluationDecision::Approve
        } else {
            SchemaLlmEvaluationDecision::NeedsHumanReview
        };

        GeneratedProductSchemas {
            schemas: self.schemas,
            evaluation: SchemaLlmEvaluation {
                decision,
                confidence: self.confidence,
                approved_by_llm: false,
                summary: self.summary,
                risks: self.risks,
                page_findings: self.page_findings,
            },
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait ProductSchemaService {
    async fn create_product_schema(
        &self,
        html_pages: &[String],
    ) -> Result<ProductCssSelectorSchema, ProductSchemaServiceError>;

    async fn create_product_schemas(
        &self,
        html_pages: &[String],
    ) -> Result<GeneratedProductSchemas, ProductSchemaServiceError>;

    /// Generate a single schema from a single HTML page and append it to the
    /// cached schema set. Used when a runtime schema-variant match fails to
    /// dynamically expand the schema set without full regeneration.
    async fn append_single_schema(
        &self,
        html: &str,
        failed_schema: Option<&ProductCssSelectorSchema>,
        last_error: Option<&ApplySchemaError>,
    ) -> Result<GeneratedProductSchemas, ProductSchemaServiceError>;

    async fn find_product_schema(
        &self,
        shop_id: &ShopId,
    ) -> Result<Option<ShopsProductSchema>, ProductSchemaServiceError>;

    async fn save_product_schema(
        &self,
        shop_id: &ShopId,
        product_schema: ProductCssSelectorSchema,
    ) -> Result<ShopsProductSchema, ProductSchemaServiceError>;

    async fn save_product_schemas(
        &self,
        shop_id: &ShopId,
        product_schemas: Vec<ProductCssSelectorSchema>,
    ) -> Result<ShopsProductSchema, ProductSchemaServiceError>;

    async fn get_product_schema(
        &self,
        shop_id: &ShopId,
        html_pages: &[String],
    ) -> Result<ShopsProductSchema, ProductSchemaServiceError>;
}

pub struct ProductSchemaServiceImpl {
    llm: Box<dyn ChatProvider>,
    rate_limiter: Option<Arc<GeminiRateLimiter>>,
    service_tier: Option<GeminiServiceTier>,
    repository: Box<dyn ShopsProductSchemaRepository + Send + Sync>,
}

impl ProductSchemaServiceImpl {
    pub fn new(
        llm: llm::builder::LLMBuilder,
        service_tier: Option<GeminiServiceTier>,
        repository: Box<dyn ShopsProductSchemaRepository + Send + Sync>,
        rate_limiter: Option<Arc<GeminiRateLimiter>>,
    ) -> Result<Self, LLMError> {
        let llm = build_schema_generation_llm(llm)?;
        Ok(Self {
            llm,
            rate_limiter,
            service_tier,
            repository,
        })
    }
}

fn build_schema_generation_llm(
    llm: llm::builder::LLMBuilder,
) -> Result<Box<dyn ChatProvider>, LLMError> {
    let schema = serde_json::to_string_pretty(&schema_for!(ProductCssSelectorSchema))
        .unwrap_or_else(|_| "Failed to generate schema".to_string());
    let response_schema =
        serde_json::to_string_pretty(&schema_for!(ProductSchemaGenerationResponse))
            .unwrap_or_else(|_| "Failed to generate response schema".to_string());
    let system_prompt = format!(
        "You are an e-commerce scraper-assistant for antiques creating extraction-schemas for HTML given product-pages.
            Return only JSON matching ProductSchemaGenerationResponse.
            The response must include schemas plus confidence LOW, MEDIUM, or HIGH.
            HIGH means the selectors are product-specific, deterministic, and safe to auto-approve when local validation passes.
            MEDIUM means the schema is plausible but needs human review. LOW means uncertain or weak selectors and needs human review.
            ProductCssSelectorSchema schema:\n\n {schema}\n\n
            ProductSchemaGenerationResponse schema:\n\n {response_schema}",
    );
    let llm = llm
        .system(system_prompt)
        .openai_enable_web_search(false)
        .reasoning(true)
        .timeout_seconds(180)
        .validator(|res| {
            parse_product_schemas_response(strip_markdown_json_embedding(res))
                .map(|_| ())
                .map_err(|err| err.to_string())
        })
        .validator_attempts(3)
        .build()?;
    let llm: Box<dyn ChatProvider> = llm;
    Ok(llm)
}

fn parse_product_schemas_response(raw: &str) -> Result<GeneratedProductSchemas, serde_json::Error> {
    let response = serde_json::from_str::<ProductSchemaGenerationResponse>(raw)?;
    if response.schemas.is_empty() {
        return Err(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "LLM produced zero schemas",
        )));
    }
    Ok(response.into_generated())
}

#[async_trait::async_trait]
impl ProductSchemaService for ProductSchemaServiceImpl {
    async fn create_product_schema(
        &self,
        html_pages: &[String],
    ) -> Result<ProductCssSelectorSchema, ProductSchemaServiceError> {
        let generated = self.create_product_schemas(html_pages).await?;
        generated.schemas.first().cloned().ok_or_else(|| {
            ProductSchemaServiceError::NoTextResponse("LLM produced zero schemas".to_string())
        })
    }

    #[tracing::instrument(skip(self, html_pages), fields(html_pages = html_pages.len()))]
    async fn create_product_schemas(
        &self,
        html_pages: &[String],
    ) -> Result<GeneratedProductSchemas, ProductSchemaServiceError> {
        log_schema_prompt_size_from_raw_pages(
            LlmOperation::CrawlerProductSchemaGeneration,
            html_pages,
        );
        let instruction = build_create_schemas_instruction(html_pages);
        let message = ChatMessage::user().content(instruction).build();
        let messages = vec![message];

        let started_at = Instant::now();
        let response =
            run_with_gemini_rate_limiter(&*self.llm, self.rate_limiter.as_deref(), &messages)
                .await?;
        log_llm_invocation(
            LlmOperation::CrawlerProductSchemaGeneration,
            LlmProvider::Google,
            LlmModel::Configured,
            started_at.elapsed(),
            llm_metrics(response.usage(), Some(html_pages.len()), self.service_tier),
        );
        let res = response.text().ok_or_else(|| {
            ProductSchemaServiceError::NoTextResponse("Expected text response".to_string())
        })?;

        let parsed = strip_markdown_json_embedding(&res);
        let generated = parse_product_schemas_response(parsed)
            .map_err(ProductSchemaServiceError::JsonParsingTargetSchemaError)?;
        if generated.schemas.is_empty() {
            return Err(ProductSchemaServiceError::NoTextResponse(
                "LLM produced zero schemas".to_string(),
            ));
        }

        debug!(
            schemas_count = generated.schemas.len(),
            html_pages = html_pages.len(),
            confidence = ?generated.evaluation.confidence,
            "LLM created product CSS selector schemas"
        );
        Ok(generated)
    }

    #[tracing::instrument(
        name = "scraper_append_single_schema",
        skip(self, html, failed_schema, last_error)
    )]
    async fn append_single_schema(
        &self,
        html: &str,
        failed_schema: Option<&ProductCssSelectorSchema>,
        last_error: Option<&ApplySchemaError>,
    ) -> Result<GeneratedProductSchemas, ProductSchemaServiceError> {
        // Generate single schema for this HTML page
        log_schema_prompt_size_from_raw_pages(
            LlmOperation::CrawlerProductSchemaRepair,
            &[html.to_string()],
        );
        let instruction = build_append_schema_instruction(html, failed_schema, last_error);
        let message = ChatMessage::user().content(instruction).build();
        let messages = vec![message];

        let started_at = Instant::now();
        let response =
            run_with_gemini_rate_limiter(&*self.llm, self.rate_limiter.as_deref(), &messages)
                .await?;
        log_llm_invocation(
            LlmOperation::CrawlerProductSchemaRepair,
            LlmProvider::Google,
            LlmModel::Configured,
            started_at.elapsed(),
            llm_metrics(response.usage(), Some(1), self.service_tier),
        );
        let res = response.text().ok_or_else(|| {
            ProductSchemaServiceError::NoTextResponse("Expected text response".to_string())
        })?;

        let parsed = strip_markdown_json_embedding(&res);
        let generated = parse_product_schemas_response(parsed)
            .map_err(ProductSchemaServiceError::JsonParsingTargetSchemaError)?;
        if generated.schemas.is_empty() {
            return Err(ProductSchemaServiceError::NoTextResponse(
                "LLM produced zero schemas".to_string(),
            ));
        }
        if generated.schemas.len() != 1 {
            return Err(ProductSchemaServiceError::NoTextResponse(format!(
                "Expected exactly one schema for append generation, got {}",
                generated.schemas.len()
            )));
        }

        info!(
            schema_count = generated.schemas.len(),
            confidence = ?generated.evaluation.confidence,
            "Generated schema response for append-and-retry"
        );
        Ok(generated)
    }

    async fn find_product_schema(
        &self,
        shop_id: &ShopId,
    ) -> Result<Option<ShopsProductSchema>, ProductSchemaServiceError> {
        self.repository
            .find_product_schema(shop_id)
            .await
            .map_err(ProductSchemaServiceError::DatabaseError)
    }

    async fn save_product_schema(
        &self,
        shop_id: &ShopId,
        product_schema: ProductCssSelectorSchema,
    ) -> Result<ShopsProductSchema, ProductSchemaServiceError> {
        self.save_product_schemas(shop_id, vec![product_schema])
            .await
    }

    #[tracing::instrument(
        name = "scraper_save_product_schemas",
        skip(self, product_schemas),
        fields(shop_id = %shop_id, schema_count = product_schemas.len())
    )]
    async fn save_product_schemas(
        &self,
        shop_id: &ShopId,
        product_schemas: Vec<ProductCssSelectorSchema>,
    ) -> Result<ShopsProductSchema, ProductSchemaServiceError> {
        if product_schemas.is_empty() {
            return Err(ProductSchemaServiceError::NoTextResponse(
                "LLM produced zero schemas".to_string(),
            ));
        }

        let existing = self.repository.find_product_schema(shop_id).await?;

        match existing {
            Some(_) => {
                info!("Updating existing product schema");
                self.repository
                    .update_product_schema(shop_id, &product_schemas)
                    .await
                    .map_err(ProductSchemaServiceError::DatabaseError)
            }
            None => {
                info!("Inserting new product schema");
                let now = OffsetDateTime::now_utc();
                let schema = ShopsProductSchema {
                    shop_id: *shop_id,
                    product_schemas,
                    created: now,
                    updated: now,
                };
                self.repository
                    .insert_product_schema(shop_id, &schema)
                    .await
                    .map_err(ProductSchemaServiceError::DatabaseError)
            }
        }
    }

    #[tracing::instrument(
        skip(self, html_pages),
        fields(shop_id = %shop_id, html_pages = html_pages.len())
    )]
    async fn get_product_schema(
        &self,
        shop_id: &ShopId,
        html_pages: &[String],
    ) -> Result<ShopsProductSchema, ProductSchemaServiceError> {
        if let Some(existing) = self.find_product_schema(shop_id).await? {
            debug!("Found existing product schema");
            return Ok(existing);
        }

        info!("No product schema found for shop, creating via LLM");
        let generated = self.create_product_schemas(html_pages).await?;
        self.save_product_schemas(shop_id, generated.schemas).await
    }
}

fn build_create_schemas_instruction(html_pages: &[String]) -> String {
    let prompt_pages: Vec<String> = if html_pages.is_empty() {
        Vec::new()
    } else {
        html_pages
            .iter()
            .map(|html| html_to_schema_prompt_dsl(html))
            .collect()
    };

    if prompt_pages.is_empty() {
        return String::from(
            "Generate robust Extraction-Schemas for the given HTML product pages. Return ONLY ProductSchemaGenerationResponse JSON with schemas plus confidence LOW, MEDIUM, or HIGH.",
        );
    }

    let mut samples = String::new();
    for (idx, prompt_page) in prompt_pages.iter().enumerate() {
        let _ = std::fmt::Write::write_fmt(
            &mut samples,
            format_args!(
                "\n--- SAMPLE {sample_idx} YAML ---\n{page_dsl}\n",
                sample_idx = idx + 1,
                page_dsl = prompt_page
            ),
        );
    }

    let template_instruction = if prompt_pages.len() > 1 {
        "Infer the distinct product-page templates represented by these samples. Return one schema per distinct template, not one schema per page. If all samples clearly share the same template, return one schema; otherwise return multiple schemas so every template has precise selectors.\n"
    } else {
        "Return one schema for the single observed product-page template.\n"
    };

    format!(
        "Generate robust Extraction-Schemas that together cover these product page HTML samples from the same shop.\n\
         {template_instruction}\
         Shops often have multiple templates/layouts. Do not collapse different templates into one overly broad schema just because fields share names.\n\
         A schema may target only the subset of samples where its selectors are precise and product-specific.\n\
         A schema applies to a sample only when every non-null extraction rule in that schema exists in that sample HTML and extracts successfully.\n\
         Optional fields are optional only when the field is null for that schema because the field is not applicable to that schema's own product template.\n\
         Never omit an applicable field from one product template just to make one broad schema also work for another template.\n\
         If an applicable field differs by template, availability state, layout, DOM presence, or selector, split the samples into multiple schemas and preserve the applicable rules in each schema.\n\
         One schema is valid only when all applicable fields and every non-null selector apply across all samples that schema covers.\n\
         Return schemas ordered by specificity and completeness: first the schema with the most non-null extraction rules, then fallback templates with fewer applicable rules. When rule counts tie, put the schema with more specific product-focused selectors first.\n\
         Examples: if template A has price and template B has no price element, generate two schemas and put the priced schema first. If an auction template has estimate fields and a buy-now template has fixed price, generate separate schemas ordered by rule count. If a sold-item template lacks buy price but has sold state, split schemas when selectors differ.\n\
         Prefer high-precision selectors that represent semantic fields rather than layout wrappers.\n\
         Return ONLY ProductSchemaGenerationResponse JSON with fields schemas, confidence, summary, risks, and page_findings. The schemas field contains one ProductCssSelectorSchema for one product template or multiple schemas ordered as described above.\n\
         Use confidence HIGH only when selectors are product-specific and likely safe for unattended approval after deterministic validation. Use MEDIUM for plausible schemas with ambiguity. Use LOW when selectors or fields are uncertain. MEDIUM and LOW require human review.\n\
         The samples below are compact YAML projections of the original HTML. Derive CSS selectors from the tags, attrs, text, and tree context, and target the original raw HTML.\n\
         Here are the compact page YAML samples:{samples}"
    )
}

fn build_append_schema_instruction(
    html: &str,
    failed_schema: Option<&ProductCssSelectorSchema>,
    last_error: Option<&ApplySchemaError>,
) -> String {
    let page_dsl = html_to_schema_prompt_dsl(html);
    let failure_context = match (failed_schema, last_error) {
        (Some(schema), Some(error)) => {
            let schema_json = serde_json::to_string_pretty(schema)
                .unwrap_or_else(|_| "<failed to serialize previous schema>".to_string());
            format!(
                "\nPrevious attempt failed. Here is the schema that just failed:\n{schema_json}\n\
                 Extraction failure observed:\n{error}\n\
                 Improve/fix the failed schema for this page instead of repeating the same selectors."
            )
        }
        _ => String::new(),
    };

    format!(
        "Generate a single robust Extraction-Schema for the following product page HTML.\n\
          This schema will be appended to a set of existing schemas from the same shop.\n\
          Focus on this specific layout and make the selectors resilient.\n\
          Return ONLY ProductSchemaGenerationResponse JSON. The schemas field must contain exactly one ProductCssSelectorSchema object for this page.\n\
          Use confidence HIGH only when selectors are product-specific and likely safe for unattended approval after deterministic validation. Use MEDIUM for plausible schemas with ambiguity. Use LOW when selectors or fields are uncertain. MEDIUM and LOW require human review.\n\
          Optional fields may remain null if not confidently present.\n\
          {failure_context}\n\
          The sample below is a compact YAML projection of the original HTML. Derive CSS selectors from the tags, attrs, text, and tree context, and target the original raw HTML.\n\
          Here is the compact page YAML:\n\
          {page_dsl}"
    )
}

#[derive(Debug, Default)]
struct SchemaPromptSizeTotals {
    raw_html_bytes: usize,
    cleaned_html_bytes: usize,
    yaml_bytes: usize,
}

impl SchemaPromptSizeTotals {
    fn add(&mut self, raw_html: &str, cleaned_html: &str, yaml: &str) {
        self.raw_html_bytes += raw_html.len();
        self.cleaned_html_bytes += cleaned_html.len();
        self.yaml_bytes += yaml.len();
    }
}

fn log_schema_prompt_size_from_raw_pages(operation: LlmOperation, html_pages: &[String]) {
    let mut totals = SchemaPromptSizeTotals::default();
    for html in html_pages {
        let cleaned_html = clean_html_for_schema_generation(html);
        let yaml = html_to_schema_prompt_dsl(html);
        totals.add(html, &cleaned_html, &yaml);
    }

    log_schema_prompt_size(operation, html_pages.len(), totals);
}

fn log_schema_prompt_size(
    operation: LlmOperation,
    page_count: usize,
    totals: SchemaPromptSizeTotals,
) {
    info!(
        llmOperation = %operation,
        page_count,
        raw_html_bytes = totals.raw_html_bytes,
        cleaned_html_bytes = totals.cleaned_html_bytes,
        yaml_bytes = totals.yaml_bytes,
        raw_html_tokens = approx_prompt_tokens(totals.raw_html_bytes),
        cleaned_html_tokens = approx_prompt_tokens(totals.cleaned_html_bytes),
        yaml_tokens = approx_prompt_tokens(totals.yaml_bytes),
        yaml_vs_cleaned_percent = percent(totals.yaml_bytes, totals.cleaned_html_bytes),
        "Schema prompt source size summary"
    );
}

fn approx_prompt_tokens(chars: usize) -> usize {
    chars / 4
}

fn percent(part: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        (part as f64 / total as f64) * 100.0
    }
}

pub fn strip_markdown_json_embedding(s: &str) -> &str {
    s.trim()
        .strip_prefix("```json")
        .unwrap_or(s)
        .strip_suffix("```")
        .unwrap_or(s)
}

pub fn clean_html_for_schema_generation(input: &str) -> String {
    let document = parse_html().one(input);

    // Tags to remove entirely
    let remove_selectors = [
        "script", "style", "noscript", "svg", "canvas", "header", "footer", "nav", "aside",
    ];

    for selector in &remove_selectors {
        if let Ok(nodes) = document.select(selector) {
            for node in nodes {
                node.as_node().detach();
            }
        }
    }

    remove_comments(&document);
    strip_attributes(&document);
    let mut cleaned = Vec::new();
    document.serialize(&mut cleaned).unwrap();
    String::from_utf8(cleaned).unwrap_or_default()
}

const DSL_TEXT_LIMIT: usize = 180;
const DSL_ATTR_LIMIT: usize = 250;
const DSL_NODE_LIMIT: usize = 2_000;
const REMOVED_DSL_TAGS: &[&str] = &[
    "script", "style", "noscript", "svg", "canvas", "header", "footer", "nav", "aside",
];

#[derive(Debug, Default, Serialize)]
struct PageDslRoot {
    page_dsl: PageDsl,
}

#[derive(Debug, Default, Serialize)]
struct PageDsl {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    nodes: Vec<HtmlYamlNode>,
}

#[derive(Debug, Serialize)]
struct HtmlYamlNode {
    tag: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    attrs: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<HtmlYamlNode>,
}

pub fn html_to_schema_prompt_dsl(input: &str) -> String {
    let document = parse_html().one(input);

    for selector in REMOVED_DSL_TAGS {
        if let Ok(nodes) = document.select(selector) {
            for node in nodes {
                node.as_node().detach();
            }
        }
    }
    remove_comments(&document);

    let mut projected_count = 0;
    let page = PageDsl {
        nodes: project_children(&document, &mut projected_count),
    };

    yaml_serde::to_string(&PageDslRoot { page_dsl: page })
        .unwrap_or_else(|_| "page_dsl: {}\n".to_string())
}

fn project_children(node: &NodeRef, projected_count: &mut usize) -> Vec<HtmlYamlNode> {
    let mut projected = Vec::new();
    for child in node.children() {
        if *projected_count >= DSL_NODE_LIMIT {
            break;
        }
        projected.extend(project_node(&child, projected_count));
    }
    projected
}

fn project_node(node: &NodeRef, projected_count: &mut usize) -> Vec<HtmlYamlNode> {
    let Some(element) = node.as_element() else {
        return Vec::new();
    };

    let tag = element.name.local.to_string();
    if REMOVED_DSL_TAGS.contains(&tag.as_str()) {
        return Vec::new();
    }

    let attrs = {
        let attrs = element.attributes.borrow();
        projected_attrs(&attrs)
    };
    let text = direct_text(node).map(|text| truncate_chars(&text, DSL_TEXT_LIMIT));
    let children = project_children(node, projected_count);

    if attrs.is_empty() && text.is_none() && children.is_empty() {
        return Vec::new();
    }

    if is_collapsible_wrapper(&tag) && text.is_none() && is_layout_only_attrs(&attrs) {
        return children;
    }

    *projected_count += 1;
    vec![HtmlYamlNode {
        tag,
        attrs,
        text,
        children,
    }]
}

fn is_collapsible_wrapper(tag: &str) -> bool {
    matches!(
        tag,
        "html" | "head" | "body" | "main" | "div" | "span" | "section" | "article"
    )
}

fn is_layout_only_attrs(attrs: &BTreeMap<String, String>) -> bool {
    attrs.is_empty() || (attrs.len() == 1 && attrs.contains_key("class"))
}

fn projected_attrs(attrs: &kuchiki::Attributes) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    for name in PROJECTED_ATTRS {
        if let Some(value) = attrs.get(*name).filter(|value| !value.trim().is_empty()) {
            result.insert((*name).to_string(), truncate_chars(value, DSL_ATTR_LIMIT));
        }
    }
    result
}

const PROJECTED_ATTRS: &[&str] = &[
    "id",
    "class",
    "itemprop",
    "property",
    "name",
    "content",
    "value",
    "type",
    "src",
    "srcset",
    "alt",
    "title",
    "datetime",
    "data-lazy",
    "data-lazy-src",
    "data-src",
    "data-large_image",
    "data-full",
    "data-original",
    "data-zoom-image",
    "data-testid",
    "data-test",
    "data-cy",
];

fn normalize_text(raw: &str) -> Option<String> {
    let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() { None } else { Some(text) }
}

fn direct_text(node: &NodeRef) -> Option<String> {
    let mut text = String::new();
    for child in node.children() {
        if let Some(contents) = child.as_text() {
            text.push_str(&contents.borrow());
            text.push(' ');
        }
    }
    normalize_text(&text)
}

fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }

    let mut truncated = value.chars().take(limit).collect::<String>();
    truncated.push('…');
    truncated
}

fn remove_comments(node: &NodeRef) {
    for child in node.children() {
        if child.as_comment().is_some() {
            child.detach();
        } else {
            remove_comments(&child);
        }
    }
}

fn strip_attributes(document: &NodeRef) {
    let deny_prefixes = ["on"]; // onclick, onload, etc.

    let deny_exact = [
        "style",
        "integrity",
        "crossorigin",
        "referrerpolicy",
        "nonce",
        "tabindex",
        "width",
        "height",
        "loading",
        "decoding",
    ];

    for css_match in document.select("*").unwrap() {
        let mut attributes = css_match.attributes.borrow_mut();

        attributes.map.retain(|key, _| {
            let name = key.local.as_ref();

            // Remove JS event handlers
            if deny_prefixes.iter().any(|prefix| name.starts_with(prefix)) {
                return false;
            }

            // Remove known useless attributes
            if deny_exact.contains(&name) {
                return false;
            }

            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::css_selector::product_schema_repository::MockShopsProductSchemaRepository;
    use crate::scraper::css_selector::rule::{
        ExtractionCardinality, ExtractionKind, ExtractionRule,
    };
    use common::shop_id::ShopId;
    use time::OffsetDateTime;

    fn sample_css_schema() -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            shops_product_id: ExtractionRule {
                selector: "span.product-id".into(),
                additional_selectors: vec![],
                extract: ExtractionKind::Text,
                cardinality: ExtractionCardinality::First,
            },
            title: ExtractionRule {
                selector: "h1.title".into(),
                additional_selectors: vec![],
                extract: ExtractionKind::Text,
                cardinality: ExtractionCardinality::First,
            },
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            seller_name: None,
            state: ExtractionRule {
                selector: "span.state".into(),
                additional_selectors: vec![],
                extract: ExtractionKind::Text,
                cardinality: ExtractionCardinality::First,
            },
            images: ExtractionRule {
                selector: "img.product-image".into(),
                additional_selectors: vec![],
                extract: ExtractionKind::Attribute { name: "src".into() },
                cardinality: ExtractionCardinality::All,
            },
            auction_start: None,
            auction_end: None,
            default_currency: None,
        }
    }

    fn sample_shops_product_schema(shop_id: ShopId) -> ShopsProductSchema {
        let now = OffsetDateTime::now_utc();
        ShopsProductSchema {
            shop_id,
            product_schemas: vec![sample_css_schema()],
            created: now,
            updated: now,
        }
    }

    fn generated_response_json(schemas: Vec<ProductCssSelectorSchema>) -> String {
        serde_json::to_string(&serde_json::json!({
            "schemas": schemas,
            "confidence": "HIGH",
            "summary": "Selectors are product-specific.",
            "risks": [],
            "page_findings": [],
        }))
        .expect("generated response should serialize")
    }

    // -----------------------------------------------------------------------
    // find_product_schema
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_return_schema_when_found_in_repository_for_find() {
        let shop_id = ShopId::new();
        let expected = sample_shops_product_schema(shop_id);
        let expected_clone = expected.clone();

        let mut repository = MockShopsProductSchemaRepository::new();
        repository
            .expect_find_product_schema()
            .withf(move |id| *id == shop_id)
            .return_once(move |_| Box::pin(async move { Ok(Some(expected_clone)) }));

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let result = service.find_product_schema(&shop_id).await.unwrap();
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.shop_id, expected.shop_id);
        assert_eq!(result.product_schemas, expected.product_schemas);
    }

    #[tokio::test]
    async fn should_return_none_when_not_found_in_repository_for_find() {
        let shop_id = ShopId::new();

        let mut repository = MockShopsProductSchemaRepository::new();
        repository
            .expect_find_product_schema()
            .return_once(|_| Box::pin(async { Ok(None) }));

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let result = service.find_product_schema(&shop_id).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn should_propagate_database_error_for_find() {
        let shop_id = ShopId::new();

        let mut repository = MockShopsProductSchemaRepository::new();
        repository
            .expect_find_product_schema()
            .return_once(|_| Box::pin(async { Err(sqlx::Error::RowNotFound) }));

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let result = service.find_product_schema(&shop_id).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProductSchemaServiceError::DatabaseError(_)
        ));
    }

    // -----------------------------------------------------------------------
    // save_product_schema
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_insert_schema_when_not_existing_for_save() {
        let shop_id = ShopId::new();
        let css_schema = sample_css_schema();
        let expected = sample_shops_product_schema(shop_id);
        let expected_clone = expected.clone();

        let mut repository = MockShopsProductSchemaRepository::new();
        repository
            .expect_find_product_schema()
            .return_once(|_| Box::pin(async { Ok(None) }));
        repository
            .expect_insert_product_schema()
            .withf(move |id, _schema| *id == shop_id)
            .return_once(move |_, _| Box::pin(async move { Ok(expected_clone) }));

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let result = service
            .save_product_schema(&shop_id, css_schema)
            .await
            .unwrap();
        assert_eq!(result.shop_id, expected.shop_id);
        assert_eq!(result.product_schemas, expected.product_schemas);
    }

    #[tokio::test]
    async fn should_update_schema_when_already_existing_for_save() {
        let shop_id = ShopId::new();
        let existing = sample_shops_product_schema(shop_id);
        let existing_clone = existing.clone();
        let css_schema = sample_css_schema();
        let updated = sample_shops_product_schema(shop_id);
        let updated_clone = updated.clone();

        let mut repository = MockShopsProductSchemaRepository::new();
        repository
            .expect_find_product_schema()
            .return_once(move |_| Box::pin(async move { Ok(Some(existing_clone)) }));
        repository
            .expect_update_product_schema()
            .withf(move |id, _schema| *id == shop_id)
            .return_once(move |_, _| Box::pin(async move { Ok(updated_clone) }));

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let result = service
            .save_product_schema(&shop_id, css_schema)
            .await
            .unwrap();
        assert_eq!(result.shop_id, updated.shop_id);
    }

    #[tokio::test]
    async fn should_propagate_database_error_when_find_fails_for_save() {
        let shop_id = ShopId::new();
        let css_schema = sample_css_schema();

        let mut repository = MockShopsProductSchemaRepository::new();
        repository
            .expect_find_product_schema()
            .return_once(|_| Box::pin(async { Err(sqlx::Error::RowNotFound) }));

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let result = service.save_product_schema(&shop_id, css_schema).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProductSchemaServiceError::DatabaseError(_)
        ));
    }

    #[tokio::test]
    async fn should_propagate_database_error_when_insert_fails_for_save() {
        let shop_id = ShopId::new();
        let css_schema = sample_css_schema();

        let mut repository = MockShopsProductSchemaRepository::new();
        repository
            .expect_find_product_schema()
            .return_once(|_| Box::pin(async { Ok(None) }));
        repository
            .expect_insert_product_schema()
            .return_once(|_, _| Box::pin(async { Err(sqlx::Error::RowNotFound) }));

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let result = service.save_product_schema(&shop_id, css_schema).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProductSchemaServiceError::DatabaseError(_)
        ));
    }

    // -----------------------------------------------------------------------
    // get_product_schema
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_return_existing_schema_without_llm_call_for_get() {
        let shop_id = ShopId::new();
        let existing = sample_shops_product_schema(shop_id);
        let existing_clone = existing.clone();

        let mut repository = MockShopsProductSchemaRepository::new();
        repository
            .expect_find_product_schema()
            .return_once(move |_| Box::pin(async move { Ok(Some(existing_clone)) }));
        // insert and update should NOT be called
        repository.expect_insert_product_schema().never();
        repository.expect_update_product_schema().never();

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let html_pages = vec!["<html></html>".to_string()];
        let result = service
            .get_product_schema(&shop_id, &html_pages)
            .await
            .unwrap();
        assert_eq!(result.shop_id, existing.shop_id);
        assert_eq!(result.product_schemas, existing.product_schemas);
    }

    #[tokio::test]
    async fn should_create_and_save_schema_when_not_found_for_get() {
        let shop_id = ShopId::new();
        let css_schema = sample_css_schema();
        let saved = sample_shops_product_schema(shop_id);
        let saved_clone = saved.clone();

        let mut repository = MockShopsProductSchemaRepository::new();

        // First call from get_product_schema -> find_product_schema
        repository
            .expect_find_product_schema()
            .times(1)
            .return_once(|_| Box::pin(async { Ok(None) }));

        // Second call from save_product_schema -> find_product_schema (upsert check)
        repository
            .expect_find_product_schema()
            .times(1)
            .return_once(|_| Box::pin(async { Ok(None) }));

        repository
            .expect_insert_product_schema()
            .return_once(move |_, _| Box::pin(async move { Ok(saved_clone) }));

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProviderReturning(css_schema)),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let html_pages = vec!["<html></html>".to_string()];
        let result = service
            .get_product_schema(&shop_id, &html_pages)
            .await
            .unwrap();
        assert_eq!(result.shop_id, saved.shop_id);
    }

    #[tokio::test]
    async fn should_propagate_database_error_when_find_fails_for_get() {
        let shop_id = ShopId::new();

        let mut repository = MockShopsProductSchemaRepository::new();
        repository
            .expect_find_product_schema()
            .return_once(|_| Box::pin(async { Err(sqlx::Error::RowNotFound) }));

        let service = ProductSchemaServiceImpl {
            llm: Box::new(MockLlmProvider),
            rate_limiter: None,
            service_tier: None,
            repository: Box::new(repository),
        };

        let html_pages = vec!["<html></html>".to_string()];
        let result = service.get_product_schema(&shop_id, &html_pages).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProductSchemaServiceError::DatabaseError(_)
        ));
    }

    #[test]
    fn should_include_all_html_samples_in_create_instruction() {
        let html_pages = vec![
            "<html><body><h1>A</h1></body></html>".to_string(),
            "<html><body><h1>B</h1></body></html>".to_string(),
        ];
        let instruction = build_create_schemas_instruction(&html_pages);
        assert!(instruction.contains("--- SAMPLE 1 YAML ---"));
        assert!(instruction.contains("--- SAMPLE 2 YAML ---"));
        assert!(instruction.contains("compact page YAML samples"));
        assert!(instruction.contains("Derive CSS selectors"));
        assert!(instruction.contains("Return one schema per distinct template"));
        assert!(instruction.contains("not one schema per page"));
        assert!(instruction.contains("The schemas field contains one ProductCssSelectorSchema"));
        assert!(instruction.contains("multiple schemas ordered as described"));
    }

    #[test]
    fn should_build_schema_prompt_dsl_with_generic_tree_nodes() {
        let html = r#"
            <html>
              <head>
                <meta property="og:title" content="Example: Vase">
                <script>window.noisy = true;</script>
              </head>
              <body>
                <nav><a href="/noise">Noise</a></nav>
                <input id="ProductId" name="ProductId" type="hidden" value="SKU-42">
                <section class="product">
                  <h1 class="title">Example "Vase"</h1>
                  <div class="gallery"><img src="/image.jpg" data-large_image="/large.jpg"></div>
                </section>
              </body>
            </html>
        "#;

        let dsl = html_to_schema_prompt_dsl(html);

        assert!(dsl.contains("tag: meta"));
        assert!(dsl.contains("property: og:title"));
        assert!(dsl.contains("content: 'Example: Vase'"));
        assert!(dsl.contains("tag: input"));
        assert!(dsl.contains("id: ProductId"));
        assert!(dsl.contains("value: SKU-42"));
        assert!(dsl.contains("tag: h1"));
        assert!(dsl.contains("class: title"));
        assert!(dsl.contains("tag: img"));
        assert!(dsl.contains("src: /image.jpg"));
        assert!(dsl.contains("data-large_image: /large.jpg"));
        assert!(dsl.contains("Example \"Vase\""));
        assert!(!dsl.contains("<script"));
        assert!(!dsl.contains("<nav"));
    }

    #[test]
    fn should_build_schema_prompt_dsl_deterministically() {
        let html = r#"
            <html><body>
              <div class="product"><span class="price">10 EUR</span></div>
              <input id="State" value="available">
            </body></html>
        "#;

        assert_eq!(
            html_to_schema_prompt_dsl(html),
            html_to_schema_prompt_dsl(html)
        );
    }

    #[test]
    fn should_parse_generated_schemas_response() {
        let payload = generated_response_json(vec![sample_css_schema()]);
        let parsed = parse_product_schemas_response(&payload).unwrap();
        assert_eq!(parsed.schemas.len(), 1);
        assert_eq!(parsed.schemas[0], sample_css_schema());
        assert_eq!(
            parsed.evaluation.confidence,
            SchemaLlmEvaluationConfidence::High
        );
        assert!(parsed.evaluation.is_high_confidence_approval());
    }

    #[test]
    fn should_reject_legacy_single_schema_response_without_confidence() {
        let payload = serde_json::to_string(&sample_css_schema()).unwrap();
        let parsed = parse_product_schemas_response(&payload);
        assert!(parsed.is_err());
    }

    #[test]
    fn should_reject_empty_generated_schema_array() {
        let payload = generated_response_json(Vec::new());
        let parsed = parse_product_schemas_response(&payload);
        assert!(parsed.is_err());
    }

    #[test]
    fn should_reject_generated_response_without_confidence() {
        let payload = serde_json::to_string(&serde_json::json!({
            "schemas": [sample_css_schema()],
            "summary": "missing confidence"
        }))
        .unwrap();
        let parsed = parse_product_schemas_response(&payload);
        assert!(parsed.is_err());
    }

    #[test]
    fn should_parse_high_confidence_generation_response() {
        let payload = serde_json::to_string(&serde_json::json!({
            "schemas": [sample_css_schema()],
            "confidence": "HIGH",
            "summary": "Selectors are product-specific.",
            "risks": [],
            "page_findings": [
                {"role": "PRIMARY", "schema_index": 0, "finding": "Required fields are present."}
            ]
        }))
        .unwrap();

        let generated = parse_product_schemas_response(&payload).unwrap();
        let evaluation = generated.evaluation;

        assert!(evaluation.is_high_confidence_approval());
        assert!(!evaluation.approved_by_llm);
        assert_eq!(evaluation.page_findings.len(), 1);
    }

    #[test]
    fn should_mark_unavailable_schema_evaluation_as_low_confidence_human_review() {
        let evaluation = SchemaLlmEvaluation::unavailable("budget exhausted");

        assert_eq!(
            evaluation.decision,
            SchemaLlmEvaluationDecision::NeedsHumanReview
        );
        assert_eq!(evaluation.confidence, SchemaLlmEvaluationConfidence::Low);
        assert!(!evaluation.approved_by_llm);
        assert!(!evaluation.is_high_confidence_approval());
    }

    // -----------------------------------------------------------------------
    // Helpers: Mock LLM providers
    // -----------------------------------------------------------------------

    /// A concrete [`llm::chat::ChatResponse`] for test doubles.
    #[derive(Debug)]
    struct FakeChatResponse(Option<String>);

    impl std::fmt::Display for FakeChatResponse {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match &self.0 {
                Some(text) => write!(f, "{text}"),
                None => write!(f, ""),
            }
        }
    }

    impl llm::chat::ChatResponse for FakeChatResponse {
        fn text(&self) -> Option<String> {
            self.0.clone()
        }

        fn tool_calls(&self) -> Option<Vec<llm::ToolCall>> {
            None
        }
    }

    /// A mock LLM provider that panics if called — used when we expect no LLM
    /// interaction.
    struct MockLlmProvider;

    #[async_trait::async_trait]
    impl llm::chat::ChatProvider for MockLlmProvider {
        async fn chat_with_tools(
            &self,
            _messages: &[ChatMessage],
            _tools: Option<&[llm::chat::Tool]>,
        ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
            panic!("LLM should not be called in this test")
        }
    }

    /// A mock LLM provider that returns a fixed `ProductCssSelectorSchema`.
    struct MockLlmProviderReturning(ProductCssSelectorSchema);

    #[async_trait::async_trait]
    impl llm::chat::ChatProvider for MockLlmProviderReturning {
        async fn chat_with_tools(
            &self,
            _messages: &[ChatMessage],
            _tools: Option<&[llm::chat::Tool]>,
        ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
            let json = generated_response_json(vec![self.0.clone()]);
            Ok(Box::new(FakeChatResponse(Some(json))))
        }
    }
}
