use crate::core::user_search_filter::EnhancedSearchDescription;
use common::enhanced_match_reason::EnhancedMatchReason;
use common::language::domain::Language;
use large_language_model::{
    GeminiServiceTier, LlmModel, LlmOperation, LlmProvider, log_llm_invocation,
};
use llm::{
    backends::google::{GooglePlatform, GoogleServiceTier},
    chat::{ChatMessage, ImageMime},
};
use product::core::description::Description;
use product::core::product_image::ProductImage;
use product::core::title::Title;
use tracing::{debug, warn};

const DEFAULT_ENHANCED_SEARCH_MATCH_MODEL: &str = "gemini-3.1-flash-lite";

#[derive(Debug, thiserror::Error)]
pub enum EnhancedSearchMatchError {
    #[error("LLM request failed: {0}")]
    LlmError(#[from] llm::error::LLMError),
    #[error("Invalid LLM response: {0}")]
    InvalidResponse(String),
}

/// Result of an enhanced search match evaluation.
///
/// When the product matches, `reason` contains a compact user-facing explanation.
/// When the product does not match, `reason` is `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct EnhancedSearchMatchResult {
    pub matches: bool,
    pub reason: Option<EnhancedMatchReason>,
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait EnhancedSearchMatchService {
    async fn evaluate(
        &self,
        enhanced_search_description: &EnhancedSearchDescription,
        product_title: &Title,
        product_description: &Description,
        language: Language,
        product_images: &[ProductImage],
    ) -> Result<EnhancedSearchMatchResult, EnhancedSearchMatchError>;
}

pub struct EnhancedSearchMatchServiceImpl {
    llm: Box<dyn llm::LLMProvider>,
    http_client: reqwest::Client,
}

impl EnhancedSearchMatchServiceImpl {
    pub fn new(api_key: &str) -> Self {
        Self::new_with_model(api_key, DEFAULT_ENHANCED_SEARCH_MATCH_MODEL, false)
    }

    pub fn new_with_model(api_key: &str, model: &str, flex: bool) -> Self {
        let mut builder = llm::builder::LLMBuilder::new()
            .backend(llm::builder::LLMBackend::Google)
            .google_platform(GooglePlatform::GeminiEnterpriseAgent {
                project_id: "project-2c6e1dcc-3fb9-4910-adc".to_owned(),
                region: Some("europe-west3".to_owned()),
            })
            .api_key(api_key)
            .model(model)
            .temperature(0.0)
            .max_tokens(256)
            .timeout_seconds(30)
            .resilient(true)
            .resilient_attempts(3)
            .resilient_backoff(500, 5_000)
            .resilient_jitter(true)
            .system(
                "You are a product matching assistant for an antiques marketplace. \
                Given a user's search description, a product's title and description, \
                and optionally product images, \
                determine whether the product matches what the user is looking for.\n\n\
                If the product matches, respond with exactly two lines:\n\
                match: yes\n\
                reason: <short explanation in the user's preferred language>\n\n\
                If the product does NOT match, respond with exactly one line:\n\
                match: no\n\n\
                The reason must be compact and user-facing. Keep it to one or two sentences. \
                Do not include any additional text or explanations.",
            );
        if flex {
            builder = builder.google_service_tier(GoogleServiceTier::Flex);
        }
        let llm = builder
            .build()
            .expect("Failed to initialize LLM provider with valid configuration");
        Self {
            llm,
            http_client: reqwest::Client::new(),
        }
    }

    /// Fetches image bytes from the given URL.
    /// Returns `(mime_type, bytes)` on success or `None` on failure.
    async fn fetch_image_bytes(&self, url: &str) -> Option<(ImageMime, Vec<u8>)> {
        match self.http_client.get(url).send().await {
            Ok(response) => {
                let mime = response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(parse_image_mime_from_content_type)
                    .unwrap_or(ImageMime::JPEG);

                match response.bytes().await {
                    Ok(bytes) => Some((mime, bytes.to_vec())),
                    Err(err) => {
                        warn!(error = %err, url = %url, "Failed reading image bytes for LLM evaluation.");
                        None
                    }
                }
            }
            Err(err) => {
                warn!(error = %err, url = %url, "Failed fetching product image for LLM evaluation.");
                None
            }
        }
    }
}

#[async_trait::async_trait]
impl EnhancedSearchMatchService for EnhancedSearchMatchServiceImpl {
    async fn evaluate(
        &self,
        enhanced_search_description: &EnhancedSearchDescription,
        product_title: &Title,
        product_description: &Description,
        language: Language,
        product_images: &[ProductImage],
    ) -> Result<EnhancedSearchMatchResult, EnhancedSearchMatchError> {
        let user_message = format!(
            "User's search description: {enhanced_search_description}\n\
             Product title: {product_title}\n\
             Product description: {product_description}\n\
             User's preferred language: {language}",
            language = language.format_human_readable(),
        );

        let mut messages = vec![ChatMessage::user().content(&user_message).build()];
        let image_count_before = messages.len();

        for image in product_images.iter() {
            if let Some((mime, bytes)) = self.fetch_image_bytes(image.url.as_str()).await {
                messages.push(ChatMessage::user().image(mime, bytes).build());
            }
        }

        debug!(
            images_included = messages.len() - image_count_before,
            "Requesting enhanced search match evaluation."
        );

        let started_at = std::time::Instant::now();
        let response = self.llm.chat(&messages).await?;
        log_llm_invocation(
            LlmOperation::ProductEnhancedSearchDescriptionMatching,
            LlmProvider::Google,
            LlmModel::Gemini31FlashLite,
            started_at.elapsed(),
            llm_metrics(response.usage(), Some(GeminiServiceTier::Standard)),
        );

        let response_text = response.text().ok_or_else(|| {
            EnhancedSearchMatchError::InvalidResponse("Empty response from LLM".to_string())
        })?;

        parse_enhanced_match_response(&response_text)
    }
}

fn parse_enhanced_match_response(
    response: &str,
) -> Result<EnhancedSearchMatchResult, EnhancedSearchMatchError> {
    let mut match_decision = None;
    let mut reason = None;

    for line in response.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("match:") {
            let value = value.trim().to_lowercase();
            match_decision = Some(value == "yes");
        } else if let Some(value) = line.strip_prefix("reason:") {
            reason = Some(value.trim().to_owned());
        }
    }

    match match_decision {
        Some(true) => {
            let reason = reason.ok_or_else(|| {
                EnhancedSearchMatchError::InvalidResponse(format!(
                    "Match is 'yes' but no reason provided in response: {response}"
                ))
            })?;
            Ok(EnhancedSearchMatchResult {
                matches: true,
                reason: Some(EnhancedMatchReason::from(reason)),
            })
        }
        Some(false) => Ok(EnhancedSearchMatchResult {
            matches: false,
            reason: None,
        }),
        None => Err(EnhancedSearchMatchError::InvalidResponse(format!(
            "Could not parse match decision from response: {response}"
        ))),
    }
}

/// Parses an `ImageMime` from a raw Content-Type header value.
/// Falls back to `None` for unsupported or missing MIME types.
fn parse_image_mime_from_content_type(content_type: &str) -> Option<ImageMime> {
    let mime = content_type.split(';').next()?.trim();
    match mime {
        "image/jpeg" => Some(ImageMime::JPEG),
        "image/png" => Some(ImageMime::PNG),
        "image/gif" => Some(ImageMime::GIF),
        "image/webp" => Some(ImageMime::WEBP),
        _ => None,
    }
}

fn llm_metrics(
    usage: Option<llm::chat::Usage>,
    service_tier: Option<large_language_model::GeminiServiceTier>,
) -> large_language_model::LlmInvocationMetrics {
    let Some(usage) = usage else {
        return large_language_model::LlmInvocationMetrics {
            service_tier,
            ..Default::default()
        };
    };

    large_language_model::LlmInvocationMetrics {
        service_tier,
        prompt_tokens: Some(usage.prompt_tokens),
        completion_tokens: Some(usage.completion_tokens),
        total_tokens: Some(usage.total_tokens),
        cached_prompt_tokens: usage
            .prompt_tokens_details
            .and_then(|details| details.cached_tokens),
        reasoning_tokens: usage
            .completion_tokens_details
            .and_then(|details| details.reasoning_tokens),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(
        "match: yes\nreason: The product matches the description.",
        true,
        Some("The product matches the description.")
    )]
    #[case("match: no", false, None)]
    #[case(
        "match: YES\nreason: Exact match found.",
        true,
        Some("Exact match found.")
    )]
    #[case("match: No", false, None)]
    #[case(
        "  match: yes  \n  reason: Passt perfekt zur Beschreibung.  ",
        true,
        Some("Passt perfekt zur Beschreibung.")
    )]
    #[case(
        "match: yes\nreason: Le produit correspond à la description.",
        true,
        Some("Le produit correspond à la description.")
    )]
    #[case("match: no\nreason: Ignored reason for non-match.", false, None)]
    #[case("match: NO", false, None)]
    fn should_parse_valid_response(
        #[case] response: &str,
        #[case] expected_matches: bool,
        #[case] expected_reason: Option<&str>,
    ) {
        let result = parse_enhanced_match_response(response).unwrap();
        assert_eq!(result.matches, expected_matches);
        assert_eq!(
            result.reason,
            expected_reason.map(EnhancedMatchReason::from)
        );
    }

    #[rstest]
    #[case("match: yes", "no reason provided")]
    #[case("invalid response", "Could not parse match decision")]
    #[case("", "Could not parse match decision")]
    #[case(
        "reason: Some reason without match line.",
        "Could not parse match decision"
    )]
    fn should_fail_parsing_invalid_response(
        #[case] response: &str,
        #[case] expected_error_contains: &str,
    ) {
        let result = parse_enhanced_match_response(response);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains(expected_error_contains),
            "Expected error to contain '{expected_error_contains}', got: {err}"
        );
    }

    #[test]
    fn should_return_match_with_reason_for_matching_product() {
        let response = "match: yes\nreason: Goldene Manschettenknöpfe mit 800er Silber und 24K Goldauflage gefunden.";
        let result = parse_enhanced_match_response(response).unwrap();
        assert!(result.matches);
        assert_eq!(
            result.reason,
            Some(EnhancedMatchReason::from(
                "Goldene Manschettenknöpfe mit 800er Silber und 24K Goldauflage gefunden."
            ))
        );
    }

    #[test]
    fn should_return_no_match_without_reason_for_non_matching_product() {
        let response = "match: no";
        let result = parse_enhanced_match_response(response).unwrap();
        assert!(!result.matches);
        assert_eq!(result.reason, None);
    }

    #[test]
    fn should_discard_reason_when_match_is_no() {
        let response = "match: no\nreason: El producto es una pulsera, no gemelos.";
        let result = parse_enhanced_match_response(response).unwrap();
        assert!(!result.matches);
        assert_eq!(result.reason, None);
    }

    #[test]
    fn should_handle_multiline_response_with_extra_content() {
        let response = "Here is my analysis:\nmatch: yes\nreason: Perfect match for the criteria.\nAdditional notes: etc.";
        let result = parse_enhanced_match_response(response).unwrap();
        assert!(result.matches);
        assert_eq!(
            result.reason,
            Some(EnhancedMatchReason::from("Perfect match for the criteria."))
        );
    }

    #[test]
    fn should_error_when_match_yes_but_no_reason() {
        let response = "match: yes";
        let result = parse_enhanced_match_response(response);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("no reason provided")
        );
    }

    #[rstest]
    #[case("image/jpeg", Some(ImageMime::JPEG))]
    #[case("image/png", Some(ImageMime::PNG))]
    #[case("image/gif", Some(ImageMime::GIF))]
    #[case("image/webp", Some(ImageMime::WEBP))]
    #[case("image/jpeg; charset=utf-8", Some(ImageMime::JPEG))]
    #[case("image/png; boundary=something", Some(ImageMime::PNG))]
    #[case("image/bmp", None)]
    #[case("text/html", None)]
    #[case("application/octet-stream", None)]
    #[case("", None)]
    fn should_parse_image_mime_from_content_type(
        #[case] content_type: &str,
        #[case] expected: Option<ImageMime>,
    ) {
        assert_eq!(parse_image_mime_from_content_type(content_type), expected);
    }
}
