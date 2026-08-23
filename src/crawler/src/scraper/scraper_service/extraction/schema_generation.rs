use crate::review::model::{PAGE_ROLE_PRIMARY, PAGE_ROLE_SEED, SchemaReviewPageInput};
use crate::review::schema_evaluation::{
    evaluate_schema_matrix_for_inputs, schema_matrix_has_required_coverage, unused_schema_indices,
};
use crate::scraper::css_selector::product_schema::ShopsProductSchema;
use crate::scraper::css_selector::product_schema_service::GeneratedProductSchemas;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::extraction::schema_review_gate::GeneratedSchemaReviewOutcome;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use serde_json::json;
use shop_core::shop_id::ShopId;
use tracing::debug;
use url::Url;

struct SchemaGenerationAttempt {
    generated: GeneratedProductSchemas,
    deterministic_approval_ok: bool,
    unused_schema_indices: Vec<usize>,
}

impl SchemaGenerationAttempt {
    fn new(generated: GeneratedProductSchemas, pages: &[SchemaReviewPageInput]) -> Self {
        let matrix = evaluate_schema_matrix_for_inputs(&generated.schemas, pages);
        let deterministic_approval_ok = schema_matrix_has_required_coverage(&matrix);
        let unused_schema_indices = unused_schema_indices(&matrix);
        Self {
            generated,
            deterministic_approval_ok,
            unused_schema_indices,
        }
    }
}

impl ScraperServiceImpl {
    /// Obtains product CSS selector schemas for `shop_id`, loading them from
    /// the DB or generating them via the LLM if they do not yet exist.
    ///
    /// The dispatcher gates concurrent scraper work per shop inside one process,
    /// while database uniqueness prevents duplicate pending reviews across
    /// processes.
    #[tracing::instrument(skip(self, html), fields(shop_id = %shop_id, url = %url))]
    #[allow(clippy::result_large_err)]
    pub(crate) async fn obtain_schemas(
        &self,
        shop_id: &ShopId,
        url: &Url,
        product_url_pattern: Option<&str>,
        html: &str,
    ) -> Result<ShopsProductSchema, ScraperError> {
        debug!("Obtaining product CSS selector schemas");
        if let Some(existing) = self.schema_service.find_product_schema(shop_id).await? {
            debug!("Schema found in DB");
            Ok(existing)
        } else {
            if let Some(review_id) = self.pending_product_schema_review_id(shop_id).await? {
                return Err(ScraperError::PendingSchemaReview {
                    url: url.clone(),
                    review_id,
                });
            }

            let seed_pages = self
                .collect_schema_seed_pages(shop_id, url, product_url_pattern, html)
                .await;
            if self.review_repository.is_some() {
                if let Some(existing) = self.schema_service.find_product_schema(shop_id).await? {
                    debug!("Schema found in DB after seed page collection");
                    return Ok(existing);
                }
                if let Some(review_id) = self.pending_product_schema_review_id(shop_id).await? {
                    return Err(ScraperError::PendingSchemaReview {
                        url: url.clone(),
                        review_id,
                    });
                }
            }

            let seed_html_pages: Vec<String> = seed_pages
                .iter()
                .map(|page| page.raw_html.clone())
                .collect();
            let pages = seed_pages
                .iter()
                .enumerate()
                .map(|(idx, page)| SchemaReviewPageInput {
                    url: page.url.to_string(),
                    role: if idx == 0 {
                        PAGE_ROLE_PRIMARY.to_string()
                    } else {
                        PAGE_ROLE_SEED.to_string()
                    },
                    raw_html: page.raw_html.clone(),
                })
                .collect::<Vec<_>>();

            self.consume_llm_budget_or_err(shop_id, url).await?;
            let yaml_generated = self
                .schema_service
                .create_product_schemas(&seed_html_pages)
                .await?;
            let yaml_attempt = SchemaGenerationAttempt::new(yaml_generated, &pages);
            let schema_count = yaml_attempt.generated.schemas.len();
            let validation_summary = json!({
                "seed_page_count": seed_pages.len(),
                "schema_count": schema_count,
                "confidence": yaml_attempt.generated.evaluation.confidence,
                "deterministic_approval_ok": yaml_attempt.deterministic_approval_ok,
                "unused_schema_indices": yaml_attempt.unused_schema_indices,
            });
            let generated = yaml_attempt.generated;

            match self
                .handle_generated_schema_review(
                    shop_id,
                    "initial_schema_generation",
                    generated.schemas,
                    generated.evaluation,
                    pages,
                    validation_summary,
                )
                .await?
            {
                GeneratedSchemaReviewOutcome::Persisted(saved) => Ok(saved),
                GeneratedSchemaReviewOutcome::PendingReview(review_id) => {
                    Err(ScraperError::PendingSchemaReview {
                        url: url.clone(),
                        review_id,
                    })
                }
            }
        }
    }
}
