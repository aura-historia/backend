use crate::scraper::css_selector::product_schema::{ProductCssSelectorSchema, RawExtractedProduct};
use crate::scraper::normalization::error::{NormalizationError, NormalizationFailureScope};
use crate::scraper::normalization::product::NormalizedProduct;
use crate::scraper::normalization::product_normalization_service::{
    NormalizationFailure, NormalizationSuccess,
};
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::extraction::schema_candidates::{
    collect_applicable_candidates, rank_candidates, score_raw_product,
};
use crate::scraper::scraper_service::image_validation::filter_valid_image_urls;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use common::shop_id::ShopId;
use tracing::debug;
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
    /// schema as generation input.
    GenerateNewSchema {
        reason: FreshSchemaGenerationReason,
    },
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
    /// selected as generation input.
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
        // `scraper::Html` is `!Send`: parse and apply every cached schema in a
        // synchronous block so the parsed document is dropped before awaits.
        let mut candidates = {
            let parsed = scraper::Html::parse_document(html);
            let candidate_set = collect_applicable_candidates(schemas, &parsed);

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

            candidate_set.candidates
        };

        for candidate in &mut candidates {
            candidate.raw.images = match filter_valid_image_urls(
                std::mem::take(&mut candidate.raw.images),
                url,
                &*self.image_validator,
            )
            .await
            {
                Ok(images) => images,
                Err(NormalizationError::NoValidImages { .. }) => Vec::new(),
                Err(err) => return Err(ScraperError::NormalizationError(err)),
            };
            candidate.score = score_raw_product(&candidate.raw);
        }

        rank_candidates(&mut candidates);
        for candidate in &candidates {
            debug!(
                candidate_schema_index = candidate.schema_index,
                candidate_schema_score = candidate.score.as_usize(),
                "Ranked cached schema candidate"
            );
        }

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
                    if err.failure_scope() == NormalizationFailureScope::CandidateData =>
                {
                    debug!(
                        candidate_schema_index = candidate.schema_index,
                        candidate_schema_score = candidate.score.as_usize(),
                        candidate_normalization_result = "candidate_data_failure",
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
        raw: RawExtractedProduct,
    ) -> Result<NormalizedProduct, ScraperError> {
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
}
