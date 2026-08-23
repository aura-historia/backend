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
pub enum GeneratedSingleSchema {
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

impl GeneratedSingleSchema {
    pub fn evaluation(&self) -> &SchemaLlmEvaluation {
        match self {
            GeneratedSingleSchema::Product { evaluation, .. }
            | GeneratedSingleSchema::Removed { evaluation, .. }
            | GeneratedSingleSchema::NotProduct { evaluation, .. } => evaluation,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum SinglePageKind {
    Product,
    Removed,
    NotProduct,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(super) enum ProductSchemaResponseValidationError {
    #[error("initial response classified page as non-product")]
    InitialPageClassification,
    #[error("initial response included removed-page schema")]
    InitialRemovedSchema,
    #[error("initial response included classification reason")]
    InitialReason,
    #[error("initial response contained no product schemas")]
    InitialEmptySchemas,
    #[error("single product response must contain exactly one schema")]
    SingleProductSchemaCount,
    #[error("removed response contained product schemas")]
    RemovedContainsProductSchemas,
    #[error("removed response omitted removed-page evidence")]
    RemovedMissingSchema,
    #[error("not-product response contained product schemas")]
    NotProductContainsProductSchemas,
    #[error("removed-page schema was invalid")]
    InvalidRemovedSchema,
}

impl ProductSchemaResponseValidationError {
    pub(super) const fn feedback_code(&self) -> &'static str {
        match self {
            Self::InitialPageClassification => "initial_page_classification",
            Self::InitialRemovedSchema => "initial_removed_schema",
            Self::InitialReason => "initial_reason_present",
            Self::InitialEmptySchemas => "initial_empty_schemas",
            Self::SingleProductSchemaCount => "single_product_schema_count",
            Self::RemovedContainsProductSchemas => "removed_contains_product_schemas",
            Self::RemovedMissingSchema => "removed_missing_schema",
            Self::NotProductContainsProductSchemas => "not_product_contains_product_schemas",
            Self::InvalidRemovedSchema => "invalid_removed_schema",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(super) struct ProductSchemaGenerationResponse {
    #[serde(default)]
    page_kind: Option<SinglePageKind>,
    #[serde(default)]
    schemas: Vec<ProductCssSelectorSchema>,
    #[serde(default)]
    removed_schema: Option<RemovedPageSchema>,
    #[serde(default)]
    reason: Option<String>,
    confidence: SchemaLlmEvaluationConfidence,
    summary: String,
    #[serde(default)]
    risks: Vec<String>,
}

impl ProductSchemaGenerationResponse {
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

    pub(super) fn try_into_initial(
        self,
    ) -> Result<GeneratedProductSchemas, ProductSchemaResponseValidationError> {
        if matches!(
            self.page_kind,
            Some(SinglePageKind::Removed | SinglePageKind::NotProduct)
        ) {
            return Err(ProductSchemaResponseValidationError::InitialPageClassification);
        }
        if self.removed_schema.is_some() {
            return Err(ProductSchemaResponseValidationError::InitialRemovedSchema);
        }
        if self.reason.is_some() {
            return Err(ProductSchemaResponseValidationError::InitialReason);
        }
        if self.schemas.is_empty() {
            return Err(ProductSchemaResponseValidationError::InitialEmptySchemas);
        }
        let evaluation = self.evaluation();
        Ok(GeneratedProductSchemas {
            schemas: self.schemas,
            evaluation,
        })
    }

    pub(super) fn try_into_single(
        self,
    ) -> Result<GeneratedSingleSchema, ProductSchemaResponseValidationError> {
        let evaluation = self.evaluation();
        match self.page_kind.unwrap_or(SinglePageKind::Product) {
            SinglePageKind::Product => {
                if self.schemas.len() != 1 {
                    return Err(ProductSchemaResponseValidationError::SingleProductSchemaCount);
                }
                let Some(schema) = self.schemas.into_iter().next() else {
                    return Err(ProductSchemaResponseValidationError::SingleProductSchemaCount);
                };
                Ok(GeneratedSingleSchema::Product {
                    schema: Box::new(schema),
                    evaluation,
                })
            }
            SinglePageKind::Removed => {
                if !self.schemas.is_empty() {
                    return Err(
                        ProductSchemaResponseValidationError::RemovedContainsProductSchemas,
                    );
                }
                let Some(schema) = self.removed_schema else {
                    return Err(ProductSchemaResponseValidationError::RemovedMissingSchema);
                };
                if schema.validate_for_llm_response().is_err() {
                    return Err(ProductSchemaResponseValidationError::InvalidRemovedSchema);
                }
                Ok(GeneratedSingleSchema::Removed { schema, evaluation })
            }
            SinglePageKind::NotProduct => {
                if !self.schemas.is_empty() {
                    return Err(
                        ProductSchemaResponseValidationError::NotProductContainsProductSchemas,
                    );
                }
                Ok(GeneratedSingleSchema::NotProduct {
                    reason: self.reason.unwrap_or_else(|| "not product page".to_owned()),
                    evaluation,
                })
            }
        }
    }
}

pub(super) fn product_schema_generation_response_json_schema() -> String {
    serde_json::to_string_pretty(&schema_for!(ProductSchemaGenerationResponse))
        .unwrap_or_else(|_| "Failed to generate response schema".to_owned())
}

#[cfg(test)]
pub(super) fn parse_product_schemas_response(
    raw: &str,
) -> Result<GeneratedProductSchemas, serde_json::Error> {
    let response = serde_json::from_str::<ProductSchemaGenerationResponse>(raw)?;
    response.try_into_initial().map_err(invalid_data)
}

pub(super) fn single_schema_generation_response_json_schema() -> String {
    serde_json::to_string_pretty(&schema_for!(ProductSchemaGenerationResponse))
        .unwrap_or_else(|_| "Failed to generate response schema".to_owned())
}

#[cfg(test)]
pub(super) fn parse_single_schema_response(
    raw: &str,
) -> Result<GeneratedSingleSchema, serde_json::Error> {
    let response = serde_json::from_str::<ProductSchemaGenerationResponse>(raw)?;
    response.try_into_single().map_err(invalid_data)
}

#[cfg(test)]
fn invalid_data(error: ProductSchemaResponseValidationError) -> serde_json::Error {
    serde_json::Error::io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        error.to_string(),
    ))
}

pub fn strip_markdown_json_embedding(s: &str) -> &str {
    s.trim()
        .strip_prefix("```json")
        .unwrap_or(s)
        .strip_suffix("```")
        .unwrap_or(s)
}
