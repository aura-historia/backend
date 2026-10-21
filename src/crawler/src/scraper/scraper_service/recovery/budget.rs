use crate::scraper::css_selector::product_schema_service::ProductListingSchemaServiceError;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use listing_source_core::ListingSourceId;
use url::Url;

impl ScraperServiceImpl {
    /// Charge one LLM call against the per-ListingSource budget. When the budget
    /// would be exceeded the function returns [`ScraperError::LlmBudgetExceeded`]
    /// without modifying the counter.
    #[allow(clippy::result_large_err)]
    pub(crate) async fn consume_llm_budget_or_err(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
    ) -> Result<(), ScraperError> {
        let incremented = self
            .candidate_service
            .try_increment_listing_source_llm_calls_with_limit(
                listing_source_id,
                1,
                self.max_llm_calls_per_listing_source,
            )
            .await
            .map_err(|err| {
                ScraperError::SchemaServiceError(ProductListingSchemaServiceError::DatabaseError(
                    err,
                ))
            })?;

        if !incremented {
            return Err(ScraperError::LlmBudgetExceeded {
                listing_source_id: *listing_source_id,
                url: url.clone(),
                max_calls: self.max_llm_calls_per_listing_source,
            });
        }

        Ok(())
    }
}
