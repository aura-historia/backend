use crate::review::model::{PAGE_ROLE_TRIGGERING_REPAIR_PAGE, SchemaReviewPageInput};
use crate::scraper::css_selector::product_schema::{
    ApplySchemaError, ProductCssSelectorSchema, RawExtractedProduct,
};
use crate::scraper::css_selector::product_schema_service::GeneratedAppendSchema;
use crate::scraper::css_selector::rule::ExtractionError;
use crate::scraper::normalization::error::NormalizationError;
use crate::scraper::normalization::product::NormalizedProduct;
use crate::scraper::normalization::product_normalization_service::{
    NormalizationFailure, NormalizationSuccess,
};
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::extraction::engine::{apply_schema, try_apply_schemas};
use crate::scraper::scraper_service::extraction::schema_review_gate::GeneratedSchemaReviewOutcome;
use crate::scraper::scraper_service::image_validation::filter_valid_image_urls;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use crate::scraper::scraper_service::util::html::normalization_error_to_schema_hint;
use serde_json::json;
use shop_core::shop_id::ShopId;
use tracing::info;
use url::Url;

// ---------------------------------------------------------------------------
// NormalizationRetryContext
// ---------------------------------------------------------------------------

pub(crate) struct NormalizationRetryContext<'a> {
    pub(crate) shop_id: &'a ShopId,
    pub(crate) domain: &'a str,
    pub(crate) url: &'a Url,
    pub(crate) html: &'a str,
    pub(crate) existing_schemas: &'a [ProductCssSelectorSchema],
    pub(crate) selected_schema: ProductCssSelectorSchema,
}

pub(crate) enum ExistingSchemaSelection {
    Normalized(Box<NormalizedProduct>),
    NeedsRepair {
        selected_schema: Box<ProductCssSelectorSchema>,
        last_norm_error: NormalizationError,
    },
    NoSchemaApplied {
        last_error: ApplySchemaError,
    },
}

// ---------------------------------------------------------------------------
// impl ScraperServiceImpl
// ---------------------------------------------------------------------------

impl ScraperServiceImpl {
    #[tracing::instrument(
        skip(self, schemas, html),
        fields(
            shop_id = %shop_id,
            url = %url,
            schema_count = schemas.len()
        )
    )]
    #[allow(clippy::result_large_err)]
    pub(crate) async fn select_existing_schema_with_normalization(
        &self,
        shop_id: &ShopId,
        url: &Url,
        html: &str,
        schemas: &[ProductCssSelectorSchema],
    ) -> Result<ExistingSchemaSelection, ScraperError> {
        let mut last_apply_error: Option<ApplySchemaError> = None;
        let mut last_fixable_norm_failure: Option<(ProductCssSelectorSchema, NormalizationError)> =
            None;

        for schema in schemas {
            let raw = match apply_schema(schema, html) {
                Ok(raw) => raw,
                Err(err) => {
                    last_apply_error = Some(err);
                    continue;
                }
            };

            match self
                .normalize_applied_schema(shop_id, url, schema, raw)
                .await
            {
                Ok(product) => return Ok(ExistingSchemaSelection::Normalized(Box::new(product))),
                Err(ScraperError::NormalizationError(err))
                    if normalization_error_to_schema_hint(&err).is_some() =>
                {
                    last_fixable_norm_failure = Some((schema.clone(), err));
                }
                Err(ScraperError::NormalizationError(err)) => {
                    return Err(ScraperError::NormalizationError(err));
                }
                Err(err) => return Err(err),
            }
        }

        if let Some((selected_schema, last_norm_error)) = last_fixable_norm_failure {
            return Ok(ExistingSchemaSelection::NeedsRepair {
                selected_schema: Box::new(selected_schema),
                last_norm_error,
            });
        }

        Ok(ExistingSchemaSelection::NoSchemaApplied {
            last_error: last_apply_error.unwrap_or_else(|| {
                ApplySchemaError::Title(ExtractionError::NoElementMatched {
                    selector: "title".to_string(),
                })
            }),
        })
    }

    #[allow(clippy::result_large_err)]
    async fn normalize_applied_schema(
        &self,
        shop_id: &ShopId,
        url: &Url,
        selected_schema: &ProductCssSelectorSchema,
        mut raw: RawExtractedProduct,
    ) -> Result<NormalizedProduct, ScraperError> {
        raw.images = match filter_valid_image_urls(raw.images, url, &*self.image_validator).await {
            Ok(images) => images,
            Err(NormalizationError::NoValidImages { .. }) => Vec::new(),
            Err(err) => return Err(ScraperError::NormalizationError(err)),
        };

        match self
            .normalization_service
            .normalize(
                raw,
                url.clone(),
                selected_schema.default_currency.map(money::Currency::from),
            )
            .await
        {
            Ok(NormalizationSuccess {
                product,
                llm_calls_used,
            }) => {
                self.consume_llm_budget_n_or_err(shop_id, url, llm_calls_used)
                    .await?;
                Ok(product)
            }
            Err(NormalizationFailure {
                error,
                llm_calls_used,
            }) => {
                self.consume_llm_budget_n_or_err(shop_id, url, llm_calls_used)
                    .await?;
                Err(ScraperError::NormalizationError(error))
            }
        }
    }

    /// Thin dispatcher: run normalization once and branch on the result.
    ///
    /// - **Happy path** — charge normalization LLM calls and return the product.
    /// - **Fixable normalization error** — delegate to
    ///   [`Self::fix_normalization_with_schema_retry`] which will attempt to
    ///   generate a better schema and re-normalize.
    /// - **Non-fixable normalization error** — propagate immediately as
    ///   [`ScraperError::NormalizationError`].
    #[tracing::instrument(
        skip(self, ctx, raw),
        fields(
            shop_id = %ctx.shop_id,
            domain = ctx.domain,
            url = %ctx.url,
            schema_count = ctx.existing_schemas.len()
        )
    )]
    #[allow(clippy::result_large_err)]
    pub(crate) async fn normalize_with_schema_fix_retry(
        &self,
        ctx: NormalizationRetryContext<'_>,
        mut raw: crate::scraper::css_selector::product_schema::RawExtractedProduct,
    ) -> Result<NormalizedProduct, ScraperError> {
        raw.images =
            match filter_valid_image_urls(raw.images, ctx.url, &*self.image_validator).await {
                Ok(images) => images,
                Err(NormalizationError::NoValidImages { .. }) => Vec::new(),
                Err(err) if normalization_error_to_schema_hint(&err).is_some() => {
                    return self.fix_normalization_with_schema_retry(ctx, err).await;
                }
                Err(err) => return Err(ScraperError::NormalizationError(err)),
            };
        match self
            .normalization_service
            .normalize(
                raw,
                ctx.url.clone(),
                ctx.selected_schema
                    .default_currency
                    .map(money::Currency::from),
            )
            .await
        {
            Ok(NormalizationSuccess {
                product,
                llm_calls_used,
            }) => {
                self.consume_llm_budget_n_or_err(ctx.shop_id, ctx.url, llm_calls_used)
                    .await?;
                Ok(product)
            }
            Err(NormalizationFailure {
                error,
                llm_calls_used,
            }) if normalization_error_to_schema_hint(&error).is_some() => {
                self.consume_llm_budget_n_or_err(ctx.shop_id, ctx.url, llm_calls_used)
                    .await?;
                self.fix_normalization_with_schema_retry(ctx, error).await
            }
            Err(NormalizationFailure {
                error,
                llm_calls_used,
            }) => {
                self.consume_llm_budget_n_or_err(ctx.shop_id, ctx.url, llm_calls_used)
                    .await?;
                Err(ScraperError::NormalizationError(error))
            }
        }
    }

    /// Attempt to repair a fixable normalization failure by generating and
    /// applying improved schema variants.
    ///
    /// Each iteration:
    /// 1. Charges one schema-generation LLM call against the budget.
    /// 2. Asks the schema service to append a single new schema from the
    ///    current page HTML.
    /// 3. Tries to apply the generated schema to the HTML.
    ///    - Apply failure → loop with updated apply-error context.
    /// 4. Re-normalizes the re-extracted product.
    ///    - Success → charge normalization LLM calls, persist the new schema,
    ///      return the product.
    ///    - Fixable norm error → loop with updated norm-error context.
    ///    - Non-fixable norm error → propagate immediately.
    ///
    /// On exhaustion the terminal failure mode determines the error variant:
    /// - Last failure was an apply error → [`ScraperError::SchemaRegenerationExhausted`].
    /// - Last failure was a normalization error → [`ScraperError::NormalizationFixExhausted`].
    #[tracing::instrument(
        skip(self, ctx, first_norm_err),
        fields(
            shop_id = %ctx.shop_id,
            domain = ctx.domain,
            url = %ctx.url,
            schema_count = ctx.existing_schemas.len()
        )
    )]
    #[allow(clippy::result_large_err)]
    pub(crate) async fn fix_normalization_with_schema_retry(
        &self,
        ctx: NormalizationRetryContext<'_>,
        first_norm_err: NormalizationError,
    ) -> Result<NormalizedProduct, ScraperError> {
        if normalization_error_to_schema_hint(&first_norm_err).is_none() {
            return Err(ScraperError::NormalizationError(first_norm_err));
        };

        if let Some(review_id) = self.pending_product_schema_review_id(ctx.shop_id).await? {
            return Err(ScraperError::PendingSchemaReview {
                url: ctx.url.clone(),
                review_id,
            });
        }

        self.consume_llm_budget_or_err(ctx.shop_id, ctx.url).await?;

        let generated = self.schema_service.append_single_schema(ctx.html).await?;
        let (generated_schema, evaluation) = match generated {
            GeneratedAppendSchema::Product { schema, evaluation } => (*schema, evaluation),
            GeneratedAppendSchema::Removed { schema, .. } => {
                if !schema.matches(ctx.html) {
                    return Err(crate::scraper::scraper_service::recovery::schema_retry::page_classification_did_not_match(
                        ctx.url,
                        &schema.selector,
                    ));
                }
                self.save_removed_page_schema(ctx.shop_id, schema).await?;
                self.mark_product_removed_best_effort(ctx.shop_id, ctx.url)
                    .await;
                return Err(ScraperError::ProductRemoved {
                    url: ctx.url.clone(),
                    details: "normalization schema repair classified page as removed".to_string(),
                });
            }
            GeneratedAppendSchema::NotProduct { reason, .. } => {
                self.mark_url_other_best_effort(ctx.shop_id, ctx.url).await;
                return Err(ScraperError::NotProductPage {
                    url: ctx.url.clone(),
                    details: reason,
                });
            }
        };

        let mut reapplied = match try_apply_schemas(std::iter::once(&generated_schema), ctx.html) {
            Ok((_, raw)) => raw,
            Err(apply_err) => {
                return Err(ScraperError::SchemaRegenerationExhausted {
                    url: ctx.url.clone(),
                    attempts: 1,
                    last_error: Box::new(apply_err),
                });
            }
        };
        reapplied.images = match filter_valid_image_urls(
            reapplied.images,
            ctx.url,
            &*self.image_validator,
        )
        .await
        {
            Ok(images) => images,
            Err(NormalizationError::NoValidImages { .. }) => Vec::new(),
            Err(norm_err) if normalization_error_to_schema_hint(&norm_err).is_some() => {
                return Err(ScraperError::NormalizationFixExhausted {
                    url: ctx.url.clone(),
                    attempts: 1,
                    last_norm_error: Box::new(norm_err),
                });
            }
            Err(norm_err) => return Err(ScraperError::NormalizationError(norm_err)),
        };

        match self
            .normalization_service
            .normalize(
                reapplied,
                ctx.url.clone(),
                generated_schema.default_currency.map(money::Currency::from),
            )
            .await
        {
            Ok(NormalizationSuccess {
                product,
                llm_calls_used,
            }) => {
                self.consume_llm_budget_n_or_err(ctx.shop_id, ctx.url, llm_calls_used)
                    .await?;
                let mut persisted_schemas = ctx.existing_schemas.to_vec();
                persisted_schemas.push(generated_schema.clone());

                let pages = vec![SchemaReviewPageInput {
                    url: ctx.url.to_string(),
                    role: PAGE_ROLE_TRIGGERING_REPAIR_PAGE.to_string(),
                    raw_html: ctx.html.to_string(),
                }];
                match self
                    .handle_generated_schema_review(
                        ctx.shop_id,
                        "normalization_schema_repair",
                        persisted_schemas,
                        evaluation,
                        pages,
                        json!({
                            "schema_applied": true,
                            "normalization_fixed": true,
                        }),
                    )
                    .await?
                {
                    GeneratedSchemaReviewOutcome::Persisted(_) => {
                        info!(
                            domain = ctx.domain,
                            url = %ctx.url,
                            "Schema fixed normalization failure"
                        );
                        Ok(product)
                    }
                    GeneratedSchemaReviewOutcome::PendingReview(review_id) => {
                        Err(ScraperError::PendingSchemaReview {
                            url: ctx.url.clone(),
                            review_id,
                        })
                    }
                }
            }
            Err(NormalizationFailure {
                error: norm_err,
                llm_calls_used,
            }) => {
                self.consume_llm_budget_n_or_err(ctx.shop_id, ctx.url, llm_calls_used)
                    .await?;
                if normalization_error_to_schema_hint(&norm_err).is_some() {
                    return Err(ScraperError::NormalizationFixExhausted {
                        url: ctx.url.clone(),
                        attempts: 1,
                        last_norm_error: Box::new(norm_err),
                    });
                }
                Err(ScraperError::NormalizationError(norm_err))
            }
        }
    }
}
