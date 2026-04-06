use crate::core::enhanced_match_reason::EnhancedMatchReason;
use common::language::domain::Language;
use llm::chat::ChatMessage;
use tracing::debug;

#[derive(Debug, thiserror::Error)]
pub enum EnhancedSearchMatchError {
    #[error("LLM request failed: {0}")]
    LlmError(#[from] llm::error::LLMError),
    #[error("Invalid LLM response: {0}")]
    InvalidResponse(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnhancedSearchMatchResult {
    pub matches: bool,
    pub reason: EnhancedMatchReason,
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait EnhancedSearchMatchService {
    async fn evaluate(
        &self,
        enhanced_search_description: &str,
        product_title: &str,
        product_description: &str,
        language: Language,
    ) -> Result<EnhancedSearchMatchResult, EnhancedSearchMatchError>;
}

pub struct EnhancedSearchMatchServiceImpl {
    llm: Box<dyn llm::LLMProvider>,
}

impl EnhancedSearchMatchServiceImpl {
    pub fn new(api_key: &str) -> Self {
        let llm = llm::builder::LLMBuilder::new()
            .backend(llm::builder::LLMBackend::Google)
            .api_key(api_key)
            .model("gemini-2.5-flash-lite")
            .system(
                "You are a product matching assistant for an antiques marketplace. \
                Given a user's search description and a product's title and description, \
                determine whether the product matches what the user is looking for. \
                Respond ONLY with exactly two lines:\n\
                match: <yes|no>\n\
                reason: <short explanation in the user's preferred language>\n\n\
                The reason must be compact and user-facing. Keep it to one or two sentences.",
            )
            .build()
            .expect("shouldn't fail building LLM provider");
        Self { llm }
    }
}

#[async_trait::async_trait]
impl EnhancedSearchMatchService for EnhancedSearchMatchServiceImpl {
    async fn evaluate(
        &self,
        enhanced_search_description: &str,
        product_title: &str,
        product_description: &str,
        language: Language,
    ) -> Result<EnhancedSearchMatchResult, EnhancedSearchMatchError> {
        let user_message = format!(
            "User's search description: {enhanced_search_description}\n\
             Product title: {product_title}\n\
             Product description: {product_description}\n\
             User's preferred language: {language}",
            language = language.format_human_readable(),
        );

        debug!("Requesting enhanced search match evaluation from Gemini API.");

        let response = self
            .llm
            .chat(&[ChatMessage::user().content(&user_message).build()])
            .await?;

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

    match (match_decision, reason) {
        (Some(matches), Some(reason)) => Ok(EnhancedSearchMatchResult {
            matches,
            reason: EnhancedMatchReason::from(reason),
        }),
        (None, _) => Err(EnhancedSearchMatchError::InvalidResponse(format!(
            "Could not parse match decision from response: {response}"
        ))),
        (_, None) => Err(EnhancedSearchMatchError::InvalidResponse(format!(
            "Could not parse reason from response: {response}"
        ))),
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
        "The product matches the description."
    )]
    #[case(
        "match: no\nreason: The product does not match.",
        false,
        "The product does not match."
    )]
    #[case("match: YES\nreason: Exact match found.", true, "Exact match found.")]
    #[case(
        "match: No\nreason: Missing key features.",
        false,
        "Missing key features."
    )]
    #[case(
        "  match: yes  \n  reason: Passt perfekt zur Beschreibung.  ",
        true,
        "Passt perfekt zur Beschreibung."
    )]
    #[case(
        "match: yes\nreason: Le produit correspond à la description.",
        true,
        "Le produit correspond à la description."
    )]
    fn should_parse_valid_response(
        #[case] response: &str,
        #[case] expected_matches: bool,
        #[case] expected_reason: &str,
    ) {
        let result = parse_enhanced_match_response(response).unwrap();
        assert_eq!(result.matches, expected_matches);
        assert_eq!(result.reason, EnhancedMatchReason::from(expected_reason));
    }

    #[rstest]
    #[case(
        "reason: Some reason without match line.",
        "Could not parse match decision"
    )]
    #[case("match: yes", "Could not parse reason")]
    #[case("invalid response", "Could not parse match decision")]
    #[case("", "Could not parse match decision")]
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
    fn should_return_matches_true_for_matching_product() {
        let response = "match: yes\nreason: Goldene Manschettenknöpfe mit 800er Silber und 24K Goldauflage gefunden.";
        let result = parse_enhanced_match_response(response).unwrap();
        assert!(result.matches);
        assert_eq!(
            result.reason,
            EnhancedMatchReason::from(
                "Goldene Manschettenknöpfe mit 800er Silber und 24K Goldauflage gefunden."
            )
        );
    }

    #[test]
    fn should_return_matches_false_for_non_matching_product() {
        let response = "match: no\nreason: El producto es una pulsera, no gemelos.";
        let result = parse_enhanced_match_response(response).unwrap();
        assert!(!result.matches);
        assert_eq!(
            result.reason,
            EnhancedMatchReason::from("El producto es una pulsera, no gemelos.")
        );
    }

    #[test]
    fn should_handle_multiline_response_with_extra_content() {
        let response = "Here is my analysis:\nmatch: yes\nreason: Perfect match for the criteria.\nAdditional notes: etc.";
        let result = parse_enhanced_match_response(response).unwrap();
        assert!(result.matches);
        assert_eq!(
            result.reason,
            EnhancedMatchReason::from("Perfect match for the criteria.")
        );
    }
}
