use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};

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

pub(super) fn product_schema_generation_response_schema_json() -> String {
    serde_json::to_string_pretty(&schema_for!(ProductSchemaGenerationResponse))
        .unwrap_or_else(|_| "Failed to generate response schema".to_string())
}

pub(super) fn parse_product_schemas_response(
    raw: &str,
) -> Result<GeneratedProductSchemas, serde_json::Error> {
    let response = serde_json::from_str::<ProductSchemaGenerationResponse>(raw)?;
    if response.schemas.is_empty() {
        return Err(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "LLM produced zero schemas",
        )));
    }
    Ok(response.into_generated())
}

pub fn strip_markdown_json_embedding(s: &str) -> &str {
    s.trim()
        .strip_prefix("```json")
        .unwrap_or(s)
        .strip_suffix("```")
        .unwrap_or(s)
}
