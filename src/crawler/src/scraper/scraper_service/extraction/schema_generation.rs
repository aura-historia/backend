use crate::scraper::css_selector::product_schema::ShopsProductSchema;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use common::shop_id::ShopId;
use tracing::debug;
use url::Url;

impl ScraperServiceImpl {
    /// Obtains product CSS selector schemas for `shop_id`, loading them from
    /// the DB or generating them via the LLM if they do not yet exist.
    ///
    /// The dispatcher guarantees at most one in-flight scrape per domain at a
    /// time, so no additional locking is required here.
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
            let seed_pages = self.collect_schema_seed_pages(shop_id, url, html).await;
            self.consume_llm_budget_or_err(shop_id, url).await?;
            let schemas = self
                .schema_service
                .create_product_schemas(&seed_pages)
                .await?;
            Ok(self
                .schema_service
                .save_product_schemas(shop_id, schemas)
                .await?)
        }
    }
}
