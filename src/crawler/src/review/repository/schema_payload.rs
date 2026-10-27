use crate::scraper::css_selector::product_schema::{
    ListingSourceProductSchema, ProductCssSelectorSchema,
};
use crate::scraper::css_selector::raw_attribute_registry::is_registered_key;
use crate::scraper::css_selector::removed_page_schema::{
    RemovedPageSchema, rebase_reviewed_removed_page_schemas,
};
use crate::scraper::css_selector::rule::ExtractionRule;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

use super::ReviewRepositoryError;
use serde::{Deserialize, Deserializer, Serialize};

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
    /// Decodes and validates a candidate payload, templates included, as approval needs it.
    pub(super) fn parse(
        candidate_payload: &serde_json::Value,
    ) -> Result<Self, ReviewRepositoryError> {
        let candidate = Self::read(candidate_payload)?;
        validate_removed_page_schemas(&candidate.removed_page_schemas)?;
        Ok(candidate)
    }

    /// Decodes a candidate to show, evaluate or edit it, validating its schemas but not its
    /// removed-page templates: the review opens with the ListingSource's stored templates, and
    /// one that today's validation rejects must stay visible for the reviewer to fix or retire,
    /// without blocking edits that leave it as it is. Approving the candidate, or saving changed
    /// templates, still refuses it.
    pub(super) fn read(
        candidate_payload: &serde_json::Value,
    ) -> Result<Self, ReviewRepositoryError> {
        let candidate = serde_json::from_value::<Self>(candidate_payload.clone())
            .map_err(|_| ReviewRepositoryError::InvalidProductSchemaCandidate)?;
        validate_product_schemas(&candidate.schemas)?;
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

    /// Decodes a stored candidate without validating it, to compare it with the live schemas or
    /// to apply an edit to it: a candidate that today's validation rejects is still a fact about
    /// what was approved or what the reviewer has so far.
    pub(super) fn decode_stored(
        candidate_payload: &serde_json::Value,
    ) -> Result<Self, serde_json::Error> {
        serde_json::from_value::<Self>(candidate_payload.clone())
    }

    /// This candidate holding `schemas` instead of its own.
    pub(super) fn with_schemas(self, schemas: Vec<ProductCssSelectorSchema>) -> Self {
        Self { schemas, ..self }
    }

    pub(super) fn to_payload(&self) -> Result<serde_json::Value, ReviewRepositoryError> {
        Ok(serde_json::to_value(self)?)
    }

    /// SHA-256 of this candidate's schemas and removed-page templates, equal for equal
    /// candidates: their serialization keeps field order and sorts raw attribute keys.
    pub(super) fn fingerprint(&self) -> Result<String, ReviewRepositoryError> {
        Ok(Sha256::digest(serde_json::to_vec(self)?)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }

    /// The product-schema configuration approving this reviewed candidate makes live: the schema
    /// list approval stores, merged with the `live` list for a legacy single-schema append or
    /// repair, and the removed-page templates rebased onto the changes other writers made to the
    /// `stored` list since the review read it as `base`. The live matrix evaluates exactly this,
    /// and approval stores exactly this.
    pub(super) fn going_live(
        &self,
        reason: &str,
        live: Option<&ListingSourceProductSchema>,
        base: &[RemovedPageSchema],
        stored: &[RemovedPageSchema],
    ) -> Result<Self, ReviewRepositoryError> {
        let schemas = approval_product_schemas(reason, live, self.schemas.clone())?;
        let removed_page_schemas =
            rebase_reviewed_removed_page_schemas(base, self.removed_page_schemas.clone(), stored);
        Ok(Self {
            schemas,
            removed_page_schemas,
        })
    }
}

/// A reviewer's edit of a product-schema review's candidate, as `POST
/// /api/reviews/{id}/candidate` sends it: the schemas that replace the candidate's and, when the
/// key is present, the removed-page templates that replace its templates.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProductSchemaCandidateEdit {
    schemas: Vec<ProductCssSelectorSchema>,
    /// Left out, the candidate keeps its templates, so an edit of the schemas alone never
    /// retires them; an empty list retires every one.
    #[serde(default, deserialize_with = "present")]
    removed_page_schemas: Option<Vec<RemovedPageSchema>>,
}

impl ProductSchemaCandidateEdit {
    /// Decodes an edit and validates its schemas.
    pub(super) fn parse(payload: &serde_json::Value) -> Result<Self, ReviewRepositoryError> {
        let edit = serde_json::from_value::<Self>(payload.clone())
            .map_err(|_| ReviewRepositoryError::InvalidProductSchemaCandidate)?;
        validate_product_schemas(&edit.schemas)?;
        Ok(edit)
    }

    /// The candidate this edit makes of `current`: the edit's schemas, and its templates when
    /// it carries a list that differs from `current`'s, validated; otherwise `current`'s
    /// templates as they are, so one that today's validation rejects refuses approval but not an
    /// edit that leaves it alone. The schemas may introduce only registered raw attribute keys.
    pub(super) fn apply_to(
        self,
        current: &ProductSchemaReviewCandidate,
    ) -> Result<ProductSchemaReviewCandidate, ReviewRepositoryError> {
        refuse_unregistered_raw_attribute_keys(&current.schemas, &self.schemas)?;
        let removed_page_schemas = match self.removed_page_schemas {
            Some(templates) if templates != current.removed_page_schemas => {
                validate_removed_page_schemas(&templates)?;
                templates
            }
            _ => current.removed_page_schemas.clone(),
        };
        Ok(ProductSchemaReviewCandidate {
            schemas: self.schemas,
            removed_page_schemas,
        })
    }
}

/// Decodes a key that is present, so an explicit `null` is refused like any other malformed
/// value instead of reading as a left-out key.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
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

/// Whether the stored candidate of an approved review is the ListingSource's `live` schema list,
/// the condition for editing the live list through that review. A stored candidate that no longer
/// decodes is not provably the live list, and no list is live before the first approval.
pub(super) fn holds_live_schemas(
    stored_candidate_payload: &serde_json::Value,
    live: Option<&[ProductCssSelectorSchema]>,
) -> bool {
    live.is_some_and(|live| {
        ProductSchemaReviewCandidate::decode_stored(stored_candidate_payload)
            .is_ok_and(|stored| stored.schemas == live)
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
/// `rule`, its schemas validated and introducing only registered raw attribute keys. Its
/// removed-page templates pass through as they are.
pub(super) fn update_schema_field_payload(
    candidate_payload: &serde_json::Value,
    schema_index: usize,
    field: &str,
    rule: Option<ExtractionRule>,
) -> Result<ProductSchemaReviewCandidate, ReviewRepositoryError> {
    let candidate = ProductSchemaReviewCandidate::read(candidate_payload)?;
    let mut schemas = candidate.schemas.clone();
    let Some(schema) = schemas.get_mut(schema_index) else {
        return Err(ReviewRepositoryError::InvalidSchemaField(format!(
            "schema index {schema_index}"
        )));
    };

    SchemaField::parse(field)?.set_rule(schema, rule)?;
    validate_product_schemas(&schemas)?;
    refuse_unregistered_raw_attribute_keys(&candidate.schemas, &schemas)?;
    Ok(candidate.with_schemas(schemas))
}

/// Refuses `edited` schemas that introduce a raw attribute key outside the registry. A key one of
/// the `current` schemas already holds, such as one generated before the registry named its keys,
/// stays editable and removable.
fn refuse_unregistered_raw_attribute_keys(
    current: &[ProductCssSelectorSchema],
    edited: &[ProductCssSelectorSchema],
) -> Result<(), ReviewRepositoryError> {
    let current_keys: BTreeSet<&str> = current
        .iter()
        .flat_map(|schema| schema.raw_attributes.keys().map(String::as_str))
        .collect();
    match edited
        .iter()
        .flat_map(|schema| schema.raw_attributes.keys())
        .find(|key| !is_registered_key(key) && !current_keys.contains(key.as_str()))
    {
        Some(key) => Err(ReviewRepositoryError::UnregisteredRawAttributeKey(
            key.clone(),
        )),
        None => Ok(()),
    }
}

/// A rule of a product schema that a field edit sets or deletes, named on the wire by the
/// schema's JSON key, a raw attribute as `raw_attributes.<key>`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SchemaField {
    SourceListingId,
    Title,
    Description,
    Price,
    PriceEstimateMin,
    PriceEstimateMax,
    State,
    Images,
    RawAttribute(String),
}

const RAW_ATTRIBUTE_FIELD_PREFIX: &str = "raw_attributes.";

impl SchemaField {
    fn parse(field: &str) -> Result<Self, ReviewRepositoryError> {
        if let Some(key) = field.strip_prefix(RAW_ATTRIBUTE_FIELD_PREFIX) {
            if key.is_empty() || key.trim() != key {
                return Err(ReviewRepositoryError::InvalidSchemaField(field.into()));
            }
            return Ok(Self::RawAttribute(key.to_owned()));
        }
        Ok(match field {
            "source_listing_id" => Self::SourceListingId,
            "title" => Self::Title,
            "description" => Self::Description,
            "price" => Self::Price,
            "price_estimate_min" => Self::PriceEstimateMin,
            "price_estimate_max" => Self::PriceEstimateMax,
            "state" => Self::State,
            "images" => Self::Images,
            other => return Err(ReviewRepositoryError::InvalidSchemaField(other.into())),
        })
    }

    fn name(&self) -> &str {
        match self {
            Self::SourceListingId => "source_listing_id",
            Self::Title => "title",
            Self::Description => "description",
            Self::Price => "price",
            Self::PriceEstimateMin => "price_estimate_min",
            Self::PriceEstimateMax => "price_estimate_max",
            Self::State => "state",
            Self::Images => "images",
            Self::RawAttribute(key) => key,
        }
    }

    /// Sets this field of `schema` to `rule`, or deletes it when `rule` is `None`; a required
    /// field cannot be deleted.
    fn set_rule(
        &self,
        schema: &mut ProductCssSelectorSchema,
        rule: Option<ExtractionRule>,
    ) -> Result<(), ReviewRepositoryError> {
        let required =
            |rule: Option<ExtractionRule>| rule.ok_or_else(|| self.required_field_error());
        match self {
            Self::SourceListingId => schema.source_listing_id = rule,
            Self::Title => schema.title = required(rule)?,
            Self::Description => schema.description = rule,
            Self::Price => schema.price = rule,
            Self::PriceEstimateMin => schema.price_estimate_min = rule,
            Self::PriceEstimateMax => schema.price_estimate_max = rule,
            Self::State => schema.state = required(rule)?,
            Self::Images => schema.images = required(rule)?,
            Self::RawAttribute(key) => match rule {
                Some(rule) => {
                    schema.raw_attributes.insert(key.clone(), rule);
                }
                None => {
                    schema.raw_attributes.remove(key);
                }
            },
        }
        Ok(())
    }

    fn required_field_error(&self) -> ReviewRepositoryError {
        ReviewRepositoryError::RequiredSchemaField(self.name().into())
    }
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

    /// A candidate holding `templates`, decoded the way an edit reads the stored candidate.
    fn current_candidate(templates: serde_json::Value) -> ProductSchemaReviewCandidate {
        ProductSchemaReviewCandidate::read(&serde_json::json!({
            "schemas": [schema("h1.product")],
            "removed_page_schemas": templates,
        }))
        .expect("readable candidate")
    }

    fn edit(payload: serde_json::Value) -> ProductSchemaCandidateEdit {
        ProductSchemaCandidateEdit::parse(&payload).expect("valid edit")
    }

    #[test]
    fn should_keep_the_current_templates_when_an_edit_leaves_them_out() {
        let current =
            current_candidate(serde_json::json!([removed_page_template(".notice", ".*")]));

        let edited = edit(serde_json::json!({ "schemas": [schema("h1.edited")] }))
            .apply_to(&current)
            .expect("an edit of the schemas alone keeps the templates unvalidated");

        assert_eq!(edited.schemas, vec![schema("h1.edited")]);
        assert_eq!(edited.removed_page_schemas, current.removed_page_schemas);
    }

    #[test]
    fn should_retire_every_template_when_an_edit_sends_an_empty_list() {
        let current = current_candidate(serde_json::json!([removed_page_template(
            ".notice",
            "no longer available"
        )]));

        let edited = edit(serde_json::json!({
            "schemas": [schema("h1.product")],
            "removed_page_schemas": [],
        }))
        .apply_to(&current)
        .expect("an empty list is a valid replacement");

        assert!(edited.removed_page_schemas.is_empty());
    }

    #[test]
    fn should_validate_templates_only_when_an_edit_changes_them() {
        let rejected = removed_page_template(".notice", ".*");
        let current = current_candidate(serde_json::json!([rejected.clone()]));

        let unchanged = edit(serde_json::json!({
            "schemas": [schema("h1.edited")],
            "removed_page_schemas": [rejected.clone()],
        }))
        .apply_to(&current);
        let changed = edit(serde_json::json!({
            "schemas": [schema("h1.product")],
            "removed_page_schemas": [rejected, removed_page_template(".notice", "sold")],
        }))
        .apply_to(&current);

        assert!(
            matches!(unchanged, Ok(candidate) if candidate.removed_page_schemas == current.removed_page_schemas)
        );
        assert!(matches!(
            changed,
            Err(ReviewRepositoryError::InvalidRemovedPageTemplate { index: 0, .. })
        ));
    }

    #[test]
    fn should_refuse_an_edit_with_null_templates_or_invalid_schemas() {
        for payload in [
            serde_json::json!({ "schemas": [schema("h1.product")], "removed_page_schemas": null }),
            serde_json::json!({ "schemas": [] }),
            serde_json::json!({ "schemas": [schema("[")] }),
            serde_json::json!({ "schemas": [schema("h1.product")], "unexpected": true }),
        ] {
            assert!(
                matches!(
                    ProductSchemaCandidateEdit::parse(&payload),
                    Err(ReviewRepositoryError::InvalidProductSchemaCandidate)
                ),
                "{payload} must be refused"
            );
        }
    }

    #[test]
    fn should_keep_a_template_validation_rejects_through_a_field_edit() {
        let payload = serde_json::json!({
            "schemas": [schema("h1.product")],
            "removed_page_schemas": [removed_page_template(".notice", ".*")],
        });

        let edited = update_schema_field_payload(&payload, 0, "price", Some(text_rule(".price")))
            .expect("a field edit leaves the templates alone");

        assert_eq!(
            edited
                .to_payload()
                .ok()
                .map(|payload| payload["removed_page_schemas"].clone()),
            Some(payload["removed_page_schemas"].clone())
        );
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

    fn with_raw_attribute(
        mut schema: ProductCssSelectorSchema,
        key: &str,
        selector: &str,
    ) -> ProductCssSelectorSchema {
        schema
            .raw_attributes
            .insert(key.to_owned(), text_rule(selector));
        schema
    }

    #[test]
    fn should_set_and_delete_a_raw_attribute_rule_when_the_field_names_its_key() {
        let payload = serde_json::json!({ "schemas": [schema("h1.product")] });

        let added = update_schema_field_payload(
            &payload,
            0,
            "raw_attributes.rawMaterial",
            Some(text_rule(".material")),
        )
        .expect("a registered key can be added");
        let deleted = update_schema_field_payload(
            &added.to_payload().expect("serializable candidate"),
            0,
            "raw_attributes.rawMaterial",
            None,
        )
        .expect("a raw attribute rule can be deleted");

        assert_eq!(
            added.schemas[0].raw_attributes.get("rawMaterial"),
            Some(&text_rule(".material"))
        );
        assert!(deleted.schemas[0].raw_attributes.is_empty());
    }

    #[test]
    fn should_refuse_a_raw_attribute_field_without_a_key() {
        for field in [
            "raw_attributes.",
            "raw_attributes. rawMaterial",
            "raw_attributes",
        ] {
            let result = update_schema_field_payload(
                &serde_json::json!({ "schemas": [schema("h1.product")] }),
                0,
                field,
                Some(text_rule(".material")),
            );

            assert!(
                matches!(result, Err(ReviewRepositoryError::InvalidSchemaField(_))),
                "{field} must be refused"
            );
        }
    }

    #[test]
    fn should_refuse_a_field_edit_that_adds_an_unregistered_raw_attribute_key() {
        let result = update_schema_field_payload(
            &serde_json::json!({ "schemas": [schema("h1.product")] }),
            0,
            "raw_attributes.rawFoo",
            Some(text_rule(".foo")),
        );

        assert!(matches!(
            result,
            Err(ReviewRepositoryError::UnregisteredRawAttributeKey(key)) if key == "rawFoo"
        ));
    }

    #[test]
    fn should_keep_an_unregistered_raw_attribute_key_editable_when_the_candidate_holds_it() {
        let payload = serde_json::json!({
            "schemas": [with_raw_attribute(schema("h1.product"), "rawLegacy", ".legacy"), schema("h1.sold")],
        });

        let edited = update_schema_field_payload(
            &payload,
            1,
            "raw_attributes.rawLegacy",
            Some(text_rule(".legacy-sold")),
        )
        .expect("a key the candidate holds stays editable");
        let deleted = update_schema_field_payload(&payload, 0, "raw_attributes.rawLegacy", None)
            .expect("a key the candidate holds stays removable");

        assert_eq!(
            edited.schemas[1].raw_attributes.get("rawLegacy"),
            Some(&text_rule(".legacy-sold"))
        );
        assert!(deleted.schemas[0].raw_attributes.is_empty());
    }

    #[test]
    fn should_refuse_a_candidate_edit_that_introduces_an_unregistered_raw_attribute_key() {
        let current = ProductSchemaReviewCandidate::read(&serde_json::json!({
            "schemas": [with_raw_attribute(schema("h1.product"), "rawLegacy", ".legacy")],
        }))
        .expect("readable candidate");

        let kept = edit(serde_json::json!({
            "schemas": [
                with_raw_attribute(schema("h1.product"), "rawLegacy", ".legacy"),
                with_raw_attribute(schema("h1.sold"), "rawCondition", ".condition"),
            ],
        }))
        .apply_to(&current);
        let introduced = edit(serde_json::json!({
            "schemas": [with_raw_attribute(schema("h1.product"), "rawInvented", ".invented")],
        }))
        .apply_to(&current);

        assert!(matches!(kept, Ok(candidate) if candidate.schemas.len() == 2));
        assert!(matches!(
            introduced,
            Err(ReviewRepositoryError::UnregisteredRawAttributeKey(key)) if key == "rawInvented"
        ));
    }

    #[test]
    fn should_hold_the_live_schemas_only_when_the_stored_candidate_is_the_live_list() {
        let stored = serde_json::json!({ "schemas": [schema("h1.product")] });
        let live = [schema("h1.product")];
        let changed = [schema("h1.product"), schema("h1.sold")];

        assert!(holds_live_schemas(&stored, Some(&live)));
        assert!(!holds_live_schemas(&stored, Some(&changed)));
        assert!(!holds_live_schemas(&stored, None));
        assert!(!holds_live_schemas(
            &serde_json::json!({ "schemas": [], "unexpected": true }),
            Some(&live)
        ));
    }
}
