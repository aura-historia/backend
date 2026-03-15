use llm::builder::{LLMBackend, LLMBuilder};
use llm::chat::{ChatMessage, StructuredOutputFormat};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::error::SpiderError;

const DEFAULT_MODEL: &str = "gemini-3.1-flash-lite-preview";
const SAMPLE_LIMIT: usize = 20;

#[derive(Debug, Deserialize, Serialize)]
struct PatternResponse {
    pattern: String,
}

fn pattern_response_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "pattern": {
                "type": "string",
                "description": "A Rust-compatible regex matching product page URLs. Empty string if no pattern found."
            }
        },
        "required": ["pattern"]
    })
}

pub struct GeminiClient {
    api_key: String,
    model: String,
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait PatternInferenceClient: Send + Sync {
    async fn infer_product_url_pattern(
        &self,
        urls: &[String],
    ) -> Result<Option<String>, SpiderError>;
}

impl GeminiClient {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            model: DEFAULT_MODEL.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl PatternInferenceClient for GeminiClient {
    async fn infer_product_url_pattern(
        &self,
        urls: &[String],
    ) -> Result<Option<String>, SpiderError> {
        let structured_format = StructuredOutputFormat {
            name: "PatternResponse".to_string(),
            description: Some("Product URL pattern extraction result".to_string()),
            schema: Some(pattern_response_schema()),
            strict: None,
        };

        let prompt = build_prompt(urls);
        let messages = vec![ChatMessage::user().content(prompt).build()];

        let llm = LLMBuilder::new()
            .backend(LLMBackend::Google)
            .api_key(self.api_key.clone())
            .model(self.model.clone())
            .temperature(0.0)
            .schema(structured_format)
            .build()
            .map_err(|error| {
                SpiderError::Gemini(format!(
                    "Failed to build Gemini client (structured output): {error}"
                ))
            })?;

        let response = match llm.chat(&messages).await {
            Ok(response) => response,
            Err(first_error) => {
                let first_error = first_error.to_string();
                if !should_retry_without_schema(&first_error) {
                    return Err(SpiderError::Gemini(format!(
                        "Gemini chat error (structured output): {first_error}"
                    )));
                }

                warn!(
                    error = %first_error,
                    "Gemini structured output failed, retrying without schema"
                );

                let fallback_llm = LLMBuilder::new()
                    .backend(LLMBackend::Google)
                    .api_key(self.api_key.clone())
                    .model(self.model.clone())
                    .temperature(0.0)
                    .build()
                    .map_err(|error| {
                        SpiderError::Gemini(format!(
                            "Failed to build Gemini fallback client (no schema): {error}"
                        ))
                    })?;

                fallback_llm
                    .chat(&messages)
                    .await
                    .map_err(|second_error| {
                        SpiderError::Gemini(format!(
                            "Gemini chat failed with structured output ('{first_error}') and fallback without schema ('{second_error}')."
                        ))
                    })?
            }
        };

        let response_text = response
            .text()
            .ok_or_else(|| SpiderError::Gemini("Gemini returned no text response".to_string()))?;

        parse_pattern_response(&response_text)
    }
}

fn build_prompt(urls: &[String]) -> String {
    let sample = if urls.len() > SAMPLE_LIMIT {
        &urls[..SAMPLE_LIMIT]
    } else {
        urls
    };

    format!(
        "You are an expert at recognising e-commerce URL structures.\n\
         \n\
         TASK: return a single Rust-compatible regex that matches EVERY individual \
         product-detail page URL in the list below and rejects everything else.\n\
         \n\
         STEP 1 - IDENTIFY THE STRUCTURAL SEPARATOR\n\
         Look at every URL path and determine how the site separates product pages \
         from other pages.\n\
         A) SEGMENT-BASED: products live under a dedicated path segment like\n\
            /product/<slug>, /produkt/<slug>, /lot/<slug>, /item/<slug>, etc.\n\
            Other pages use different segments (/category/, /tag/, /about, ...).\n\
            -> Anchor on that exact segment with slashes.\n\
            -> Be careful: /produkt-kategorie/ shares a prefix with /produkt/ \
               but is NOT a product page.\n\
         B) FLAT / MIXED: all pages are top-level slugs with no stable segment.\n\
            -> There is NO reliable structural pattern.\n\
            -> Return an empty pattern string.\n\
         \n\
         STEP 2 - SLUG SUFFIX\n\
         Only if you found a segment in Step 1:\n\
         - If ALL product URLs under that segment end with -<digits only>, use -\\d+$\n\
         - Otherwise use [\\w%.-]+$\n\
         Never require a specific suffix pattern that appears in only some URLs.\n\
         \n\
         STEP 3 - STRICT SELF-CHECK\n\
         Mentally test your pattern against EVERY URL in the list.\n\
         a) Every product detail URL MUST match. If one does not -> return empty string.\n\
         b) Category/listing/home/utility/pagination URLs MUST NOT match.\n\
            If one matches and cannot be fixed safely -> return empty string.\n\
         Prefer empty string over a pattern that misses products.\n\
         \n\
         OUTPUT\n\
         Return JSON with exactly one field:\n\
         {{\n\
           \"pattern\": \"<regex or empty string>\"\n\
         }}\n\
         \n\
         Pattern rules when non-empty:\n\
         - Match the full URL including scheme and domain.\n\
         - Use literal slashes for path segments.\n\
         - End with $.\n\
         \n\
         First {sample_len} URLs (structure context):\n\
         {sample_urls}\n\
         \n\
         All {all_len} URLs:\n\
         {all_urls}",
        sample_len = sample.len(),
        sample_urls = sample.join("\n"),
        all_len = urls.len(),
        all_urls = urls.join("\n")
    )
}

fn parse_pattern_response(response_text: &str) -> Result<Option<String>, SpiderError> {
    if let Some(pattern) = parse_pattern_from_json(response_text) {
        return Ok(pattern);
    }

    let cleaned = response_text
        .replace("```json", "")
        .replace("```JSON", "")
        .replace("```", "")
        .trim()
        .to_string();

    if let Some(pattern) = parse_pattern_from_json(&cleaned) {
        return Ok(pattern);
    }

    warn!(
        response = %response_text,
        "Failed to parse Gemini response into PatternResponse"
    );
    Ok(None)
}

fn parse_pattern_from_json(raw: &str) -> Option<Option<String>> {
    let parsed = serde_json::from_str::<PatternResponse>(raw).ok()?;
    let pattern = parsed.pattern.trim().to_string();

    if pattern.is_empty() {
        debug!("Gemini returned empty product URL pattern");
        Some(None)
    } else {
        debug!(pattern = %pattern, "Gemini returned product URL pattern");
        Some(Some(pattern))
    }
}

fn should_retry_without_schema(error: &str) -> bool {
    error.contains("400") || error.contains("INVALID_ARGUMENT")
}
