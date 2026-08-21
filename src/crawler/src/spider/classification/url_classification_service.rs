use regex::Regex;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};

use crate::google_llm::{GeminiRateLimiter, run_with_gemini_rate_limiter};
use crate::logging::llm_metrics;
use crate::scraper::css_selector::product_schema_service::strip_markdown_json_embedding;
use large_language_model::{
    GeminiServiceTier, LlmModel, LlmOperation, LlmProvider, log_llm_invocation,
};
use llm::{
    chat::{ChatMessage, ChatProvider},
    error::LLMError,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::spider::utils::url::CrawledUrl;

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
    ) -> Result<Option<Regex>, UrlClassificationError>;
    fn filter_product_urls(
        &self,
        pattern: &Regex,
        all_urls: &[String],
    ) -> Result<Vec<CrawledUrl>, UrlClassificationError>;
}

#[derive(Debug, Error)]
pub enum UrlClassificationError {
    #[error("LLM classification error: {0}")]
    Llm(String),

    #[error(transparent)]
    Regex(#[from] regex::Error),

    #[error("No product pages found: {0}")]
    NoProducts(String),
}

pub struct UrlClassificationServiceImpl {
    llm: Box<dyn ChatProvider>,
    rate_limiter: Option<Arc<GeminiRateLimiter>>,
    service_tier: Option<GeminiServiceTier>,
}

impl UrlClassificationServiceImpl {
    pub fn new(
        llm: llm::builder::LLMBuilder,
        service_tier: Option<GeminiServiceTier>,
        rate_limiter: Option<Arc<GeminiRateLimiter>>,
    ) -> Result<Self, LLMError> {
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
            .system(system_prompt)
            .reasoning(true)
            .timeout_seconds(180)
            .validator(|res| {
                serde_json::from_str::<PatternResponse>(strip_markdown_json_embedding(res))
                    .map(|_| ())
                    .map_err(|err| err.to_string())
            })
            .validator_attempts(3)
            .build()?;
        let llm: Box<dyn ChatProvider> = llm;

        Ok(Self {
            llm,
            rate_limiter,
            service_tier,
        })
    }

    #[cfg(test)]
    pub fn new_with_provider(llm: Box<dyn ChatProvider>) -> Self {
        Self {
            llm,
            rate_limiter: None,
            service_tier: None,
        }
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

    fn parse_pattern_response(
        response_text: &str,
    ) -> Result<Option<String>, UrlClassificationError> {
        let parsed: PatternResponse =
            serde_json::from_str(strip_markdown_json_embedding(response_text)).map_err(|e| {
                UrlClassificationError::Llm(format!("Failed to parse response: {}", e))
            })?;

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
    #[tracing::instrument(
        name = "spider_classify_product_url_pattern",
        skip(self, all_urls),
        fields(url_count = all_urls.len())
    )]
    async fn find_product_url_pattern(
        &self,
        all_urls: &[String],
    ) -> Result<Option<Regex>, UrlClassificationError> {
        debug!("Analyzing crawled URLs with LLM");

        let prompt = Self::build_prompt(all_urls);
        let messages = vec![ChatMessage::user().content(prompt).build()];

        let started_at = Instant::now();
        let response =
            match run_with_gemini_rate_limiter(&*self.llm, self.rate_limiter.as_deref(), &messages)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    return Err(UrlClassificationError::Llm(format!(
                        "LLM chat error: {}",
                        e
                    )));
                }
            };
        log_llm_invocation(
            LlmOperation::CrawlerUrlClassification,
            LlmProvider::Google,
            LlmModel::Configured,
            started_at.elapsed(),
            llm_metrics(response.usage(), Some(all_urls.len()), self.service_tier),
        );

        let response_text = response.text().ok_or_else(|| {
            UrlClassificationError::Llm("LLM returned no text response".to_string())
        })?;

        match Self::parse_pattern_response(&response_text) {
            Ok(Some(pattern)) => match Regex::new(&pattern) {
                Ok(regex) => {
                    info!(pattern = %pattern, "LLM returned a valid URL pattern");
                    Ok(Some(regex))
                }
                Err(error) => {
                    warn!(
                        pattern = %pattern,
                        error = ?error,
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

    #[tracing::instrument(
        name = "spider_filter_product_urls",
        skip(self, pattern, all_urls),
        fields(url_count = all_urls.len())
    )]
    fn filter_product_urls(
        &self,
        pattern: &Regex,
        all_urls: &[String],
    ) -> Result<Vec<CrawledUrl>, UrlClassificationError> {
        info!("Applying URL pattern to crawled URLs");

        let mut matches = Vec::new();
        for url_str in all_urls {
            if let Ok(parsed_url) = url::Url::parse(url_str) {
                let crawler_url = CrawledUrl::new(parsed_url);
                if crawler_url.matches_pattern(pattern) {
                    matches.push(crawler_url);
                }
            }
        }

        debug!(match_count = matches.len(), "Finished applying URL pattern");

        if matches.is_empty() {
            return Err(UrlClassificationError::NoProducts(
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
