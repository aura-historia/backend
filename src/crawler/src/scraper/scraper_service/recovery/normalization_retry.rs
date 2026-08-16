use crate::review::model::{PAGE_ROLE_TRIGGERING_REPAIR_PAGE, SchemaReviewPageInput};
use crate::scraper::css_selector::product_schema::{ProductCssSelectorSchema, RawExtractedProduct};
use crate::scraper::normalization::error::NormalizationError;
use crate::scraper::normalization::product::NormalizedProduct;
use crate::scraper::normalization::product_normalization_service::{
    NormalizationFailure, NormalizationSuccess,
};
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::extraction::schema_review_gate::GeneratedSchemaReviewOutcome;
use crate::scraper::scraper_service::extraction::schema_selection::{
    collect_applicable_candidates, rank_candidates,
};
use crate::scraper::scraper_service::image_validation::filter_valid_image_urls;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use crate::scraper::scraper_service::util::html::normalization_error_to_schema_hint;
use common::shop_id::ShopId;
use serde_json::json;
use tracing::{debug, info};
use url::Url;

// ---------------------------------------------------------------------------
// ExistingSchemaSelection
// ---------------------------------------------------------------------------

/// Why fresh schema generation was requested after cached selection failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FreshSchemaGenerationReason {
    /// No cached schema could be applied to the current page.
    NoCachedSchemaApplied,
    /// One or more cached schemas applied, but none produced a valid
    /// normalized product.
    NoCachedSchemaNormalized,
}

impl FreshSchemaGenerationReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NoCachedSchemaApplied => "no_cached_schema_applied",
            Self::NoCachedSchemaNormalized => "no_cached_schema_normalized",
        }
    }
}

pub(crate) enum ExistingSchemaSelection {
    Normalized(Box<NormalizedProduct>),
    /// Cached selection cannot produce a valid product — generate a
    /// completely new schema for the current page. Never carries a cached
    /// schema as a repair source.
    GenerateNewSchema {
        reason: FreshSchemaGenerationReason,
    },
}

// ---------------------------------------------------------------------------
// FreshSchemaGenerationContext
// ---------------------------------------------------------------------------

pub(crate) struct FreshSchemaGenerationContext<'a> {
    pub(crate) shop_id: &'a ShopId,
    pub(crate) domain: &'a str,
    pub(crate) url: &'a Url,
    pub(crate) html: &'a str,
    pub(crate) existing_schemas: &'a [ProductCssSelectorSchema],
}

// ---------------------------------------------------------------------------
// impl ScraperServiceImpl
// ---------------------------------------------------------------------------

impl ScraperServiceImpl {
    /// Applies every cached schema to the current page, ranks successful
    /// extractions by attribute completeness, and normalizes candidates from
    /// richest to least rich.
    ///
    /// The first candidate that normalizes successfully wins. When no cached
    /// candidate succeeds — either because none applied or none normalized —
    /// returns [`ExistingSchemaSelection::GenerateNewSchema`] so the caller
    /// falls back to fresh schema generation. No cached schema is ever
    /// selected as a repair source.
    #[tracing::instrument(
        skip(self, schemas, html),
        fields(
            shop_id = %shop_id,
            url = %url,
            schema_count = schemas.len()
        )
    )]
    pub(crate) async fn select_existing_schema_with_normalization(
        &self,
        shop_id: &ShopId,
        url: &Url,
        html: &str,
        schemas: &[ProductCssSelectorSchema],
    ) -> Result<ExistingSchemaSelection, ScraperError> {
        // `scraper::Html` is `!Send`: parse, apply every cached schema, score,
        // and rank inside one synchronous block so the parsed document is
        // dropped before any `.await`.
        let mut candidates = {
            let parsed = scraper::Html::parse_document(html);
            let mut candidate_set = collect_applicable_candidates(schemas, &parsed);

            debug!(
                schema_candidates_total = schemas.len(),
                schema_candidates_applied = candidate_set.candidates.len(),
                schema_candidates_apply_failed = candidate_set.apply_failures.len(),
                "Cached schema application complete"
            );
            for (schema_index, err) in &candidate_set.apply_failures {
                debug!(
                    candidate_schema_index = schema_index,
                    candidate_apply_error = ?err,
                    "Cached schema failed to apply"
                );
            }

            rank_candidates(&mut candidate_set.candidates);
            for candidate in &candidate_set.candidates {
                debug!(
                    candidate_schema_index = candidate.schema_index,
                    candidate_schema_score = candidate.score.as_usize(),
                    "Ranked cached schema candidate"
                );
            }

            candidate_set.candidates
        };

        if candidates.is_empty() {
            debug!(
                fresh_schema_generation_reason =
                    FreshSchemaGenerationReason::NoCachedSchemaApplied.as_str(),
                "No cached schema applied; fresh schema generation required"
            );
            return Ok(ExistingSchemaSelection::GenerateNewSchema {
                reason: FreshSchemaGenerationReason::NoCachedSchemaApplied,
            });
        }

        for candidate in candidates.drain(..) {
            match self
                .normalize_applied_schema(shop_id, url, candidate.schema, candidate.raw)
                .await
            {
                Ok(product) => {
                    debug!(
                        selected_schema_index = candidate.schema_index,
                        selected_schema_score = candidate.score.as_usize(),
                        candidate_normalization_result = "success",
                        "Cached schema selected"
                    );
                    return Ok(ExistingSchemaSelection::Normalized(Box::new(product)));
                }
                Err(ScraperError::NormalizationError(err))
                    if normalization_error_to_schema_hint(&err).is_some() =>
                {
                    debug!(
                        candidate_schema_index = candidate.schema_index,
                        candidate_schema_score = candidate.score.as_usize(),
                        candidate_normalization_result = "fixable_failure",
                        "Cached candidate normalization failed; trying next candidate"
                    );
                }
                Err(err) => return Err(err),
            }
        }

        debug!(
            fresh_schema_generation_reason =
                FreshSchemaGenerationReason::NoCachedSchemaNormalized.as_str(),
            "No cached schema normalized; fresh schema generation required"
        );
        Ok(ExistingSchemaSelection::GenerateNewSchema {
            reason: FreshSchemaGenerationReason::NoCachedSchemaNormalized,
        })
    }

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
                selected_schema
                    .default_currency
                    .map(common::currency::domain::Currency::from),
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

    /// Generates a completely new schema from the current page HTML, applies
    /// it, and normalizes the result.
    ///
    /// Every call is an independent fresh-generation attempt:
    ///
    /// * no cached schema is passed as repair input;
    /// * no localized, field-level, or selector-patching repair is performed;
    /// * a failed generated candidate is discarded, never mutated;
    /// * a successfully generated schema is reviewed and appended/persisted
    ///   through the existing schema-generation flow.
    ///
    /// On exhaustion the terminal failure mode determines the error variant:
    /// - Generated schema failed to apply → [`ScraperError::SchemaRegenerationExhausted`].
    /// - Generated schema applied but normalization kept failing →
    ///   [`ScraperError::NormalizationFixExhausted`].
    #[tracing::instrument(
        skip(self, ctx),
        fields(
            shop_id = %ctx.shop_id,
            domain = ctx.domain,
            url = %ctx.url,
            schema_count = ctx.existing_schemas.len()
        )
    )]
    pub(crate) async fn generate_fresh_schema_for_page(
        &self,
        ctx: FreshSchemaGenerationContext<'_>,
    ) -> Result<NormalizedProduct, ScraperError> {
        let (generated_schema, mut reapplied, evaluation) = self
            .generate_and_apply_fresh_schema(ctx.shop_id, ctx.url, ctx.html)
            .await?;

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
                    last_norm_error: norm_err,
                });
            }
            Err(norm_err) => return Err(ScraperError::NormalizationError(norm_err)),
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
                        "fresh_schema_generation",
                        persisted_schemas,
                        evaluation,
                        pages,
                        json!({
                            "schema_applied": true,
                            "fresh_generation": true,
                        }),
                    )
                    .await?
                {
                    GeneratedSchemaReviewOutcome::Persisted(_) => {
                        info!(
                            domain = ctx.domain,
                            url = %ctx.url,
                            "Freshly generated schema produced valid product"
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
                        last_norm_error: norm_err,
                    });
                }
                Err(ScraperError::NormalizationError(norm_err))
            }
        }
    }
}
