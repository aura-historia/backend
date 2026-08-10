use common::{
    enhanced_match_reason::EnhancedMatchReason,
    error::boxed::box_error,
    logging::{
        GeminiServiceTier, LlmInvocationMetrics, LlmModel, LlmOperation, LlmProvider,
        log_llm_invocation,
    },
};
use google_cloud_auth::credentials::AccessTokenCredentials;
use product_service::ports::ProductSearchFilterMatchSource;
use search_filter_service::ports::{
    ProductMatchEvaluation, ProductMatchEvaluator, ProductMatchEvaluatorError, SearchFilterView,
};
use serde::{Deserialize, Serialize};

const GEMINI_MODEL: &str = "gemini-3.1-flash-lite";
const SYSTEM_INSTRUCTION: &str = "You are a product matching assistant for an antiques marketplace. Decide whether the product actually matches the requested search description. Return only JSON with a boolean `matches` and, when `matches` is true, a compact user-facing `reason` in the search language. Do not include markdown or extra fields.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexAiProductMatchEvaluatorConfig {
    project_id: String,
    location: String,
}

impl VertexAiProductMatchEvaluatorConfig {
    pub fn new(project_id: impl Into<String>, location: impl Into<String>) -> Self {
        Self {
            project_id: project_id.into(),
            location: location.into(),
        }
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn location(&self) -> &str {
        &self.location
    }
}

pub struct VertexAiProductMatchEvaluator {
    generate_content_url: String,
    client: reqwest::Client,
    credentials: AccessTokenCredentials,
}

impl VertexAiProductMatchEvaluator {
    pub fn new(
        config: VertexAiProductMatchEvaluatorConfig,
        credentials: AccessTokenCredentials,
    ) -> Self {
        Self {
            generate_content_url: build_generate_content_url(&config),
            client: reqwest::Client::new(),
            credentials,
        }
    }

    async fn request(
        &self,
        product: &ProductSearchFilterMatchSource,
        filter: &SearchFilterView,
    ) -> Result<ProductMatchEvaluation, ProductMatchEvaluatorError> {
        let request = GenerateContentRequest::new(product, filter)?;
        let access_token = self
            .credentials
            .access_token()
            .await
            .map_err(evaluation_error)?;
        let started_at = std::time::Instant::now();
        let response = self
            .client
            .post(&self.generate_content_url)
            .bearer_auth(access_token.token)
            .json(&request)
            .send()
            .await
            .map_err(evaluation_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(evaluation_error(std::io::Error::other(format!(
                "Vertex AI returned HTTP {status}"
            ))));
        }
        let response = response
            .json::<GenerateContentResponse>()
            .await
            .map_err(evaluation_error)?;
        log_llm_invocation(
            LlmOperation::ProductEnhancedSearchDescriptionMatching,
            LlmProvider::Google,
            LlmModel::Gemini31FlashLite,
            started_at.elapsed(),
            LlmInvocationMetrics {
                service_tier: Some(GeminiServiceTier::Standard),
                ..Default::default()
            },
        );
        response.into_evaluation()
    }
}

#[async_trait::async_trait]
impl ProductMatchEvaluator for VertexAiProductMatchEvaluator {
    async fn evaluate(
        &self,
        product: &ProductSearchFilterMatchSource,
        filter: &SearchFilterView,
    ) -> Result<ProductMatchEvaluation, ProductMatchEvaluatorError> {
        self.request(product, filter).await
    }
}

fn build_generate_content_url(config: &VertexAiProductMatchEvaluatorConfig) -> String {
    let endpoint = match config.location() {
        "us" | "eu" => format!(
            "https://aiplatform.{}.rep.googleapis.com",
            config.location()
        ),
        "global" => "https://aiplatform.googleapis.com".to_owned(),
        location => format!("https://{location}-aiplatform.googleapis.com"),
    };
    format!(
        "{endpoint}/v1/projects/{}/locations/{}/publishers/google/models/{GEMINI_MODEL}:generateContent",
        config.project_id(),
        config.location(),
    )
}

fn evaluation_error(
    source: impl std::error::Error + Send + Sync + 'static,
) -> ProductMatchEvaluatorError {
    ProductMatchEvaluatorError::EvaluationFailed {
        source: box_error(source),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentRequest {
    system_instruction: ProviderContent,
    contents: Vec<ProviderContent>,
    generation_config: GenerationConfig,
}

impl GenerateContentRequest {
    fn new(
        product: &ProductSearchFilterMatchSource,
        filter: &SearchFilterView,
    ) -> Result<Self, ProductMatchEvaluatorError> {
        let description = filter
            .search
            .enhanced_search_description
            .as_ref()
            .ok_or_else(|| {
                evaluation_error(std::io::Error::other("filter has no enhanced description"))
            })?;
        let (title, product_description) = product_text(product);
        let prompt = format!(
            "Requested search description:\n{description}\n\nProduct title:\n{title}\n\nProduct description:\n{product_description}\n\nRespond in {}.",
            filter.search.language.format_human_readable(),
        );
        Ok(Self {
            system_instruction: ProviderContent::text(SYSTEM_INSTRUCTION),
            contents: vec![ProviderContent::text(prompt)],
            generation_config: GenerationConfig {
                temperature: 0.0,
                max_output_tokens: 256,
                response_mime_type: "application/json",
            },
        })
    }
}

fn product_text(product: &ProductSearchFilterMatchSource) -> (&str, &str) {
    let title = product
        .product_title
        .as_ref()
        .map(|title| title.payload.as_ref())
        .or_else(|| {
            product
                .titles
                .get(
                    &product
                        .product_title
                        .as_ref()
                        .map_or(common::language::domain::Language::En, |title| {
                            title.localization
                        }),
                )
                .map(AsRef::as_ref)
        })
        .or_else(|| {
            product
                .titles
                .get(&common::language::domain::Language::En)
                .map(AsRef::as_ref)
        })
        .or_else(|| {
            product
                .titles
                .iter()
                .min_by_key(|(language, _)| language.as_str())
                .map(|(_, title)| title.as_ref())
        })
        .unwrap_or("");
    let description = product
        .product_description
        .as_ref()
        .map(|description| description.payload.as_ref())
        .or_else(|| {
            product
                .descriptions
                .get(&common::language::domain::Language::En)
                .map(AsRef::as_ref)
        })
        .or_else(|| {
            product
                .descriptions
                .iter()
                .min_by_key(|(language, _)| language.as_str())
                .map(|(_, description)| description.as_ref())
        })
        .unwrap_or("");
    (title, description)
}

#[derive(Debug, Serialize, Deserialize)]
struct ProviderContent {
    parts: Vec<ProviderPart>,
}

impl ProviderContent {
    fn text(text: impl Into<String>) -> Self {
        Self {
            parts: vec![ProviderPart { text: text.into() }],
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ProviderPart {
    text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    temperature: f32,
    max_output_tokens: u16,
    response_mime_type: &'static str,
}

#[derive(Debug, Deserialize)]
struct GenerateContentResponse {
    #[serde(default)]
    candidates: Vec<ProviderCandidate>,
}

impl GenerateContentResponse {
    fn into_evaluation(self) -> Result<ProductMatchEvaluation, ProductMatchEvaluatorError> {
        let text = self
            .candidates
            .into_iter()
            .find_map(|candidate| candidate.content)
            .and_then(|content| content.parts.into_iter().next())
            .map(|part| part.text)
            .ok_or_else(|| {
                evaluation_error(std::io::Error::other("Vertex AI response has no content"))
            })?;
        let decision =
            serde_json::from_str::<ProductMatchDecision>(&text).map_err(evaluation_error)?;
        if !decision.matches {
            return Ok(ProductMatchEvaluation::NotMatched);
        }
        let reason = decision
            .reason
            .filter(|reason| !reason.trim().is_empty())
            .ok_or_else(|| {
                evaluation_error(std::io::Error::other("matched response has no reason"))
            })?;
        Ok(ProductMatchEvaluation::Matched {
            reason: Some(EnhancedMatchReason::from(reason)),
        })
    }
}

#[derive(Debug, Deserialize)]
struct ProviderCandidate {
    content: Option<ProviderContent>,
}

#[derive(Debug, Deserialize)]
struct ProductMatchDecision {
    matches: bool,
    #[serde(default)]
    reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_build_regional_and_global_vertex_endpoints() {
        assert_eq!(
            build_generate_content_url(&VertexAiProductMatchEvaluatorConfig::new("project", "eu")),
            "https://aiplatform.eu.rep.googleapis.com/v1/projects/project/locations/eu/publishers/google/models/gemini-3.1-flash-lite:generateContent"
        );
        assert_eq!(
            build_generate_content_url(&VertexAiProductMatchEvaluatorConfig::new(
                "project", "global"
            )),
            "https://aiplatform.googleapis.com/v1/projects/project/locations/global/publishers/google/models/gemini-3.1-flash-lite:generateContent"
        );
    }

    #[test]
    fn should_accept_structured_match_response() -> Result<(), ProductMatchEvaluatorError> {
        let response = GenerateContentResponse {
            candidates: vec![ProviderCandidate {
                content: Some(ProviderContent::text(
                    r#"{"matches":true,"reason":"The title and description match."}"#,
                )),
            }],
        };

        assert!(matches!(
            response.into_evaluation()?,
            ProductMatchEvaluation::Matched { reason: Some(_) }
        ));
        Ok(())
    }

    #[test]
    fn should_reject_matched_response_without_reason() {
        let response = GenerateContentResponse {
            candidates: vec![ProviderCandidate {
                content: Some(ProviderContent::text(r#"{"matches":true}"#)),
            }],
        };

        assert!(response.into_evaluation().is_err());
    }
}
