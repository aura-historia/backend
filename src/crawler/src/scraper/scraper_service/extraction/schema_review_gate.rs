use crate::CrawlerReviewId;
use crate::authoring::CrawlerAuthoringMode;
use crate::review::model::SchemaMatrix;
use crate::review::model::{STATUS_APPROVED, SchemaReviewPageInput};
use crate::review::repository::SchemaReviewWithStatusInput;
use crate::review::schema_evaluation::{
    evaluate_schema_matrix_for_inputs, implausible_pages, optional_rules_missing_on_every_page,
    schema_matrix_deterministic_approval_ok, selector_field_is_soft,
    selector_field_matched_nothing, unused_schema_indices,
};
use crate::scraper::css_selector::product_schema::{
    ListingSourceProductSchema, ProductCssSelectorSchema,
};
use crate::scraper::css_selector::product_schema_service::{
    ProductListingSchemaServiceError, SchemaLlmEvaluation,
};
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use listing_source_core::ListingSourceId;
use serde_json::{Value, json};
use tracing::{info, warn};

pub(crate) enum GeneratedSchemaReviewOutcome {
    Persisted(ListingSourceProductSchema),
    PendingReview(CrawlerReviewId),
}

impl ScraperServiceImpl {
    #[allow(clippy::result_large_err)]
    pub(crate) async fn handle_generated_schema_review(
        &self,
        listing_source_id: &ListingSourceId,
        reason: &str,
        schemas: Vec<ProductCssSelectorSchema>,
        evaluation: SchemaLlmEvaluation,
        pages: Vec<SchemaReviewPageInput>,
        validation_summary: Value,
    ) -> Result<GeneratedSchemaReviewOutcome, ScraperError> {
        if let Err(error) = ProductCssSelectorSchema::validate_all(&schemas) {
            warn!(
                listing_source_id = %listing_source_id,
                reason,
                schema_count = schemas.len(),
                error = %error,
                "Generated product schema failed deterministic validation; not persisting review"
            );
            return Err(ProductListingSchemaServiceError::InvalidGeneratedSchema {
                source: application::error::box_error(error),
            }
            .into());
        }
        let Some(review_repository) = &self.review_repository else {
            let saved = self
                .schema_service
                .save_product_schemas(listing_source_id, schemas)
                .await?;
            return Ok(GeneratedSchemaReviewOutcome::Persisted(saved));
        };
        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);
        let deterministic_approval_ok = schema_matrix_deterministic_approval_ok(&matrix);
        let mut validation_summary =
            with_schema_matrix_summary(validation_summary, &matrix, deterministic_approval_ok);
        let approved_by_llm = should_auto_approve_generated_schema(
            self.authoring,
            deterministic_approval_ok,
            &evaluation,
        );
        let evaluation = evaluation.with_approved_by_llm(approved_by_llm);
        validation_summary = with_auto_schema_evaluation(validation_summary, &evaluation);

        // `llm_decision` is the LLM's self-reported verdict; `outcome` is the final gate
        // result after deterministic coverage, so a schema that failed coverage never reads
        // as approved.
        info!(
            mode = self.authoring.as_str(),
            outcome = if approved_by_llm {
                "auto_approved"
            } else {
                "pending_review"
            },
            llm_decision = ?evaluation.decision,
            confidence = ?evaluation.confidence,
            approved_by_llm,
            deterministic_approval_ok,
            "Schema generation confidence evaluated"
        );

        if approved_by_llm {
            let saved = self
                .schema_service
                .save_product_schemas(listing_source_id, schemas.clone())
                .await?;
            review_repository
                .create_schema_review_with_status(SchemaReviewWithStatusInput {
                    listing_source_id,
                    reason,
                    schemas: &schemas,
                    pages,
                    validation_summary,
                    status: STATUS_APPROVED,
                    notes: Some("Auto-approved by LLM schema generation confidence"),
                })
                .await
                .map_err(review_error_to_schema_service_error)?;
            return Ok(GeneratedSchemaReviewOutcome::Persisted(saved));
        }

        let review_id = review_repository
            .create_schema_review(
                listing_source_id,
                reason,
                &schemas,
                pages,
                validation_summary,
            )
            .await
            .map_err(review_error_to_schema_service_error)?;
        Ok(GeneratedSchemaReviewOutcome::PendingReview(review_id))
    }
}

fn with_schema_matrix_summary(
    mut validation_summary: Value,
    matrix: &SchemaMatrix,
    deterministic_approval_ok: bool,
) -> Value {
    let mut failed_fields = Vec::new();
    let mut optional_field_misses = Vec::new();
    for candidate in &matrix.candidates {
        for page in &candidate.pages {
            for field in &page.fields {
                if field.field == "images" && field.selector_match_count == Some(0) {
                    continue;
                }
                let selector_missing = selector_field_matched_nothing(field);
                let entry = json!({
                    "schema_index": candidate.schema_index,
                    "page_reference": page.page_reference,
                    "url": page.url,
                    "role": page.role,
                    "field": field.field,
                    "selector": field.selector,
                    "selector_match_count": field.selector_match_count,
                    "present_but_missing_rule": selector_missing,
                    "error": field.error,
                });
                if field.error.is_some() || (selector_missing && field.required) {
                    failed_fields.push(entry);
                } else if selector_missing && selector_field_is_soft(field) {
                    // An optional rule that matched nothing left the field empty; the
                    // page still applied, so this is reviewer information, not a failure.
                    optional_field_misses.push(entry);
                }
            }
        }
    }

    let matrix_summary = json!({
        "deterministic_approval_ok": deterministic_approval_ok,
        "schema_candidate_count": matrix.candidates.len(),
        "unused_schema_indices": unused_schema_indices(matrix),
        "present_but_missing_rule_failures": failed_fields,
        "optional_field_misses": optional_field_misses,
        // Per-page misses tell page variance apart from a broken selector only in
        // aggregate: a rule that missed on every applied page is listed here and
        // blocks deterministic approval.
        "optional_rules_missing_on_every_page": optional_rules_missing_on_every_page(matrix),
        // Every failed per-page or cross-page plausibility check blocks deterministic
        // approval, also on a page another candidate covers.
        "implausible_pages": implausible_pages(matrix),
    });

    if let Some(object) = validation_summary.as_object_mut() {
        object.insert("schema_matrix".to_string(), json!(matrix));
        object.insert("schema_matrix_summary".to_string(), matrix_summary);
        validation_summary
    } else {
        json!({
            "summary": validation_summary,
            "schema_matrix": matrix,
            "schema_matrix_summary": matrix_summary,
        })
    }
}

fn with_auto_schema_evaluation(
    mut validation_summary: Value,
    evaluation: &SchemaLlmEvaluation,
) -> Value {
    let evaluation = serde_json::to_value(evaluation)
        .unwrap_or_else(|err| json!({ "serialization_error": err.to_string() }));
    if let Some(object) = validation_summary.as_object_mut() {
        object.insert("auto_schema_evaluation".to_string(), evaluation);
        validation_summary
    } else {
        json!({
            "summary": validation_summary,
            "auto_schema_evaluation": evaluation,
        })
    }
}

fn should_auto_approve_generated_schema(
    authoring: CrawlerAuthoringMode,
    deterministic_approval_ok: bool,
    evaluation: &SchemaLlmEvaluation,
) -> bool {
    authoring == CrawlerAuthoringMode::Auto
        && deterministic_approval_ok
        && evaluation.is_high_confidence_approval()
}

pub(crate) fn review_error_to_schema_service_error(
    err: crate::review::repository::ReviewRepositoryError,
) -> ProductListingSchemaServiceError {
    use crate::review::repository::ReviewRepositoryError;
    match err {
        ReviewRepositoryError::Database(err) => {
            ProductListingSchemaServiceError::DatabaseError(err)
        }
        err @ ReviewRepositoryError::InvalidProductSchemaCandidate => {
            ProductListingSchemaServiceError::InvalidGeneratedSchema {
                source: application::error::box_error(err),
            }
        }
        err => ProductListingSchemaServiceError::ReviewPersistence {
            source: application::error::box_error(err),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::{
        PAGE_ROLE_PRIMARY, PAGE_ROLE_SEED, SchemaCandidateEvaluation, SchemaPageEvaluation,
        SchemaPageReference, SchemaReviewPageInput, SelectorFieldEvaluation,
    };
    use serde_json::json;

    #[test]
    fn should_record_present_but_missing_rule_failures_in_validation_summary() {
        let matrix = SchemaMatrix {
            review_id: None,
            candidates: vec![SchemaCandidateEvaluation {
                schema_index: 0,
                pages: vec![SchemaPageEvaluation {
                    page_reference: SchemaPageReference::Input { page_index: 0 },
                    url: "https://example.com/product".to_string(),
                    role: "PRIMARY".to_string(),
                    apply_ok: false,
                    extracted: None,
                    error: Some("failed to extract `title`".to_string()),
                    plausibility_findings: Default::default(),
                    cross_page_findings: Default::default(),
                    identity_agreement_findings: Default::default(),
                    fields: vec![SelectorFieldEvaluation {
                        field: "title".to_string(),
                        selector: "h1.product".to_string(),
                        selector_match_count: Some(0),
                        additional_selector_match_counts: Vec::new(),
                        error: None,
                        required: true,
                        skipped_element_count: None,
                    }],
                }],
            }],
        };

        let summary = with_schema_matrix_summary(json!({ "schema_count": 1 }), &matrix, false);

        assert_eq!(
            summary["schema_matrix_summary"]["deterministic_approval_ok"],
            false
        );
        assert_eq!(
            summary["schema_matrix_summary"]["present_but_missing_rule_failures"][0]["field"],
            "title"
        );
        assert_eq!(
            summary["schema_matrix_summary"]["present_but_missing_rule_failures"][0]["present_but_missing_rule"],
            true
        );
        assert_eq!(
            summary["schema_matrix_summary"]["optional_field_misses"],
            json!([])
        );
    }

    fn optional_miss_schema() -> ProductCssSelectorSchema {
        serde_json::from_value(json!({
            "source_listing_id": {"selector": "#sku", "type": "text"},
            "title": {"selector": "h1", "type": "text"},
            "description": {"selector": "div.titre + p", "type": "text", "cardinality": "all"},
            "price": {"selector": ".price", "type": "text"},
            "state": {"selector": "#state", "type": "text"},
            "images": {"selector": "img", "type": "image_url", "cardinality": "all"}
        }))
        .unwrap_or_else(|error| panic!("schema fixture should deserialize: {error}"))
    }

    /// The page of the product `path`, with its own identity, title and image.
    fn seed_page(role: &str, path: &str, description: Option<&str>) -> SchemaReviewPageInput {
        let description = description
            .map(|text| format!("<div class=\"titre\"></div><p>{text}</p>"))
            .unwrap_or_default();
        SchemaReviewPageInput {
            url: format!("https://example.com/{path}"),
            role: role.to_string(),
            raw_html: format!(
                "<html><body><span id=\"sku\">{path}</span><h1>Product {path}</h1>{description}<span class=\"price\">10 EUR</span><span id=\"state\">Sold</span><img src=\"{path}.jpg\"></body></html>"
            ),
        }
    }

    #[test]
    fn should_block_deterministic_approval_and_list_pages_that_fail_plausibility_checks() {
        let schema = ProductCssSelectorSchema {
            description: None,
            ..optional_miss_schema()
        };
        let mut sold_page = seed_page(PAGE_ROLE_SEED, "sold-item", None);
        sold_page.raw_html = sold_page
            .raw_html
            .replace("Product sold-item", "Sold items - Navarro")
            .replace("10 EUR", "0.00 EUR");
        let mut other_sold_page = seed_page(PAGE_ROLE_SEED, "other-sold-item", None);
        other_sold_page.raw_html = other_sold_page
            .raw_html
            .replace("Product other-sold-item", "Sold items - Navarro");
        let pages = vec![
            seed_page(PAGE_ROLE_PRIMARY, "chair", None),
            sold_page,
            other_sold_page,
        ];

        let matrix = evaluate_schema_matrix_for_inputs(std::slice::from_ref(&schema), &pages);
        let deterministic_approval_ok = schema_matrix_deterministic_approval_ok(&matrix);
        let summary = with_schema_matrix_summary(
            json!({ "schema_count": 1 }),
            &matrix,
            deterministic_approval_ok,
        );

        assert!(
            matrix.candidates[0].pages.iter().all(|page| page.apply_ok),
            "neither a zero price nor a repeated title fails the page at runtime"
        );
        assert!(!deterministic_approval_ok);
        assert_eq!(
            summary["schema_matrix_summary"]["implausible_pages"],
            json!([
                {
                    "schema_index": 0,
                    "page_reference": {"kind": "input", "page_index": 1},
                    "url": "https://example.com/sold-item",
                    "role": PAGE_ROLE_SEED,
                    "plausibility_findings": [{"field": "price", "check": "ALL_ZERO_DIGITS"}],
                    "cross_page_findings": [{
                        "field": "title",
                        "check": "REPEATED_ACROSS_PAGES",
                        "values": ["Sold items - Navarro"]
                    }],
                    "identity_agreement_findings": []
                },
                {
                    "schema_index": 0,
                    "page_reference": {"kind": "input", "page_index": 2},
                    "url": "https://example.com/other-sold-item",
                    "role": PAGE_ROLE_SEED,
                    "plausibility_findings": [],
                    "cross_page_findings": [{
                        "field": "title",
                        "check": "REPEATED_ACROSS_PAGES",
                        "values": ["Sold items - Navarro"]
                    }],
                    "identity_agreement_findings": []
                }
            ])
        );
    }

    #[test]
    fn should_report_optional_field_miss_on_some_pages_without_blocking_deterministic_approval() {
        let schema = optional_miss_schema();
        let pages = vec![
            seed_page(PAGE_ROLE_PRIMARY, "with-description", Some("Oak dresser")),
            seed_page(PAGE_ROLE_SEED, "sold-product", None),
            seed_page(PAGE_ROLE_SEED, "other-product", Some("Walnut chair")),
        ];

        let matrix = evaluate_schema_matrix_for_inputs(std::slice::from_ref(&schema), &pages);
        let deterministic_approval_ok = schema_matrix_deterministic_approval_ok(&matrix);
        let summary = with_schema_matrix_summary(
            json!({ "schema_count": 1 }),
            &matrix,
            deterministic_approval_ok,
        );

        assert!(deterministic_approval_ok);
        assert_eq!(
            summary["schema_matrix_summary"]["present_but_missing_rule_failures"],
            json!([])
        );
        let misses = &summary["schema_matrix_summary"]["optional_field_misses"];
        assert_eq!(misses.as_array().map(Vec::len), Some(1));
        assert_eq!(misses[0]["field"], "description");
        assert_eq!(misses[0]["selector"], "div.titre + p");
        assert_eq!(misses[0]["url"], "https://example.com/sold-product");
        assert_eq!(misses[0]["selector_match_count"], 0);
        assert_eq!(misses[0]["present_but_missing_rule"], true);
        assert_eq!(
            summary["schema_matrix_summary"]["optional_rules_missing_on_every_page"],
            json!([])
        );
    }

    #[test]
    fn should_block_deterministic_approval_when_optional_rule_misses_on_every_seed_page() {
        let schema = optional_miss_schema();
        let pages = vec![
            seed_page(PAGE_ROLE_PRIMARY, "first", None),
            seed_page(PAGE_ROLE_SEED, "second", None),
            seed_page(PAGE_ROLE_SEED, "third", None),
        ];

        let matrix = evaluate_schema_matrix_for_inputs(std::slice::from_ref(&schema), &pages);
        let deterministic_approval_ok = schema_matrix_deterministic_approval_ok(&matrix);
        let summary = with_schema_matrix_summary(
            json!({ "schema_count": 1 }),
            &matrix,
            deterministic_approval_ok,
        );

        assert!(!deterministic_approval_ok);
        assert_eq!(
            summary["schema_matrix_summary"]["deterministic_approval_ok"],
            false
        );
        assert_eq!(
            summary["schema_matrix_summary"]["present_but_missing_rule_failures"],
            json!([])
        );
        assert_eq!(
            summary["schema_matrix_summary"]["optional_field_misses"]
                .as_array()
                .map(Vec::len),
            Some(3)
        );
        assert_eq!(
            summary["schema_matrix_summary"]["optional_rules_missing_on_every_page"],
            json!([{
                "schema_index": 0,
                "field": "description",
                "selector": "div.titre + p",
                "page_count": 3
            }])
        );
    }

    #[test]
    fn should_not_report_miss_when_an_additional_selector_matches() {
        let matrix = SchemaMatrix {
            review_id: None,
            candidates: vec![SchemaCandidateEvaluation {
                schema_index: 0,
                pages: vec![SchemaPageEvaluation {
                    page_reference: SchemaPageReference::Input { page_index: 0 },
                    url: "https://example.com/product".to_string(),
                    role: "PRIMARY".to_string(),
                    apply_ok: true,
                    extracted: None,
                    error: None,
                    plausibility_findings: Default::default(),
                    cross_page_findings: Default::default(),
                    identity_agreement_findings: Default::default(),
                    fields: vec![SelectorFieldEvaluation {
                        field: "description".to_string(),
                        selector: "div.titre + p".to_string(),
                        selector_match_count: Some(0),
                        additional_selector_match_counts: vec![2],
                        error: None,
                        required: false,
                        skipped_element_count: None,
                    }],
                }],
            }],
        };

        let summary = with_schema_matrix_summary(json!({ "schema_count": 1 }), &matrix, true);

        assert_eq!(
            summary["schema_matrix_summary"]["present_but_missing_rule_failures"],
            json!([])
        );
        assert_eq!(
            summary["schema_matrix_summary"]["optional_field_misses"],
            json!([])
        );
    }

    #[test]
    fn should_not_record_missing_images_as_present_but_missing_rule_failure() {
        let matrix = SchemaMatrix {
            review_id: None,
            candidates: vec![SchemaCandidateEvaluation {
                schema_index: 0,
                pages: vec![SchemaPageEvaluation {
                    page_reference: SchemaPageReference::Input { page_index: 0 },
                    url: "https://example.com/product".to_string(),
                    role: "PRIMARY".to_string(),
                    apply_ok: true,
                    extracted: None,
                    error: None,
                    plausibility_findings: Default::default(),
                    cross_page_findings: Default::default(),
                    identity_agreement_findings: Default::default(),
                    fields: vec![SelectorFieldEvaluation {
                        field: "images".to_string(),
                        selector: "#gallery img".to_string(),
                        selector_match_count: Some(0),
                        additional_selector_match_counts: Vec::new(),
                        error: None,
                        required: false,
                        skipped_element_count: Some(0),
                    }],
                }],
            }],
        };

        let summary = with_schema_matrix_summary(json!({ "schema_count": 1 }), &matrix, true);

        assert_eq!(
            summary["schema_matrix_summary"]["present_but_missing_rule_failures"],
            json!([])
        );
    }

    #[test]
    fn should_map_invalid_review_candidate_to_schema_outcome_not_database_error() {
        let error = review_error_to_schema_service_error(
            crate::review::repository::ReviewRepositoryError::InvalidProductSchemaCandidate,
        );

        assert!(matches!(
            error,
            ProductListingSchemaServiceError::InvalidGeneratedSchema { .. }
        ));
    }

    #[test]
    fn should_keep_non_database_review_failure_cause_instead_of_database_error() {
        let error = review_error_to_schema_service_error(
            crate::review::repository::ReviewRepositoryError::InvalidSchemaField(
                "price".to_string(),
            ),
        );

        let ProductListingSchemaServiceError::ReviewPersistence { source } = &error else {
            panic!("expected review persistence error, got {error:?}");
        };
        assert_eq!(source.to_string(), "invalid schema field `price`");
    }

    #[tokio::test]
    async fn should_reject_invalid_generated_schema_before_persisting_it() {
        use crate::scraper::candidate_service::MockScraperCandidateService;
        use crate::scraper::css_selector::product_schema_service::MockProductListingSchemaService;
        use crate::scraper::scraper_service::html_fetcher::MockHtmlFetcher;
        use std::sync::Arc;

        let mut schema_service = MockProductListingSchemaService::new();
        schema_service.expect_save_product_schemas().never();
        let service = ScraperServiceImpl::new(
            Box::new(MockHtmlFetcher::new()),
            Box::new(schema_service),
            Arc::new(MockScraperCandidateService::new()),
        );
        let mut schema: ProductCssSelectorSchema = serde_json::from_value(json!({
            "title": {"selector": "h1", "type": "text"},
            "state": {"selector": ".stock", "type": "text"},
            "images": {"selector": "img", "type": "image_url", "cardinality": "all"}
        }))
        .unwrap_or_else(|error| panic!("schema fixture should deserialize: {error}"));
        schema.title.selector = "[".into();

        let result = service
            .handle_generated_schema_review(
                &ListingSourceId::new(),
                "initial_schema_generation",
                vec![schema],
                SchemaLlmEvaluation::unavailable("not evaluated"),
                Vec::new(),
                json!({}),
            )
            .await;

        let Err(ScraperError::SchemaServiceError(
            ProductListingSchemaServiceError::InvalidGeneratedSchema { source },
        )) = result
        else {
            panic!("expected invalid generated schema error");
        };
        assert!(source.to_string().contains("invalid CSS selector `[`"));
    }

    #[test]
    fn should_keep_review_database_failures_as_database_errors() {
        let error = review_error_to_schema_service_error(
            crate::review::repository::ReviewRepositoryError::Database(sqlx::Error::PoolTimedOut),
        );

        assert!(matches!(
            error,
            ProductListingSchemaServiceError::DatabaseError(sqlx::Error::PoolTimedOut)
        ));
    }

    #[test]
    fn should_auto_approve_only_high_confidence_generation_in_auto_mode() {
        let high = SchemaLlmEvaluation {
            decision: crate::scraper::css_selector::product_schema_service::SchemaLlmEvaluationDecision::Approve,
            confidence: crate::scraper::css_selector::product_schema_service::SchemaLlmEvaluationConfidence::High,
            approved_by_llm: false,
            summary: "good".to_string(),
            risks: Vec::new(),
        };
        let medium = SchemaLlmEvaluation {
            decision: crate::scraper::css_selector::product_schema_service::SchemaLlmEvaluationDecision::NeedsHumanReview,
            confidence: crate::scraper::css_selector::product_schema_service::SchemaLlmEvaluationConfidence::Medium,
            approved_by_llm: false,
            summary: "uncertain".to_string(),
            risks: Vec::new(),
        };

        assert!(should_auto_approve_generated_schema(
            CrawlerAuthoringMode::Auto,
            true,
            &high,
        ));
        assert!(!should_auto_approve_generated_schema(
            CrawlerAuthoringMode::Auto,
            true,
            &medium,
        ));
        assert!(!should_auto_approve_generated_schema(
            CrawlerAuthoringMode::Auto,
            false,
            &high,
        ));
        assert!(!should_auto_approve_generated_schema(
            CrawlerAuthoringMode::Review,
            true,
            &high,
        ));
    }
}
