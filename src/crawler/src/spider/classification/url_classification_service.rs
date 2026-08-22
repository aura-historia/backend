use crate::llm_runtime::{CrawlerLlmGovernor, generate_with_governor};
use large_language_model::{
    GenerationOptions, LargeLanguageModel, LargeLanguageModelError, LlmOperation,
    StructuredGenerationRequest,
};
use regex::Regex;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, info};

use crate::spider::utils::url::CrawledUrl;

const SAMPLE_LIMIT: usize = 20;
const SYSTEM_INSTRUCTION: &str = "You are an expert at recognising e-commerce URL structures.\n\
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
    Only answer with JSON matching the response schema.";

#[derive(Debug, Deserialize, JsonSchema)]
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

pub struct UrlClassificationServiceImpl<Llm: LargeLanguageModel> {
    llm: Llm,
    governor: Option<Arc<CrawlerLlmGovernor>>,
}

impl<Llm: LargeLanguageModel> UrlClassificationServiceImpl<Llm> {
    pub fn new(llm: Llm, governor: Option<Arc<CrawlerLlmGovernor>>) -> Self {
        Self { llm, governor }
    }

    fn build_generation_request(
        all_urls: &[String],
    ) -> Result<StructuredGenerationRequest, UrlClassificationError> {
        let response_schema = serde_json::to_value(schemars::schema_for!(PatternResponse))
            .map_err(|error| {
                UrlClassificationError::Llm(format!("Failed to serialize response schema: {error}"))
            })?;

        Ok(StructuredGenerationRequest {
            operation: LlmOperation::CrawlerUrlClassification,
            system_instruction: SYSTEM_INSTRUCTION.to_owned(),
            prompt: Self::build_prompt(all_urls),
            image_urls: Vec::new(),
            response_schema,
            options: GenerationOptions {
                temperature: 0.0,
                max_output_tokens: 512,
            },
        })
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
}

impl From<LargeLanguageModelError> for UrlClassificationError {
    fn from(error: LargeLanguageModelError) -> Self {
        Self::Llm(error.to_string())
    }
}

#[async_trait::async_trait]
impl<Llm: LargeLanguageModel> UrlClassificationService for UrlClassificationServiceImpl<Llm> {
    #[tracing::instrument(
        name = "spider_classify_product_url_pattern",
        skip(self, all_urls),
        fields(url_count = all_urls.len())
    )]
    async fn find_product_url_pattern(
        &self,
        all_urls: &[String],
    ) -> Result<Option<Regex>, UrlClassificationError> {
        let response: PatternResponse = generate_with_governor(
            &self.llm,
            self.governor.as_ref(),
            Self::build_generation_request(all_urls)?,
        )
        .await
        .map_err(UrlClassificationError::from)?;

        let pattern = response.pattern.trim();
        if pattern.is_empty() {
            return Ok(None);
        }

        match Regex::new(pattern) {
            Ok(regex) => Ok(Some(regex)),
            Err(_) => Ok(None),
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
                "LLM pattern matched 0 URLs".to_string(),
            ));
        }

        Ok(Self::dedupe_urls(matches))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedLargeLanguageModel {
        response: &'static str,
    }

    #[async_trait::async_trait]
    impl LargeLanguageModel for FixedLargeLanguageModel {
        async fn generate<Output>(
            &self,
            _request: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: serde::de::DeserializeOwned + Send,
        {
            serde_json::from_str(self.response).map_err(|source| {
                LargeLanguageModelError::InvalidResponse {
                    source: Box::new(source),
                }
            })
        }
    }

    fn service(response: &'static str) -> UrlClassificationServiceImpl<FixedLargeLanguageModel> {
        UrlClassificationServiceImpl::new(FixedLargeLanguageModel { response }, None)
    }

    #[test]
    fn should_build_canonical_url_classification_request() {
        let urls = vec!["https://example.com/product/desk-1".to_owned()];
        let request =
            UrlClassificationServiceImpl::<FixedLargeLanguageModel>::build_generation_request(
                &urls,
            );

        match request {
            Ok(request) => {
                assert_eq!(request.operation, LlmOperation::CrawlerUrlClassification);
                assert_eq!(request.system_instruction, SYSTEM_INSTRUCTION);
                assert_eq!(
                    request.prompt,
                    UrlClassificationServiceImpl::<FixedLargeLanguageModel>::build_prompt(&urls)
                );
                assert!(request.image_urls.is_empty());
                assert_eq!(request.options.temperature, 0.0);
                assert_eq!(request.options.max_output_tokens, 512);
                assert_eq!(
                    request
                        .response_schema
                        .pointer("/properties/pattern/type")
                        .and_then(serde_json::Value::as_str),
                    Some("string")
                );
            }
            Err(error) => panic!("should serialize PatternResponse schema: {error}"),
        }
    }

    #[tokio::test]
    async fn should_return_regex_from_large_language_model_response() {
        let result = service(r#"{"pattern":"/product/\\d+$"}"#)
            .find_product_url_pattern(&["https://example.com/product/1".to_owned()])
            .await;

        assert!(matches!(
            result,
            Ok(Some(pattern)) if pattern.as_str() == r"/product/\d+$"
        ));
    }

    #[tokio::test]
    async fn should_return_none_when_large_language_model_returns_empty_pattern() {
        let result = service(r#"{"pattern":"  "}"#)
            .find_product_url_pattern(&["https://example.com/page".to_owned()])
            .await;

        assert!(matches!(result, Ok(None)));
    }

    #[tokio::test]
    async fn should_return_none_when_large_language_model_returns_invalid_regex() {
        let result = service(r#"{"pattern":"["}"#)
            .find_product_url_pattern(&["https://example.com/product/1".to_owned()])
            .await;

        assert!(matches!(result, Ok(None)));
    }

    #[tokio::test]
    async fn should_map_large_language_model_deserialization_error() {
        let result = service("not json")
            .find_product_url_pattern(&["https://example.com/product/1".to_owned()])
            .await;

        assert!(matches!(result, Err(UrlClassificationError::Llm(_))));
    }
}
