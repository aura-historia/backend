use crate::scraper::css_selector::product_schema::{ProductCssSelectorSchema, RawExtractedProduct};
use crate::scraper::normalization::error::{NormalizationError, NormalizationFailureScope};

use crate::scraper::normalization::product_normalization_service::{
    NormalizationFailure, NormalizationSuccess, ProductNormalizationContext, prepare_product,
};
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::extraction::schema_candidates::{
    AppliedSchemaCandidate, PreparedSchemaCandidate, collect_applicable_candidates,
    rank_prepared_candidates, score_prepared_product,
};
use crate::scraper::scraper_service::image_validation::filter_valid_image_urls;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use listing_source_core::ListingSourceId;
use money::Currency;
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
    /// prepared candidate.
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

pub(crate) struct PreparedSchemaSelection {
    pub(crate) prepared:
        crate::scraper::normalization::product_normalization_service::PreparedProduct,
    /// Untouched extraction retained as source evidence.
    pub(crate) raw: RawExtractedProduct,
    /// Crawler-validated image URLs in canonical group order for raw capture.
    pub(crate) validated_image_urls: Vec<String>,
    pub(crate) fallback_currency: Option<Currency>,
    pub(crate) schema: ProductCssSelectorSchema,
    pub(crate) fresh_schema: bool,
}

pub(crate) enum ExistingSchemaSelection {
    Prepared(Box<PreparedSchemaSelection>),
    /// Cached selection cannot produce a valid product — generate a
    /// completely new schema for the current page. Never carries a cached
    /// schema as generation input.
    GenerateNewSchema {
        reason: FreshSchemaGenerationReason,
    },
}

/// Cached candidates that applied to the page and passed deterministic
/// preparation, in production ranking order.
pub(crate) struct PreparedCachedCandidates<'a> {
    /// How many cached schemas applied, before preparation rejected any.
    pub(crate) applied_schema_count: usize,
    pub(crate) candidates: Vec<PreparedSchemaCandidate<'a>>,
}

/// Applies every cached schema to an already parsed page and logs the outcome.
/// Synchronous so the `!Send` document never lives across an await.
pub(crate) fn apply_cached_schemas<'a>(
    schemas: &'a [ProductCssSelectorSchema],
    parsed: &scraper::Html,
) -> Vec<AppliedSchemaCandidate<'a>> {
    let candidate_set = collect_applicable_candidates(schemas, parsed);

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
}

struct AppliedSchemaNormalizationContext<'a> {
    listing_source_id: &'a ListingSourceId,
    effective_url: &'a Url,
    schema: &'a ProductCssSelectorSchema,
    raw: RawExtractedProduct,
    validated_raw: RawExtractedProduct,
    source_record_key: &'a str,
    fallback_currency: Option<Currency>,
}

// ---------------------------------------------------------------------------
// impl ScraperServiceImpl
// ---------------------------------------------------------------------------

impl ScraperServiceImpl {
    /// Applies every cached schema to the current page, validates each
    /// candidate's images and prepares it deterministically, then ranks the
    /// survivors by completeness. A candidate-local failure rejects that
    /// candidate only; an external failure aborts. This is the single seam that
    /// decides whether cached schemas still extract a product from a page; it
    /// never generates schemas or spends LLM budget.
    pub(crate) async fn prepare_cached_candidates<'a>(
        &self,
        schemas: &'a [ProductCssSelectorSchema],
        html: &str,
        context: &ProductNormalizationContext,
    ) -> Result<PreparedCachedCandidates<'a>, ScraperError> {
        // `scraper::Html` is `!Send`: parse and apply every cached schema in a
        // synchronous block so the parsed document is dropped before awaits.
        let candidates = {
            let parsed = scraper::Html::parse_document(html);
            apply_cached_schemas(schemas, &parsed)
        };
        self.prepare_applied_candidates(candidates, context).await
    }

    /// Validates each applied candidate's images, prepares it deterministically
    /// and ranks the survivors; see [`Self::prepare_cached_candidates`].
    pub(crate) async fn prepare_applied_candidates<'a>(
        &self,
        candidates: Vec<AppliedSchemaCandidate<'a>>,
        context: &ProductNormalizationContext,
    ) -> Result<PreparedCachedCandidates<'a>, ScraperError> {
        let applied_schema_count = candidates.len();
        let mut prepared_candidates = Vec::new();
        // Image validation is candidate-local and does not call the LLM. Do
        // it before scoring so thumbnails and malformed URLs cannot inflate
        // richness.
        for candidate in candidates {
            let raw = candidate.raw;
            let mut validated_raw = raw.clone();
            let images = std::mem::take(&mut validated_raw.images);
            validated_raw.images = match filter_valid_image_urls(
                images,
                &context.effective_url,
                &*self.image_validator,
            )
            .await
            {
                Ok(images) => images,
                Err(NormalizationError::NoValidImages { .. }) => Vec::new(),
                Err(err) => return Err(ScraperError::NormalizationError(err)),
            };
            match prepare_product(validated_raw.clone(), context.clone()) {
                Ok(prepared) => {
                    let score = score_prepared_product(&validated_raw, &prepared);
                    prepared_candidates.push(PreparedSchemaCandidate {
                        schema_index: candidate.schema_index,
                        schema: candidate.schema,
                        raw,
                        validated_raw,
                        prepared,
                        score,
                    });
                }
                Err(err) if err.failure_scope() == NormalizationFailureScope::CandidateData => {
                    debug!(
                        candidate_schema_index = candidate.schema_index,
                        candidate_rejection_reason = err.failure_reason(),
                        "Cached candidate rejected during deterministic preparation"
                    );
                }
                Err(err) => return Err(ScraperError::NormalizationError(err)),
            }
        }
        rank_prepared_candidates(&mut prepared_candidates);
        for candidate in &prepared_candidates {
            debug!(
                candidate_schema_index = candidate.schema_index,
                candidate_schema_score = candidate.score.as_usize(),
                "Ranked cached schema candidate"
            );
        }

        Ok(PreparedCachedCandidates {
            applied_schema_count,
            candidates: prepared_candidates,
        })
    }

    /// Applies every cached schema to the current page, ranks successful
    /// extractions by attribute completeness, and normalizes candidates from
    /// richest to least rich.
    ///
    /// The first candidate that normalizes successfully wins. When no cached
    /// candidate succeeds — either because none applied or none prepared —
    /// returns [`ExistingSchemaSelection::GenerateNewSchema`] so the caller
    /// falls back to fresh schema generation. No cached schema is ever
    /// selected as generation input.
    #[tracing::instrument(
        skip(self, schemas, html),
        fields(
            listing_source_id = %listing_source_id,
            url = %url,
            schema_count = schemas.len()
        )
    )]
    pub(crate) async fn select_existing_schema_with_normalization(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        source_record_key: &str,
        html: &str,
        schemas: &[ProductCssSelectorSchema],
        fallback_currency: Option<Currency>,
    ) -> Result<ExistingSchemaSelection, ScraperError> {
        let PreparedCachedCandidates {
            applied_schema_count,
            candidates: prepared_candidates,
        } = self
            .prepare_cached_candidates(
                schemas,
                html,
                &ProductNormalizationContext {
                    effective_url: url.clone(),
                    source_record_key: source_record_key.to_owned(),
                    fallback_currency,
                },
            )
            .await?;

        if prepared_candidates.is_empty() {
            let reason = if applied_schema_count == 0 {
                FreshSchemaGenerationReason::NoCachedSchemaApplied
            } else {
                FreshSchemaGenerationReason::NoCachedSchemaNormalized
            };
            debug!(
                fresh_schema_generation_reason = reason.as_str(),
                "No usable cached schema candidate; fresh schema generation required"
            );
            return Ok(ExistingSchemaSelection::GenerateNewSchema { reason });
        }

        for candidate in prepared_candidates {
            let raw = candidate.raw;
            match self
                .normalize_applied_schema(AppliedSchemaNormalizationContext {
                    listing_source_id,
                    effective_url: url,
                    schema: candidate.schema,
                    raw,
                    validated_raw: candidate.validated_raw,
                    source_record_key,
                    fallback_currency,
                })
                .await
            {
                Ok(product) => {
                    debug!(
                        selected_schema_index = candidate.schema_index,
                        selected_schema_score = candidate.score.as_usize(),
                        candidate_normalization_result = "success",
                        "Cached schema selected"
                    );
                    return Ok(ExistingSchemaSelection::Prepared(Box::new(product)));
                }
                Err(ScraperError::NormalizationError(err))
                    if err.failure_scope() == NormalizationFailureScope::CandidateData =>
                {
                    debug!(
                        candidate_schema_index = candidate.schema_index,
                        candidate_schema_score = candidate.score.as_usize(),
                        candidate_rejection_reason = err.failure_reason(),
                        candidate_rejection_scope = "candidate_data",
                        candidate_normalization_result = "candidate_data_failure",
                        "Cached candidate normalization failed; trying next candidate"
                    );
                }
                Err(ScraperError::NormalizationError(err)) => {
                    debug!(
                        candidate_schema_index = candidate.schema_index,
                        candidate_schema_score = candidate.score.as_usize(),
                        normalization_failure_reason = err.failure_reason(),
                        normalization_failure_scope = "external",
                        "Cached candidate normalization failed externally; aborting selection"
                    );
                    return Err(ScraperError::NormalizationError(err));
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
        context: AppliedSchemaNormalizationContext<'_>,
    ) -> Result<PreparedSchemaSelection, ScraperError> {
        let AppliedSchemaNormalizationContext {
            listing_source_id,
            effective_url: url,
            schema: selected_schema,
            raw,
            validated_raw,
            source_record_key,
            fallback_currency,
        } = context;
        let validated_image_urls = validated_raw.images.clone();
        let normalization = self
            .normalization_service
            .normalize(
                validated_raw,
                ProductNormalizationContext {
                    effective_url: url.clone(),
                    source_record_key: source_record_key.to_owned(),
                    fallback_currency,
                },
            )
            .await;
        match normalization {
            Ok(NormalizationSuccess {
                prepared,
                llm_calls_used,
            }) => {
                self.consume_llm_budget_n_or_err(listing_source_id, url, llm_calls_used)
                    .await?;
                Ok(PreparedSchemaSelection {
                    prepared,
                    raw,
                    validated_image_urls,
                    fallback_currency,
                    schema: selected_schema.clone(),
                    fresh_schema: false,
                })
            }
            Err(NormalizationFailure {
                error,
                llm_calls_used,
            }) => {
                self.consume_llm_budget_n_or_err(listing_source_id, url, llm_calls_used)
                    .await?;
                Err(ScraperError::NormalizationError(error))
            }
        }
    }
}
