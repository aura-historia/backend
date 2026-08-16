use crate::scraper::css_selector::product_schema::{ProductCssSelectorSchema, ShopsProductSchema};
use crate::scraper::css_selector::rule::ExtractionRule;

use super::ReviewRepositoryError;

pub(super) fn parse_schemas_payload(
    candidate_payload: &serde_json::Value,
) -> Result<Vec<ProductCssSelectorSchema>, serde_json::Error> {
    serde_json::from_value(
        candidate_payload
            .get("schemas")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::Array(Vec::new())),
    )
}

pub(super) fn approval_product_schemas(
    reason: &str,
    existing: Option<&ShopsProductSchema>,
    reviewed_schemas: Vec<ProductCssSelectorSchema>,
) -> Result<Vec<ProductCssSelectorSchema>, ReviewRepositoryError> {
    let should_backfill_existing = matches!(
        reason,
        "append_schema_generation" | "normalization_schema_repair"
    ) && reviewed_schemas.len() == 1;

    if !should_backfill_existing {
        return Ok(reviewed_schemas);
    }

    let Some(existing) = existing else {
        return Ok(reviewed_schemas);
    };

    merge_product_schema_lists(&existing.product_schemas, reviewed_schemas)
}

pub(super) fn update_schema_rule(
    schema: &mut ProductCssSelectorSchema,
    field: &str,
    rule: Option<ExtractionRule>,
) -> Result<(), ReviewRepositoryError> {
    match field {
        "shops_product_id" => schema.shops_product_id = rule,
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
        "seller_name" => schema.seller_name = rule,
        "auction_start" => schema.auction_start = rule,
        "auction_end" => schema.auction_end = rule,
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
    use crate::scraper::css_selector::currency_dto::CurrencyDto;
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
            shops_product_id: Some(text_rule("#product-id")),
            title: text_rule(title_selector),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            seller_name: None,
            state: text_rule("#state"),
            images: image_rule("img"),
            auction_start: None,
            auction_end: None,
            default_currency: CurrencyDto::Eur,
            raw_attributes: Default::default(),
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

        update_schema_rule(&mut schema, "price", None).expect("optional price rule can be deleted");

        assert!(schema.price.is_none());
    }

    #[test]
    fn update_schema_rule_rejects_required_rule_deletion() {
        let mut schema = schema("h1.template-a");

        let err = update_schema_rule(&mut schema, "title", None).unwrap_err();

        assert!(
            matches!(err, ReviewRepositoryError::RequiredSchemaField(field) if field == "title")
        );
    }
}
