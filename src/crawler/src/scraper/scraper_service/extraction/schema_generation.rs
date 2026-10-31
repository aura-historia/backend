use crate::CrawlerDomainId;
use crate::review::model::{PAGE_ROLE_PRIMARY, PAGE_ROLE_SEED, SchemaReviewPageInput};
use crate::review::schema_evaluation::{
    evaluate_schema_matrix_for_inputs, schema_matrix_deterministic_approval_ok,
    unused_schema_indices,
};
use crate::scraper::css_selector::product_schema::ListingSourceProductSchema;
use crate::scraper::css_selector::product_schema_service::GeneratedProductSchemas;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::domain::product::ScrapeMode;
use crate::scraper::scraper_service::extraction::schema_review_gate::GeneratedSchemaReviewOutcome;
use crate::scraper::scraper_service::pipeline::scrape_product::FetchedPage;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use serde_json::json;
use tracing::debug;

struct SchemaGenerationAttempt {
    generated: GeneratedProductSchemas,
    deterministic_approval_ok: bool,
    unused_schema_indices: Vec<usize>,
}

impl SchemaGenerationAttempt {
    fn new(generated: GeneratedProductSchemas, pages: &[SchemaReviewPageInput]) -> Self {
        let matrix = evaluate_schema_matrix_for_inputs(&generated.schemas, pages);
        let deterministic_approval_ok = schema_matrix_deterministic_approval_ok(&matrix);
        let unused_schema_indices = unused_schema_indices(&matrix);
        Self {
            generated,
            deterministic_approval_ok,
            unused_schema_indices,
        }
    }
}

impl ScraperServiceImpl {
    /// Generates product CSS selector schemas for `listing_source_id` via the LLM, or opens a
    /// manual schema review over the seed pages when an operator authors schemas.
    /// The caller has already read the schema cache and found it empty.
    ///
    /// The dispatcher gates concurrent scraper work per ListingSource inside one process,
    /// while database uniqueness prevents duplicate pending reviews across
    /// processes.
    #[tracing::instrument(skip(self, page), fields(listing_source_id = %page.listing_source_id, url = %page.url))]
    #[allow(clippy::result_large_err)]
    pub(crate) async fn obtain_schemas(
        &self,
        page: FetchedPage<'_>,
        product_url_pattern: Option<&str>,
        domain_id: Option<&CrawlerDomainId>,
        mode: ScrapeMode,
    ) -> Result<ListingSourceProductSchema, ScraperError> {
        let FetchedPage {
            listing_source_id,
            requested_url,
            url,
            html,
            ..
        } = page;
        debug!("No cached product CSS selector schema; generating");
        if let Some(review_id) = self
            .pending_product_schema_review_id(listing_source_id)
            .await?
        {
            return Err(ScraperError::PendingSchemaReview {
                url: url.clone(),
                review_id,
            });
        }

        let seed_pages = self
            .collect_schema_seed_pages(
                listing_source_id,
                domain_id,
                url,
                product_url_pattern,
                html,
                mode,
            )
            .await?;
        if self.review_repository.is_some() {
            if let Some(existing) = self
                .schema_service
                .find_product_schema(listing_source_id)
                .await?
            {
                debug!("Schema found in DB after seed page collection");
                return Ok(existing);
            }
            if let Some(review_id) = self
                .pending_product_schema_review_id(listing_source_id)
                .await?
            {
                return Err(ScraperError::PendingSchemaReview {
                    url: url.clone(),
                    review_id,
                });
            }
        }

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
        if let Some(review_repository) = self.manual_schema_review_repository() {
            self.skip_rejected_page(listing_source_id, requested_url, url, None)
                .await?;
            return Err(self
                .open_manual_schema_review(review_repository, listing_source_id, url, &[], pages)
                .await);
        }

        let seed_html_pages: Vec<String> = seed_pages
            .iter()
            .map(|page| page.raw_html.clone())
            .collect();
        self.consume_llm_budget_or_err(listing_source_id, url)
            .await?;
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
                listing_source_id,
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
