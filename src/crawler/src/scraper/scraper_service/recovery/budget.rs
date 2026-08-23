use crate::scraper::css_selector::product_schema_service::ProductSchemaServiceError;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use shop_core::shop_id::ShopId;
use url::Url;

impl ScraperServiceImpl {
    #[allow(clippy::result_large_err)]
    pub(crate) async fn consume_llm_budget_or_err(
        &self,
        shop_id: &ShopId,
        url: &Url,
    ) -> Result<(), ScraperError> {
        self.consume_llm_budget_n_or_err(shop_id, url, 1).await
    }

    /// Charge `n` LLM calls against the per-shop budget.  When `n` is zero
    /// this is a no-op.  When the budget would be exceeded the function returns
    /// [`ScraperError::LlmBudgetExceeded`] without modifying the counter.
    #[allow(clippy::result_large_err)]
    pub(crate) async fn consume_llm_budget_n_or_err(
        &self,
        shop_id: &ShopId,
        url: &Url,
        n: u32,
    ) -> Result<(), ScraperError> {
        if n == 0 {
            return Ok(());
        }
        let incremented = self
            .candidate_service
            .try_increment_shop_llm_calls_with_limit(
                shop_id,
                i64::from(n),
                self.max_llm_calls_per_shop,
            )
            .await
            .map_err(|err| {
                ScraperError::SchemaServiceError(ProductSchemaServiceError::DatabaseError(err))
            })?;

        if !incremented {
            return Err(ScraperError::LlmBudgetExceeded {
                shop_id: *shop_id,
                url: url.clone(),
                max_calls: self.max_llm_calls_per_shop,
            });
        }

        Ok(())
    }
}
