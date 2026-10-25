use crate::scraper::css_selector::product_schema::{
    ListingSourceProductSchema, ProductCssSelectorSchema,
};
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crate::scraper::css_selector::rule::ExtractionRule;

use super::ReviewRepositoryError;
use serde::{Deserialize, Serialize};

/// The candidate a product-schema review holds, as its `candidate_payload` stores it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProductSchemaReviewCandidate {
    pub(super) schemas: Vec<ProductCssSelectorSchema>,
    /// The ListingSource's removed-page templates as the reviewer would have them: the review
    /// opens with the stored ones, and approval replaces the stored list with this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) removed_page_schemas: Vec<RemovedPageSchema>,
}

impl ProductSchemaReviewCandidate {
    /// Decodes and validates a candidate payload.
    pub(super) fn parse(
        candidate_payload: &serde_json::Value,
    ) -> Result<Self, ReviewRepositoryError> {
        let candidate = serde_json::from_value::<Self>(candidate_payload.clone())
            .map_err(|_| ReviewRepositoryError::InvalidProductSchemaCandidate)?;
        validate_product_schemas(&candidate.schemas)?;
        validate_removed_page_schemas(&candidate.removed_page_schemas)?;
        Ok(candidate)
    }

    pub(super) fn new(
        schemas: &[ProductCssSelectorSchema],
        removed_page_schemas: Vec<RemovedPageSchema>,
    ) -> Self {
        Self {
            schemas: schemas.to_vec(),
            removed_page_schemas,
        }
    }

    /// Decodes a stored candidate without validating it, to compare it with the live schemas: a
    /// candidate that today's validation rejects is still a fact about what was approved.
    pub(super) fn decode_stored(candidate_payload: &serde_json::Value) -> Option<Self> {
        serde_json::from_value::<Self>(candidate_payload.clone()).ok()
    }

    /// This candidate holding `schemas` instead of its own.
    pub(super) fn with_schemas(self, schemas: Vec<ProductCssSelectorSchema>) -> Self {
        Self { schemas, ..self }
    }

    /// This candidate holding `removed_page_schemas` instead of its own.
    pub(super) fn with_removed_page_schemas(
        self,
        removed_page_schemas: Vec<RemovedPageSchema>,
    ) -> Self {
        Self {
            removed_page_schemas,
            ..self
        }
    }

    pub(super) fn to_payload(&self) -> Result<serde_json::Value, ReviewRepositoryError> {
        Ok(serde_json::to_value(self)?)
    }
}

pub(super) fn validate_product_schemas(
    schemas: &[ProductCssSelectorSchema],
) -> Result<(), ReviewRepositoryError> {
    if schemas.is_empty() || ProductCssSelectorSchema::validate_all(schemas).is_err() {
        return Err(ReviewRepositoryError::InvalidProductSchemaCandidate);
    }
    Ok(())
}

/// Validates removed-page templates against the contract a generated template meets.
fn validate_removed_page_schemas(
    removed_page_schemas: &[RemovedPageSchema],
) -> Result<(), ReviewRepositoryError> {
    removed_page_schemas
        .iter()
        .enumerate()
        .try_for_each(|(index, schema)| {
            schema
                .validate()
                .map_err(|reason| ReviewRepositoryError::InvalidRemovedPageTemplate {
                    index,
                    reason,
                })
        })
}

pub(super) fn approval_product_schemas(
    reason: &str,
    existing: Option<&ListingSourceProductSchema>,
    reviewed_schemas: Vec<ProductCssSelectorSchema>,
) -> Result<Vec<ProductCssSelectorSchema>, ReviewRepositoryError> {
    let should_backfill_existing = matches!(
        reason,
        "append_schema_generation" | "normalization_schema_repair"
    ) && reviewed_schemas.len() == 1;

    let schemas = if !should_backfill_existing {
        reviewed_schemas
    } else if let Some(existing) = existing {
        merge_product_schema_lists(&existing.product_schemas, reviewed_schemas)?
    } else {
        reviewed_schemas
    };
    validate_product_schemas(&schemas)?;
    Ok(schemas)
}

/// The candidate in `candidate_payload` with `field` of the schema at `schema_index` set to
/// `rule`, validated.
pub(super) fn update_schema_field_payload(
    candidate_payload: &serde_json::Value,
    schema_index: usize,
    field: &str,
    rule: Option<ExtractionRule>,
) -> Result<ProductSchemaReviewCandidate, ReviewRepositoryError> {
    let candidate = ProductSchemaReviewCandidate::parse(candidate_payload)?;
    let mut schemas = candidate.schemas.clone();
    let Some(schema) = schemas.get_mut(schema_index) else {
        return Err(ReviewRepositoryError::InvalidSchemaField(format!(
            "schema index {schema_index}"
        )));
    };

    update_schema_rule(schema, field, rule)?;
    validate_product_schemas(&schemas)?;
    Ok(candidate.with_schemas(schemas))
}

fn update_schema_rule(
    schema: &mut ProductCssSelectorSchema,
    field: &str,
    rule: Option<ExtractionRule>,
) -> Result<(), ReviewRepositoryError> {
    match field {
        "source_listing_id" => schema.source_listing_id = rule,
        "title" => {
            schema.title =
                rule.ok_or_else(|| ReviewRepositoryError::RequiredSchemaField(field.into()))?;
        }
        "state" => {
            schema.state =
                rule.ok_or_else(|| ReviewRepositoryError::RequiredSchemaField(field.into()))?;
        }
        "images" => {
            schema.images =
                rule.ok_or_else(|| ReviewRepositoryError::RequiredSchemaField(field.into()))?;
        }
        "description" => schema.description = rule,
        "price" => schema.price = rule,
        "price_estimate_min" => schema.price_estimate_min = rule,
        "price_estimate_max" => schema.price_estimate_max = rule,
        other => return Err(ReviewRepositoryError::InvalidSchemaField(other.into())),
    }
    Ok(())
}

fn merge_product_schema_lists(
    existing_schemas: &[ProductCssSelectorSchema],
    reviewed_schemas: Vec<ProductCssSelectorSchema>,
) -> Result<Vec<ProductCssSelectorSchema>, ReviewRepositoryError> {
    let mut merged = existing_schemas.to_vec();
    let mut seen = existing_schemas
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()?;

    for schema in reviewed_schemas {
        let key = serde_json::to_value(&schema)?;
        if !seen.contains(&key) {
            seen.push(key);
            merged.push(schema);
        }
    }

    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::css_selector::rule::{
        CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule,
    };

    fn text_rule(selector: &str) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: vec![],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        }
    }

    fn image_rule(selector: &str) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: vec![],
            extract: ExtractionKind::Attribute { name: "src".into() },
            cardinality: ExtractionCardinality::All,
        }
    }

    fn schema(title_selector: &str) -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule(title_selector),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: image_rule("img"),
            raw_attributes: Default::default(),
        }
    }

    #[test]
    fn rejects_missing_empty_and_unknown_product_schema_candidate_fields() {
        for payload in [
            serde_json::json!({}),
            serde_json::json!({ "schemas": [] }),
            serde_json::json!({ "schemas": [], "unexpected": true }),
        ] {
            assert!(matches!(
                ProductSchemaReviewCandidate::parse(&payload),
                Err(ReviewRepositoryError::InvalidProductSchemaCandidate)
            ));
        }
    }

    fn removed_page_template(selector: &str, regex: &str) -> serde_json::Value {
        serde_json::json!({ "selector": selector, "regex": regex })
    }

    #[test]
    fn should_parse_a_candidate_with_a_removed_page_template_and_keep_it_through_field_edits() {
        let payload = serde_json::json!({
            "schemas": [schema("h1.product")],
            "removed_page_schemas": [removed_page_template(".notice", "no longer available")],
        });

        let edited = update_schema_field_payload(&payload, 0, "price", Some(text_rule(".price")))
            .expect("field edit keeps a valid candidate");

        assert_eq!(edited.removed_page_schemas.len(), 1);
        assert_eq!(
            edited
                .to_payload()
                .ok()
                .map(|payload| payload["removed_page_schemas"].clone()),
            Some(payload["removed_page_schemas"].clone())
        );
    }

    #[test]
    fn should_store_a_candidate_without_templates_in_its_previous_shape() {
        let payload = serde_json::json!({ "schemas": [schema("h1.product")] });

        let candidate = ProductSchemaReviewCandidate::parse(&payload).expect("valid candidate");

        assert_eq!(candidate.to_payload().ok(), Some(payload));
    }

    #[test]
    fn should_reject_a_removed_page_template_the_generated_contract_rejects() {
        for (index, template) in [
            removed_page_template(".notice", ".*"),
            removed_page_template("[", "no longer available"),
            serde_json::json!({ "selector": ".notice" }),
            serde_json::json!({ "selector": ".notice", "text": "sold", "regex": "sold" }),
        ]
        .into_iter()
        .enumerate()
        {
            let payload = serde_json::json!({
                "schemas": [schema("h1.product")],
                "removed_page_schemas": [template],
            });

            assert!(
                matches!(
                    ProductSchemaReviewCandidate::parse(&payload),
                    Err(ReviewRepositoryError::InvalidRemovedPageTemplate { index: 0, .. })
                ),
                "template {index} must be rejected"
            );
        }
    }

    #[test]
    fn parses_non_empty_product_schema_candidate() {
        let payload = serde_json::json!({ "schemas": [schema("h1.product")] });

        let parsed = ProductSchemaReviewCandidate::parse(&payload);

        assert!(matches!(parsed, Ok(candidate) if candidate.schemas.len() == 1));
    }

    #[test]
    fn rejects_invalid_primary_and_additional_css_selectors() {
        let invalid_primary = serde_json::json!({ "schemas": [schema("[")] });
        let mut invalid_additional_schema = schema("h1.product");
        invalid_additional_schema
            .title
            .additional_selectors
            .push(CssSelector::from("["));
        let invalid_additional = serde_json::json!({ "schemas": [invalid_additional_schema] });

        for payload in [invalid_primary, invalid_additional] {
            assert!(matches!(
                ProductSchemaReviewCandidate::parse(&payload),
                Err(ReviewRepositoryError::InvalidProductSchemaCandidate)
            ));
        }
    }

    #[test]
    fn merge_product_schema_lists_appends_new_schema_after_existing_schemas() {
        let existing_a = schema("h1.template-a");
        let existing_b = schema("h1.template-b");
        let appended = schema("h1.template-c");

        let merged = merge_product_schema_lists(
            &[existing_a.clone(), existing_b.clone()],
            vec![appended.clone()],
        )
        .expect("schema merge should serialize");

        assert_eq!(merged, vec![existing_a, existing_b, appended]);
    }

    #[test]
    fn merge_product_schema_lists_deduplicates_existing_schema() {
        let existing = schema("h1.template-a");

        let merged =
            merge_product_schema_lists(std::slice::from_ref(&existing), vec![existing.clone()])
                .expect("schema merge should serialize");

        assert_eq!(merged, vec![existing]);
    }

    #[test]
    fn update_schema_rule_deletes_optional_rule() {
        let mut schema = schema("h1.template-a");
        schema.price = Some(text_rule(".price"));

        let candidate = update_schema_field_payload(
            &serde_json::json!({ "schemas": [schema] }),
            0,
            "price",
            None,
        )
        .expect("optional price rule can be deleted");

        assert!(candidate.schemas[0].price.is_none());
    }

    #[test]
    fn update_schema_rule_rejects_required_rule_deletion() {
        let schema = schema("h1.template-a");

        let err = update_schema_field_payload(
            &serde_json::json!({ "schemas": [schema] }),
            0,
            "title",
            None,
        )
        .unwrap_err();

        assert!(
            matches!(err, ReviewRepositoryError::RequiredSchemaField(field) if field == "title")
        );
    }

    #[test]
    fn update_schema_field_payload_rejects_invalid_new_rule() {
        let err = update_schema_field_payload(
            &serde_json::json!({ "schemas": [schema("h1.template-a")] }),
            0,
            "title",
            Some(text_rule("[")),
        )
        .unwrap_err();

        assert!(matches!(
            err,
            ReviewRepositoryError::InvalidProductSchemaCandidate
        ));
    }

    #[test]
    fn update_schema_field_payload_rejects_empty_schema_payload() {
        let err =
            update_schema_field_payload(&serde_json::json!({ "schemas": [] }), 0, "price", None)
                .unwrap_err();

        assert!(matches!(
            err,
            ReviewRepositoryError::InvalidProductSchemaCandidate
        ));
    }
}
