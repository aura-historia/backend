use regex::Regex;
use std::collections::HashSet;
use tracing::{debug, info, warn};

use llm::{LLMProvider, chat::ChatMessage, error::LLMError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::SpiderError;
use crate::utils::url::CrawledUrl;

const SAMPLE_LIMIT: usize = 20;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct PatternResponse {
    /// A Rust-compatible regex matching product page URLs. Empty string if no pattern found.
    pattern: String,
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait UrlClassificationService: Send + Sync {
    async fn find_product_url_pattern(
        &self,
        all_urls: &[String],
    ) -> Result<Option<Regex>, SpiderError>;
    fn filter_product_urls(
        &self,
        pattern: &Regex,
        all_urls: &[String],
    ) -> Result<Vec<CrawledUrl>, SpiderError>;
}

pub struct UrlClassificationServiceImpl {
    llm: Box<dyn LLMProvider>,
}

impl UrlClassificationServiceImpl {
    pub fn new(llm: llm::builder::LLMBuilder) -> Result<Self, LLMError> {
        let schema = schemars::schema_for!(PatternResponse);
        let schema_json = serde_json::to_string_pretty(&schema)
            .unwrap_or_else(|_| "Failed to generate schema".to_string());

        let system_prompt = format!(
            "You are an expert at recognising e-commerce URL structures.\n\
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
            - If ALL product URLs under that segment end with -\\d+$, use -\\d+$\n\
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
            Only answer with JSON for the following schema: \n\n {}",
            schema_json
        );

        let llm = llm
            .resilient(true)
            .resilient_attempts(3)
            .system(system_prompt)
            .reasoning(true)
            .timeout_seconds(180)
            .validator(|res| {
                let cleaned = res
                    .replace("```json", "")
                    .replace("```JSON", "")
                    .replace("```", "")
                    .trim()
                    .to_string();
                serde_json::from_str::<PatternResponse>(&cleaned)
                    .map(|_| ())
                    .map_err(|err| err.to_string())
            })
            .build()?;

        Ok(Self { llm })
    }

    #[cfg(test)]
    pub fn new_with_provider(llm: Box<dyn LLMProvider>) -> Self {
        Self { llm }
    }

    fn dedupe_urls(urls: Vec<CrawledUrl>) -> Vec<CrawledUrl> {
        let mut seen = HashSet::<CrawledUrl>::new();
        let mut unique = Vec::new();

        for url in urls {
            if seen.insert(url.clone()) {
                unique.push(url);
            }
        }

        unique
    }

    fn build_prompt(urls: &[String]) -> String {
        let sample = if urls.len() > SAMPLE_LIMIT {
            &urls[..SAMPLE_LIMIT]
        } else {
            urls
        };

        format!(
            "First {sample_len} URLs (structure context):\n\
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
        let cleaned = response_text
            .replace("```json", "")
            .replace("```JSON", "")
            .replace("```", "")
            .trim()
            .to_string();

        let parsed: PatternResponse = serde_json::from_str(&cleaned)
            .map_err(|e| SpiderError::Gemini(format!("Failed to parse response: {}", e)))?;

        let pattern = parsed.pattern.trim().to_string();

        if pattern.is_empty() {
            debug!("LLM returned empty product URL pattern");
            Ok(None)
        } else {
            debug!(pattern = %pattern, "LLM returned product URL pattern");
            Ok(Some(pattern))
        }
    }
}

#[async_trait::async_trait]
impl UrlClassificationService for UrlClassificationServiceImpl {
    async fn find_product_url_pattern(
        &self,
        all_urls: &[String],
    ) -> Result<Option<Regex>, SpiderError> {
        info!(urlCount = all_urls.len(), "Analyzing crawled URLs with LLM");

        let prompt = Self::build_prompt(all_urls);
        let messages = vec![ChatMessage::user().content(prompt).build()];

        let response = match self.llm.chat(&messages).await {
            Ok(r) => r,
            Err(e) => return Err(SpiderError::Gemini(format!("LLM chat error: {}", e))),
        };

        let response_text = response
            .text()
            .ok_or_else(|| SpiderError::Gemini("LLM returned no text response".to_string()))?;

        match Self::parse_pattern_response(&response_text) {
            Ok(Some(pattern)) => match Regex::new(&pattern) {
                Ok(regex) => {
                    info!(pattern = %pattern, "LLM returned a valid URL pattern");
                    Ok(Some(regex))
                }
                Err(error) => {
                    warn!(
                        pattern = %pattern,
                        error = %error,
                        "LLM returned an invalid regex pattern"
                    );
                    Ok(None)
                }
            },
            Ok(None) => {
                info!("LLM found no consistent product URL pattern");
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn filter_product_urls(
        &self,
        pattern: &Regex,
        all_urls: &[String],
    ) -> Result<Vec<CrawledUrl>, SpiderError> {
        info!(
            urlCount = all_urls.len(),
            "Applying URL pattern to crawled URLs"
        );

        let mut matches = Vec::new();
        for url_str in all_urls {
            if let Ok(parsed_url) = url::Url::parse(url_str) {
                let crawler_url = CrawledUrl::new(parsed_url);
                if crawler_url.matches_pattern(pattern) {
                    matches.push(crawler_url);
                }
            }
        }

        debug!(matchCount = matches.len(), "Finished applying URL pattern");

        if matches.is_empty() {
            return Err(SpiderError::NoProducts(
                "Gemini pattern matched 0 URLs".to_string(),
            ));
        }

        Ok(Self::dedupe_urls(matches))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_parse_valid_json() {
        let json = r#"{"pattern": "/product/\\d+"}"#;
        let result = UrlClassificationServiceImpl::parse_pattern_response(json).unwrap();
        assert_eq!(result, Some(r"/product/\d+".to_string()));
    }

    #[test]
    fn should_parse_json_with_markdown() {
        let json = "```json\n{\"pattern\": \"/item/\"}\n```";
        let result = UrlClassificationServiceImpl::parse_pattern_response(json).unwrap();
        assert_eq!(result, Some("/item/".to_string()));
    }

    #[test]
    fn should_return_none_for_empty_pattern() {
        let json = r#"{"pattern": "  "}"#;
        let result = UrlClassificationServiceImpl::parse_pattern_response(json).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn should_error_on_invalid_json() {
        let json = "not json";
        let result = UrlClassificationServiceImpl::parse_pattern_response(json);
        assert!(result.is_err());
    }
}
