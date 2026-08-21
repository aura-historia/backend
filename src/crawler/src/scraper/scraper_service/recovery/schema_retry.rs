use crate::review::model::{PAGE_ROLE_TRIGGERING_REPAIR_PAGE, SchemaReviewPageInput};
use crate::scraper::css_selector::product_schema::{ProductCssSelectorSchema, RawExtractedProduct};
use crate::scraper::css_selector::product_schema_service::GeneratedAppendSchema;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crate::scraper::css_selector::removed_page_schema::ShopsRemovedPageSchema;
use crate::scraper::css_selector::rule::ExtractionError;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::extraction::engine::try_apply_schemas;
use crate::scraper::scraper_service::extraction::schema_review_gate::GeneratedSchemaReviewOutcome;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use common::shop_id::ShopId;
use serde_json::json;
use time::OffsetDateTime;
use tracing::{info, warn};
use url::Url;

impl ScraperServiceImpl {
    /// Generates and appends new schema variants until one applies or attempts
    /// are exhausted.  On success returns the selected schema, the extracted
    /// raw product, **and the full persisted schema list** (existing + newly
    /// appended).  The caller must use this updated list as `existing_schemas`
    /// for any subsequent normalization-fix retry so that the persisted set
    /// stays consistent.
    #[tracing::instrument(
        skip(self, html, existing_schemas),
        fields(
            shop_id = %shop_id,
            url = %url,
            schema_count = existing_schemas.len()
        )
    )]
    #[allow(clippy::result_large_err)]
    pub(crate) async fn append_and_reapply_with_retry(
        &self,
        shop_id: &ShopId,
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
        if let Some(review_id) = self.pending_product_schema_review_id(shop_id).await? {
            return Err(ScraperError::PendingSchemaReview {
                url: url.clone(),
                review_id,
            });
        }

        self.consume_llm_budget_or_err(shop_id, url).await?;

        let generated = self.schema_service.append_single_schema(html).await?;
        let GeneratedAppendSchema::Product {
            schema: generated_schema,
            evaluation,
        } = generated
        else {
            return self
                .handle_generated_page_classification(shop_id, url, html, generated)
                .await;
        };
        let generated_schema = *generated_schema;

        let (selected_schema, raw) =
            match try_apply_schemas(std::iter::once(&generated_schema), html) {
                Ok(applied) => applied,
                Err(err) => {
                    warn!(error = ?err, "Generated schema did not apply; discarding");
                    return Err(ScraperError::SchemaRegenerationExhausted {
                        url: url.clone(),
                        attempts: 1,
                        last_error: Box::new(err),
                    });
                }
            };

        let mut persisted_schemas = existing_schemas.to_vec();
        persisted_schemas.push(generated_schema);

        let pages = vec![SchemaReviewPageInput {
            url: url.to_string(),
            role: PAGE_ROLE_TRIGGERING_REPAIR_PAGE.to_string(),
            raw_html: html.to_string(),
        }];
        match self
            .handle_generated_schema_review(
                shop_id,
                "append_schema_generation",
                persisted_schemas,
                evaluation,
                pages,
                json!({
                    "schema_applied": true,
                }),
            )
            .await?
        {
            GeneratedSchemaReviewOutcome::Persisted(saved) => {
                info!("Generated schema appended and applied");
                Ok((selected_schema, raw, saved.product_schemas))
            }
            GeneratedSchemaReviewOutcome::PendingReview(review_id) => {
                Err(ScraperError::PendingSchemaReview {
                    url: url.clone(),
                    review_id,
                })
            }
        }
    }

    #[allow(clippy::result_large_err)]
    async fn handle_generated_page_classification(
        &self,
        shop_id: &ShopId,
        url: &Url,
        html: &str,
        generated: GeneratedAppendSchema,
    ) -> Result<
        (
            ProductCssSelectorSchema,
            RawExtractedProduct,
            Vec<ProductCssSelectorSchema>,
        ),
        ScraperError,
    > {
        match generated {
            GeneratedAppendSchema::Removed { schema, .. } => {
                if !schema.matches(html) {
                    return Err(page_classification_did_not_match(url, &schema.selector));
                }
                self.save_removed_page_schema(shop_id, schema.clone())
                    .await?;
                self.mark_product_removed_best_effort(shop_id, url).await;
                Err(ScraperError::ProductRemoved {
                    url: url.clone(),
                    details: "append generation classified page as removed".to_string(),
                })
            }
            GeneratedAppendSchema::NotProduct { reason, .. } => {
                self.mark_url_other_best_effort(shop_id, url).await;
                Err(ScraperError::NotProductPage {
                    url: url.clone(),
                    details: reason,
                })
            }
            GeneratedAppendSchema::Product { .. } => unreachable!("product handled by caller"),
        }
    }

    #[allow(clippy::result_large_err)]
    pub(crate) async fn save_removed_page_schema(
        &self,
        shop_id: &ShopId,
        schema: RemovedPageSchema,
    ) -> Result<(), ScraperError> {
        let existing = self
            .removed_page_schema_repository
            .find_removed_page_schema(shop_id)
            .await
            .map_err(ScraperError::RemovedPageSchemaDatabaseError)?;

        match existing {
            Some(existing) => {
                let mut schemas = existing.removed_page_schemas;
                if !schemas.contains(&schema) {
                    schemas.push(schema);
                }
                self.removed_page_schema_repository
                    .update_removed_page_schema(shop_id, &schemas)
                    .await
                    .map_err(ScraperError::RemovedPageSchemaDatabaseError)?;
            }
            None => {
                let now = OffsetDateTime::now_utc();
                let row = ShopsRemovedPageSchema {
                    shop_id: *shop_id,
                    removed_page_schemas: vec![schema],
                    created: now,
                    updated: now,
                };
                self.removed_page_schema_repository
                    .insert_removed_page_schema(shop_id, &row)
                    .await
                    .map_err(ScraperError::RemovedPageSchemaDatabaseError)?;
            }
        }

        Ok(())
    }
}

pub(crate) fn page_classification_did_not_match(
    url: &Url,
    selector: &crate::scraper::css_selector::rule::CssSelector,
) -> ScraperError {
    ScraperError::SchemaRegenerationExhausted {
        url: url.clone(),
        attempts: 1,
        last_error: Box::new(
            crate::scraper::css_selector::product_schema::ApplySchemaError::Title(
                ExtractionError::NoElementMatched {
                    selector: selector.to_string(),
                },
            ),
        ),
    }
}
