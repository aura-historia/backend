use crate::review::repository::{SchemaReviewPageInput, PAGE_ROLE_TRIGGERING_REPAIR_PAGE};
use crate::scraper::css_selector::product_schema::{ApplySchemaError, ProductCssSelectorSchema};
use crate::scraper::css_selector::rule::ExtractionError;
use crate::scraper::normalization::error::NormalizationError;
use crate::scraper::normalization::product::NormalizedProduct;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::extraction::engine::try_apply_schemas;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use crate::scraper::scraper_service::util::html::normalization_error_to_schema_hint;
use common::shop_id::ShopId;
use serde_json::json;
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

// ---------------------------------------------------------------------------
// impl ScraperServiceImpl
// ---------------------------------------------------------------------------

impl ScraperServiceImpl {
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
    pub(crate) async fn normalize_with_schema_fix_retry(
        &self,
        ctx: NormalizationRetryContext<'_>,
        raw: crate::scraper::css_selector::product_schema::RawExtractedProduct,
    ) -> Result<NormalizedProduct, ScraperError> {
        match self
            .normalization_service
            .normalize(
                raw,
                ctx.url.clone(),
                ctx.selected_schema
                    .default_currency
                    .map(common::currency::domain::Currency::from),
            )
            .await
        {
            Ok((product, norm_llm_calls)) => {
                self.consume_llm_budget_n_or_err(ctx.shop_id, ctx.url, norm_llm_calls)
                    .await?;
                Ok(product)
            }
            Err(err) if normalization_error_to_schema_hint(&err).is_some() => {
                self.fix_normalization_with_schema_retry(ctx, err).await
            }
            Err(err) => Err(ScraperError::NormalizationError(err)),
        }
    }

    /// Attempt to repair a fixable normalization failure by generating and
    /// applying improved schema variants.
    ///
    /// Each iteration:
    /// 1. Charges one schema-generation LLM call against the budget.
    /// 2. Asks the schema service to append a single new schema (passing the
    ///    previously failed schema and the current error as repair context).
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
    pub(crate) async fn fix_normalization_with_schema_retry(
        &self,
        ctx: NormalizationRetryContext<'_>,
        first_norm_err: NormalizationError,
    ) -> Result<NormalizedProduct, ScraperError> {
        let attempts = self.max_schema_fix_attempts.max(1);

        // Hint for the schema generator derived from the last normalization error.
        let mut last_apply_error: Option<ApplySchemaError> =
            normalization_error_to_schema_hint(&first_norm_err);
        // Track the last normalization error so we can surface it at exhaustion
        // when the terminal failure was a norm error (schema applied fine but
        // normalization still failed).
        let mut last_norm_error: Option<NormalizationError> = Some(first_norm_err);
        let mut last_generated_schema: Option<ProductCssSelectorSchema> =
            Some(ctx.selected_schema.clone());

        for attempt in 1..=attempts {
            // Unwrap is safe: last_apply_error is Some on every loop entry —
            // it is set from the norm error on first entry and from the apply
            // error or norm error on subsequent iterations.
            let apply_hint = last_apply_error.take().unwrap();

            self.consume_llm_budget_or_err(ctx.shop_id, ctx.url).await?;

            let generated_schema = self
                .schema_service
                .append_single_schema(ctx.html, last_generated_schema.as_ref(), Some(&apply_hint))
                .await?;

            let reapplied = match try_apply_schemas(std::iter::once(&generated_schema), ctx.html) {
                Ok((_, raw)) => raw,
                Err(apply_err) => {
                    last_generated_schema = Some(generated_schema);
                    last_apply_error = Some(apply_err);
                    last_norm_error = None;
                    continue;
                }
            };

            match self
                .normalization_service
                .normalize(
                    reapplied,
                    ctx.url.clone(),
                    generated_schema
                        .default_currency
                        .map(common::currency::domain::Currency::from),
                )
                .await
            {
                Ok((product, norm_llm_calls)) => {
                    self.consume_llm_budget_n_or_err(ctx.shop_id, ctx.url, norm_llm_calls)
                        .await?;
                    let mut persisted_schemas = ctx.existing_schemas.to_vec();
                    persisted_schemas.push(generated_schema.clone());

                    if self.review_required
                        && let Some(review_repository) = &self.review_repository
                    {
                        let review_id = review_repository
                            .create_schema_review(
                                ctx.shop_id,
                                "normalization_schema_repair",
                                &persisted_schemas,
                                vec![SchemaReviewPageInput {
                                    url: ctx.url.to_string(),
                                    role: PAGE_ROLE_TRIGGERING_REPAIR_PAGE.to_string(),
                                    raw_html: ctx.html.to_string(),
                                }],
                                json!({
                                    "attempt": attempt,
                                    "schema_applied": true,
                                    "normalization_fixed": true,
                                }),
                            )
                            .await
                            .map_err(|err| {
                                crate::scraper::css_selector::product_schema_service::ProductSchemaServiceError::DatabaseError(
                                    sqlx::Error::Protocol(err.to_string()),
                                )
                            })?;
                        return Err(ScraperError::PendingSchemaReview {
                            url: ctx.url.clone(),
                            review_id,
                        });
                    }
                    self.schema_service
                        .save_product_schemas(ctx.shop_id, persisted_schemas)
                        .await?;
                    info!(
                        domain = ctx.domain,
                        url = %ctx.url,
                        attempt,
                        "Schema fixed normalization failure"
                    );
                    return Ok(product);
                }
                Err(norm_err) => {
                    let Some(hint) = normalization_error_to_schema_hint(&norm_err) else {
                        return Err(ScraperError::NormalizationError(norm_err));
                    };
                    last_generated_schema = Some(generated_schema);
                    last_apply_error = Some(hint);
                    last_norm_error = Some(norm_err);
                }
            }
        }

        // Determine the terminal failure mode.
        if let Some(norm_err) = last_norm_error {
            Err(ScraperError::NormalizationFixExhausted {
                url: ctx.url.clone(),
                attempts,
                last_norm_error: norm_err,
            })
        } else {
            Err(ScraperError::SchemaRegenerationExhausted {
                url: ctx.url.clone(),
                attempts,
                last_error: last_apply_error.unwrap_or_else(|| {
                    ApplySchemaError::Title(ExtractionError::NoElementMatched {
                        selector: "title".to_string(),
                    })
                }),
            })
        }
    }
}
