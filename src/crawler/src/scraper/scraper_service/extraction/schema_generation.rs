use crate::review::model::{PAGE_ROLE_PRIMARY, PAGE_ROLE_SEED, SchemaReviewPageInput};
use crate::scraper::css_selector::product_schema::ShopsProductSchema;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::extraction::schema_review_gate::GeneratedSchemaReviewOutcome;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use common::shop_id::ShopId;
use serde_json::json;
use tracing::debug;
use url::Url;

impl ScraperServiceImpl {
    /// Obtains product CSS selector schemas for `shop_id`, loading them from
    /// the DB or generating them via the LLM if they do not yet exist.
    ///
    /// The dispatcher gates concurrent scraper work per shop inside one process,
    /// while database uniqueness prevents duplicate pending reviews across
    /// processes.
    #[tracing::instrument(skip(self, html), fields(shop_id = %shop_id, url = %url))]
    pub(crate) async fn obtain_schemas(
        &self,
        shop_id: &ShopId,
        url: &Url,
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

            let seed_pages = self.collect_schema_seed_pages(shop_id, url, html).await;
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
            self.consume_llm_budget_or_err(shop_id, url).await?;
            let schemas = self
                .schema_service
                .create_product_schemas(&seed_html_pages)
                .await?;
            let schema_count = schemas.len();
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
                .collect();
            match self
                .handle_generated_schema_review(
                    shop_id,
                    url,
                    "initial_schema_generation",
                    schemas,
                    pages,
                    json!({ "seed_page_count": seed_pages.len(), "schema_count": schema_count }),
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
