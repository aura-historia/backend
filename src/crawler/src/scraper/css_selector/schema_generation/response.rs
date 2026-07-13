use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use schemars::{JsonSchema, schema_for};
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
pub struct SchemaLlmEvaluation {
    pub decision: SchemaLlmEvaluationDecision,
    pub confidence: SchemaLlmEvaluationConfidence,
    #[serde(default)]
    pub approved_by_llm: bool,
    pub summary: String,
    #[serde(default)]
    pub risks: Vec<String>,
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

#[derive(Debug, Clone, PartialEq)]
pub enum GeneratedAppendSchema {
    Product {
        schema: Box<ProductCssSelectorSchema>,
        evaluation: SchemaLlmEvaluation,
    },
    Removed {
        schema: RemovedPageSchema,
        evaluation: SchemaLlmEvaluation,
    },
    NotProduct {
        reason: String,
        evaluation: SchemaLlmEvaluation,
    },
}

impl GeneratedAppendSchema {
    pub fn evaluation(&self) -> &SchemaLlmEvaluation {
        match self {
            GeneratedAppendSchema::Product { evaluation, .. }
            | GeneratedAppendSchema::Removed { evaluation, .. }
            | GeneratedAppendSchema::NotProduct { evaluation, .. } => evaluation,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum AppendPageKind {
    Product,
    Removed,
    NotProduct,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
struct ProductSchemaGenerationResponse {
    #[serde(default)]
    pub page_kind: Option<AppendPageKind>,
    #[serde(default)]
    pub schemas: Vec<ProductCssSelectorSchema>,
    #[serde(default)]
    pub removed_schema: Option<RemovedPageSchema>,
    #[serde(default)]
    pub reason: Option<String>,
    pub confidence: SchemaLlmEvaluationConfidence,
    pub summary: String,
    #[serde(default)]
    pub risks: Vec<String>,
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
            },
        }
    }

    fn evaluation(&self) -> SchemaLlmEvaluation {
        let decision = if self.confidence == SchemaLlmEvaluationConfidence::High {
            SchemaLlmEvaluationDecision::Approve
        } else {
            SchemaLlmEvaluationDecision::NeedsHumanReview
        };

        SchemaLlmEvaluation {
            decision,
            confidence: self.confidence,
            approved_by_llm: false,
            summary: self.summary.clone(),
            risks: self.risks.clone(),
        }
    }

    fn into_generated_append(self) -> Result<GeneratedAppendSchema, serde_json::Error> {
        let evaluation = self.evaluation();
        match self.page_kind.unwrap_or(AppendPageKind::Product) {
            AppendPageKind::Product => {
                if self.schemas.len() != 1 {
                    return Err(invalid_data(format!(
                        "Expected exactly one product schema for append generation, got {}",
                        self.schemas.len()
                    )));
                }
                Ok(GeneratedAppendSchema::Product {
                    schema: Box::new(self.schemas.into_iter().next().ok_or_else(|| {
                        invalid_data("Expected one product schema for append generation")
                    })?),
                    evaluation,
                })
            }
            AppendPageKind::Removed => {
                if !self.schemas.is_empty() {
                    return Err(invalid_data(
                        "Removed append generation must not include product schemas",
                    ));
                }
                Ok(GeneratedAppendSchema::Removed {
                    schema: self
                        .removed_schema
                        .ok_or_else(|| invalid_data("Removed append generation missing schema"))?,
                    evaluation,
                })
            }
            AppendPageKind::NotProduct => {
                if !self.schemas.is_empty() {
                    return Err(invalid_data(
                        "Not-product append generation must not include product schemas",
                    ));
                }
                Ok(GeneratedAppendSchema::NotProduct {
                    reason: self
                        .reason
                        .unwrap_or_else(|| "not product page".to_string()),
                    evaluation,
                })
            }
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
    if matches!(
        response.page_kind,
        Some(AppendPageKind::Removed | AppendPageKind::NotProduct)
    ) {
        return Err(invalid_data(
            "Initial schema generation must not classify pages as removed or not-product",
        ));
    }
    if response.removed_schema.is_some() {
        return Err(invalid_data(
            "Initial schema generation must not include removed page schema",
        ));
    }
    if response.reason.is_some() {
        return Err(invalid_data(
            "Initial schema generation must not include page classification reason",
        ));
    }
    if response.schemas.is_empty() {
        return Err(invalid_data("LLM produced zero schemas"));
    }
    Ok(response.into_generated())
}

pub(super) fn append_schema_generation_response_schema_json() -> String {
    serde_json::to_string_pretty(&schema_for!(ProductSchemaGenerationResponse))
        .unwrap_or_else(|_| "Failed to generate response schema".to_string())
}

pub(super) fn parse_append_schema_response(
    raw: &str,
) -> Result<GeneratedAppendSchema, serde_json::Error> {
    serde_json::from_str::<ProductSchemaGenerationResponse>(raw)?.into_generated_append()
}

fn invalid_data(message: impl Into<String>) -> serde_json::Error {
    serde_json::Error::io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.into(),
    ))
}

pub fn strip_markdown_json_embedding(s: &str) -> &str {
    s.trim()
        .strip_prefix("```json")
        .unwrap_or(s)
        .strip_suffix("```")
        .unwrap_or(s)
}
