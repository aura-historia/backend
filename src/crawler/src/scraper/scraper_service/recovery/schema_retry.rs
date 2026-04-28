use crate::scraper::css_selector::product_schema::{
    ApplySchemaError, ProductCssSelectorSchema, RawExtractedProduct,
};
use crate::scraper::css_selector::rule::ExtractionError;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::extraction::engine::try_apply_schemas;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use common::shop_id::ShopId;
use tracing::{info, warn};
use url::Url;

impl ScraperServiceImpl {
    /// Generates and appends new schema variants until one applies or attempts
    /// are exhausted.  On success returns the selected schema, the extracted
    /// raw product, **and the full persisted schema list** (existing + newly
    /// appended).  The caller must use this updated list as `existing_schemas`
    /// for any subsequent normalization-fix retry so that the persisted set
    /// stays consistent.
    pub(crate) async fn append_and_reapply_with_retry(
        &self,
        shop_id: &ShopId,
        domain: &str,
        url: &Url,
        html: &str,
        existing_schemas: &[ProductCssSelectorSchema],
    ) -> Result<
        (
            ProductCssSelectorSchema,
            RawExtractedProduct,
            Vec<ProductCssSelectorSchema>,
        ),
        ScraperError,
    > {
        let attempts = self.max_schema_fix_attempts.max(1);
        let mut last_error: Option<ApplySchemaError> = None;
        let mut last_generated_schema: Option<ProductCssSelectorSchema> = None;

        for attempt in 1..=attempts {
            self.consume_llm_budget_or_err(shop_id, url).await?;

            let generated_schema = self
                .schema_service
                .append_single_schema(
                    domain,
                    html,
                    last_generated_schema.as_ref(),
                    last_error.as_ref(),
                )
                .await?;

            match try_apply_schemas(std::iter::once(&generated_schema), html) {
                Ok((selected_schema, raw)) => {
                    let mut persisted_schemas = existing_schemas.to_vec();
                    persisted_schemas.push(generated_schema);
                    let saved = self
                        .schema_service
                        .save_product_schemas(shop_id, domain, persisted_schemas)
                        .await?;
                    info!(domain, url = %url, attempt, "Generated schema appended and applied");
                    return Ok((selected_schema, raw, saved.product_schemas));
                }
                Err(err) => {
                    last_generated_schema = Some(generated_schema);
                    warn!(
                        domain,
                        url = %url,
                        attempt,
                        max_attempts = attempts,
                        error = %err,
                        "Generated schema did not apply; discarding and retrying"
                    );
                    last_error = Some(err);
                }
            }
        }

        Err(ScraperError::SchemaRegenerationExhausted {
            url: url.clone(),
            attempts,
            last_error: last_error.unwrap_or_else(|| {
                ApplySchemaError::Title(ExtractionError::NoElementMatched {
                    selector: "title".to_string(),
                })
            }),
        })
    }
}
