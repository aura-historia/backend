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
    ProductListing {
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
            GeneratedSingleSchema::ProductListing { evaluation, .. }
            | GeneratedSingleSchema::Removed { evaluation, .. }
            | GeneratedSingleSchema::NotProduct { evaluation, .. } => evaluation,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum SinglePageKind {
    #[serde(rename = "product")]
    ProductListing,
    Removed,
    NotProduct,
}

/// Semantic validation failure of a structured schema-generation response.
///
/// Each variant names the offending field and value so retry feedback and logs stay
/// precise without echoing the prompt, the HTML, or the full response.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(super) enum ProductListingSchemaResponseValidationError {
    #[error("field `schemas` was empty; at least one product schema is required")]
    InitialEmptySchemas,
    #[error("field `schemas[{schema_index}]` contained an invalid extraction rule: {detail}")]
    InvalidProductSchemaRule { schema_index: usize, detail: String },
    #[error(
        "field `schemas` must contain exactly one schema for page_kind `product`, got {schema_count}"
    )]
    SingleProductSchemaCount { schema_count: usize },
    #[error(
        "field `schemas` must be empty for page_kind `removed`, got {schema_count} product schemas"
    )]
    RemovedContainsProductSchemas { schema_count: usize },
    #[error("field `removed_schema` is required for page_kind `removed` but was null")]
    RemovedMissingSchema,
    #[error(
        "field `schemas` must be empty for page_kind `not_product`, got {schema_count} product schemas"
    )]
    NotProductContainsProductSchemas { schema_count: usize },
    #[error("field `removed_schema` was invalid: {detail}")]
    InvalidRemovedSchema { detail: String },
}

impl ProductListingSchemaResponseValidationError {
    pub(super) const fn feedback_code(&self) -> &'static str {
        match self {
            Self::InitialEmptySchemas => "initial_empty_schemas",
            Self::InvalidProductSchemaRule { .. } => "invalid_product_schema_rule",
            Self::SingleProductSchemaCount { .. } => "single_product_schema_count",
            Self::RemovedContainsProductSchemas { .. } => "removed_contains_product_schemas",
            Self::RemovedMissingSchema => "removed_missing_schema",
            Self::NotProductContainsProductSchemas { .. } => "not_product_contains_product_schemas",
            Self::InvalidRemovedSchema { .. } => "invalid_removed_schema",
        }
    }
}

fn validate_schema_rules(
    schemas: &[ProductCssSelectorSchema],
) -> Result<(), ProductListingSchemaResponseValidationError> {
    schemas
        .iter()
        .enumerate()
        .try_for_each(|(schema_index, schema)| {
            schema.validate_rules().map_err(|error| {
                ProductListingSchemaResponseValidationError::InvalidProductSchemaRule {
                    schema_index,
                    detail: error.to_string(),
                }
            })
        })
}

fn evaluation_from_response(
    confidence: SchemaLlmEvaluationConfidence,
    summary: &str,
    risks: &[String],
) -> SchemaLlmEvaluation {
    let decision = if confidence == SchemaLlmEvaluationConfidence::High {
        SchemaLlmEvaluationDecision::Approve
    } else {
        SchemaLlmEvaluationDecision::NeedsHumanReview
    };

    SchemaLlmEvaluation {
        decision,
        confidence,
        approved_by_llm: false,
        summary: summary.to_owned(),
        risks: risks.to_vec(),
    }
}

/// Structured response of the initial, multi-sample schema generation request.
///
/// Every sample is expected to be a product page, so this contract deliberately offers
/// no page classification: the model can only answer with product schemas. A sample the
/// model considers non-product is excluded from schema coverage and reported in `risks`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(super) struct ProductListingSchemasGenerationResponse {
    #[serde(default)]
    schemas: Vec<ProductCssSelectorSchema>,
    confidence: SchemaLlmEvaluationConfidence,
    summary: String,
    #[serde(default)]
    risks: Vec<String>,
}

impl ProductListingSchemasGenerationResponse {
    pub(super) fn try_into_initial(
        self,
    ) -> Result<GeneratedProductSchemas, ProductListingSchemaResponseValidationError> {
        if self.schemas.is_empty() {
            return Err(ProductListingSchemaResponseValidationError::InitialEmptySchemas);
        }
        validate_schema_rules(&self.schemas)?;
        let evaluation = evaluation_from_response(self.confidence, &self.summary, &self.risks);
        Ok(GeneratedProductSchemas {
            schemas: self.schemas,
            evaluation,
        })
    }
}

/// Structured response of the single-page fresh generation request, which classifies
/// the page as product, removed, or not_product.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(super) struct ProductListingSchemaGenerationResponse {
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

impl ProductListingSchemaGenerationResponse {
    pub(super) fn try_into_single(
        self,
    ) -> Result<GeneratedSingleSchema, ProductListingSchemaResponseValidationError> {
        let evaluation = evaluation_from_response(self.confidence, &self.summary, &self.risks);
        let schema_count = self.schemas.len();
        match self.page_kind.unwrap_or(SinglePageKind::ProductListing) {
            SinglePageKind::ProductListing => {
                if schema_count != 1 {
                    return Err(
                        ProductListingSchemaResponseValidationError::SingleProductSchemaCount {
                            schema_count,
                        },
                    );
                }
                validate_schema_rules(&self.schemas)?;
                let Some(schema) = self.schemas.into_iter().next() else {
                    return Err(
                        ProductListingSchemaResponseValidationError::SingleProductSchemaCount {
                            schema_count,
                        },
                    );
                };
                Ok(GeneratedSingleSchema::ProductListing {
                    schema: Box::new(schema),
                    evaluation,
                })
            }
            SinglePageKind::Removed => {
                if schema_count != 0 {
                    return Err(
                        ProductListingSchemaResponseValidationError::RemovedContainsProductSchemas {
                            schema_count,
                        },
                    );
                }
                let Some(schema) = self.removed_schema else {
                    return Err(ProductListingSchemaResponseValidationError::RemovedMissingSchema);
                };
                if let Err(detail) = schema.validate_for_llm_response() {
                    return Err(
                        ProductListingSchemaResponseValidationError::InvalidRemovedSchema {
                            detail,
                        },
                    );
                }
                Ok(GeneratedSingleSchema::Removed { schema, evaluation })
            }
            SinglePageKind::NotProduct => {
                if schema_count != 0 {
                    return Err(
                        ProductListingSchemaResponseValidationError::NotProductContainsProductSchemas {
                            schema_count,
                        },
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
    serde_json::to_string_pretty(&schema_for!(ProductListingSchemasGenerationResponse))
        .unwrap_or_else(|_| "Failed to generate response schema".to_owned())
}

#[cfg(test)]
pub(super) fn parse_product_schemas_response(
    raw: &str,
) -> Result<GeneratedProductSchemas, serde_json::Error> {
    let response = serde_json::from_str::<ProductListingSchemasGenerationResponse>(raw)?;
    response.try_into_initial().map_err(invalid_data)
}

pub(super) fn single_schema_generation_response_json_schema() -> String {
    serde_json::to_string_pretty(&schema_for!(ProductListingSchemaGenerationResponse))
        .unwrap_or_else(|_| "Failed to generate response schema".to_owned())
}

#[cfg(test)]
pub(super) fn parse_single_schema_response(
    raw: &str,
) -> Result<GeneratedSingleSchema, serde_json::Error> {
    let response = serde_json::from_str::<ProductListingSchemaGenerationResponse>(raw)?;
    response.try_into_single().map_err(invalid_data)
}

#[cfg(test)]
fn invalid_data(error: ProductListingSchemaResponseValidationError) -> serde_json::Error {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::css_selector::rule::ExtractionRule;

    fn required_fields(schema: &serde_json::Value) -> Vec<&str> {
        schema["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .collect()
    }

    #[test]
    fn should_generate_required_product_schema_fields_for_vertex() {
        let schema: serde_json::Value =
            serde_json::from_str(&product_schema_generation_response_json_schema())
                .unwrap_or_else(|error| panic!("response schema should serialize: {error}"));
        let defs = schema["$defs"]
            .as_object()
            .unwrap_or_else(|| panic!("response schema should contain definitions"));
        let product = defs
            .get("ProductCssSelectorSchema")
            .unwrap_or_else(|| panic!("product schema definition should exist"));
        let required = required_fields(product);

        for field in ["title", "state", "images"] {
            assert!(required.contains(&field), "{field} should be required");
        }
        for field in ["description", "price", "source_listing_id"] {
            assert!(!required.contains(&field), "{field} should be optional");
        }

        let rule = defs
            .get("ExtractionRule")
            .unwrap_or_else(|| panic!("extraction rule definition should exist"));
        let rule_required = required_fields(rule);
        assert!(rule_required.contains(&"selector"));
        assert!(!rule_required.contains(&"additional_selectors"));
        assert!(!rule_required.contains(&"cardinality"));

        let variants = rule["oneOf"]
            .as_array()
            .unwrap_or_else(|| panic!("extraction kind should retain oneOf variants"));
        assert!(
            variants
                .iter()
                .all(|variant| { required_fields(variant).contains(&"type") })
        );
    }

    #[test]
    fn should_inspect_raw_flattened_extraction_rule_schema() {
        let schema = serde_json::to_value(schemars::schema_for!(ExtractionRule))
            .unwrap_or_else(|error| panic!("extraction rule schema should serialize: {error}"));
        let rule = schema["$defs"].get("ExtractionRule").unwrap_or(&schema);

        assert!(required_fields(rule).contains(&"selector"));
        let variants = rule["oneOf"]
            .as_array()
            .unwrap_or_else(|| panic!("flattened extraction kind should be represented by oneOf"));
        assert_eq!(variants.len(), 3);
        assert!(
            variants
                .iter()
                .all(|variant| required_fields(variant).contains(&"type"))
        );
        assert!(variants.iter().any(|variant| {
            let required = required_fields(variant);
            required.contains(&"type") && required.contains(&"name")
        }));

        let types = variants
            .iter()
            .filter_map(|variant| variant["properties"]["type"]["const"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(types, vec!["text", "attribute", "image_url"]);
    }

    fn complete_rule(selector: &str, extraction_type: &str) -> serde_json::Value {
        let mut rule = serde_json::json!({
            "selector": selector,
            "type": extraction_type,
        });
        if extraction_type == "attribute" {
            rule["name"] = serde_json::Value::String("data-id".to_owned());
        }
        rule
    }

    #[test]
    fn should_keep_extraction_rule_discriminator_required_during_local_validation() {
        assert!(
            serde_json::from_value::<ExtractionRule>(serde_json::json!({
                "selector": ".gallery img"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ExtractionRule>(complete_rule(".gallery img", "image_url"))
                .is_ok()
        );
        assert!(
            serde_json::from_value::<ExtractionRule>(complete_rule(".description", "text")).is_ok()
        );
        assert!(
            serde_json::from_value::<ExtractionRule>(complete_rule(
                "[data-product-id]",
                "attribute"
            ))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<ExtractionRule>(serde_json::json!({
                "selector": "[data-product-id]",
                "type": "attribute"
            }))
            .is_err()
        );
    }

    #[test]
    fn should_validate_optional_product_rules_when_present() {
        let mut product = serde_json::json!({
            "title": complete_rule("h1", "text"),
            "state": complete_rule(".state", "text"),
            "images": complete_rule(".gallery img", "image_url"),
        });
        assert!(serde_json::from_value::<ProductCssSelectorSchema>(product.clone()).is_ok());

        product["description"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<ProductCssSelectorSchema>(product.clone()).is_ok());

        product["description"] = serde_json::json!({"selector": ".description"});
        assert!(serde_json::from_value::<ProductCssSelectorSchema>(product).is_err());
    }

    fn valid_product_schema_json() -> serde_json::Value {
        serde_json::json!({
            "title": complete_rule("h1", "text"),
            "state": complete_rule(".state", "text"),
            "images": complete_rule(".gallery img", "image_url"),
        })
    }

    #[test]
    fn should_not_offer_page_classification_fields_in_initial_response_schema() {
        let schema: serde_json::Value =
            serde_json::from_str(&product_schema_generation_response_json_schema())
                .unwrap_or_else(|error| panic!("response schema should serialize: {error}"));
        let properties = schema["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("response schema should contain properties"));

        for field in ["page_kind", "removed_schema", "reason"] {
            assert!(
                !properties.contains_key(field),
                "initial response schema must not offer `{field}`"
            );
        }
        for field in ["schemas", "confidence", "summary", "risks"] {
            assert!(
                properties.contains_key(field),
                "`{field}` should be offered"
            );
        }
    }

    #[test]
    fn should_accept_initial_response_that_also_classifies_the_batch() {
        // Regression for #1902: the model answered the multi-page generation request with
        // a page classification because the shared response schema offered one.
        let payload = serde_json::json!({
            "page_kind": "not_product",
            "reason": "one sample looks like a sold listing",
            "schemas": [valid_product_schema_json()],
            "confidence": "MEDIUM",
            "summary": "schema for the product samples",
            "risks": ["sample 4 excluded"]
        })
        .to_string();

        let generated = parse_product_schemas_response(&payload)
            .unwrap_or_else(|error| panic!("initial response should be accepted: {error}"));

        assert_eq!(generated.schemas.len(), 1);
        assert_eq!(
            generated.evaluation.confidence,
            SchemaLlmEvaluationConfidence::Medium
        );
    }

    #[test]
    fn should_deserialize_complete_product_schema_generation_response() {
        let response = serde_json::json!({
            "schemas": [{
                "title": complete_rule("h1", "text"),
                "state": complete_rule(".state", "text"),
                "images": complete_rule(".gallery img", "image_url"),
                "description": null
            }],
            "confidence": "HIGH",
            "summary": "Complete product schema"
        });
        assert!(serde_json::from_value::<ProductListingSchemaGenerationResponse>(response).is_ok());
    }
}
