use crate::scraper::css_selector::product_schema::{ProductCssSelectorSchema, RawExtractedProduct};
use crate::scraper::plausibility::PlausibilityFindings;
use crate::scraper::scraper_service::extraction::schema_candidates::{
    AppliedSchemaCandidate, CheckedSchemaCandidate, collect_applicable_candidates,
    rank_checked_candidates,
};
use crate::scraper::scraper_service::image_validation::filter_valid_image_urls;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use listing_source_core::ListingSourceId;
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
    /// One or more cached schemas applied, but every extraction failed a
    /// rejecting plausibility check.
    NoCachedSchemaPlausible,
}

impl FreshSchemaGenerationReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NoCachedSchemaApplied => "no_cached_schema_applied",
            Self::NoCachedSchemaPlausible => "no_cached_schema_plausible",
        }
    }
}

/// The schema whose extraction of the current page is captured.
pub(crate) struct SelectedSchemaCandidate {
    /// Untouched extraction retained as source evidence.
    pub(crate) raw: RawExtractedProduct,
    /// Crawler-validated image URLs in canonical group order for raw capture.
    pub(crate) validated_image_urls: Vec<String>,
    pub(crate) schema: ProductCssSelectorSchema,
    pub(crate) fresh_schema: bool,
}

pub(crate) enum ExistingSchemaSelection {
    Selected(Box<SelectedSchemaCandidate>),
    /// Cached selection cannot produce a plausible product — generate a
    /// completely new schema for the current page. Never carries a cached
    /// schema as generation input.
    GenerateNewSchema {
        reason: FreshSchemaGenerationReason,
    },
    /// No cached candidate is plausible, and one fails only because the page leaves a
    /// required value empty where it found the element: the page lacks the value, not a
    /// fitting schema, so no schema work starts for it.
    MissingRequiredValue {
        findings: PlausibilityFindings,
    },
}

/// Cached candidates that applied to the page and passed the rejecting
/// plausibility checks, in production ranking order.
pub(crate) struct CheckedCachedCandidates<'a> {
    /// How many cached schemas applied, before plausibility rejected any.
    pub(crate) applied_schema_count: usize,
    /// The rejecting findings of the first applied candidate, in schema order, that blank
    /// required values alone rejected (see [`PlausibilityFindings::missing_required_values`]).
    pub(crate) missing_required_values: Option<PlausibilityFindings>,
    pub(crate) candidates: Vec<CheckedSchemaCandidate<'a>>,
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

// ---------------------------------------------------------------------------
// impl ScraperServiceImpl
// ---------------------------------------------------------------------------

impl ScraperServiceImpl {
    /// Applies every cached schema to the current page, rejects extractions that
    /// fail a rejecting plausibility check, validates the images of the rest and
    /// ranks them. It never generates schemas or spends LLM budget.
    pub(crate) async fn check_cached_candidates<'a>(
        &self,
        schemas: &'a [ProductCssSelectorSchema],
        html: &str,
        effective_url: &Url,
    ) -> CheckedCachedCandidates<'a> {
        // `scraper::Html` is `!Send`: parse and apply every cached schema in a
        // synchronous block so the parsed document is dropped before awaits.
        let candidates = {
            let parsed = scraper::Html::parse_document(html);
            apply_cached_schemas(schemas, &parsed)
        };
        let applied_schema_count = candidates.len();
        let mut missing_required_values = None;
        let mut checked_candidates = Vec::new();
        for candidate in candidates {
            let findings = PlausibilityFindings::of(&candidate.raw);
            if findings.rejects_candidate() {
                debug!(
                    candidate_schema_index = candidate.schema_index,
                    candidate_plausibility_findings = %findings,
                    "Cached candidate rejected by plausibility checks"
                );
                if missing_required_values.is_none() {
                    missing_required_values = findings.missing_required_values();
                }
                continue;
            }
            // Image validation is candidate-local and does not call the LLM. It
            // runs before scoring so thumbnails and malformed URLs cannot inflate
            // richness.
            let validated_image_urls = filter_valid_image_urls(
                candidate.raw.images.clone(),
                effective_url,
                &*self.image_validator,
            )
            .await;
            checked_candidates.push(CheckedSchemaCandidate::new(
                candidate,
                findings,
                validated_image_urls,
            ));
        }
        rank_checked_candidates(&mut checked_candidates);
        for candidate in &checked_candidates {
            debug!(
                candidate_schema_index = candidate.schema_index,
                candidate_schema_score = candidate.score.as_usize(),
                candidate_rank_penalty = candidate.findings.rank_penalty(),
                candidate_plausibility_findings = %candidate.findings,
                "Ranked cached schema candidate"
            );
        }

        CheckedCachedCandidates {
            applied_schema_count,
            missing_required_values,
            candidates: checked_candidates,
        }
    }

    /// Applies every cached schema to the current page and selects the
    /// best-ranked plausible extraction.
    ///
    /// When no cached candidate is plausible — either because none applied or
    /// every extraction failed a rejecting check — returns
    /// [`ExistingSchemaSelection::GenerateNewSchema`] so the caller falls back to
    /// fresh schema generation. No cached schema is ever selected as generation
    /// input. A page that one cached candidate fails only for blank required values
    /// returns [`ExistingSchemaSelection::MissingRequiredValue`] instead.
    #[tracing::instrument(
        skip(self, schemas, html),
        fields(
            listing_source_id = %listing_source_id,
            url = %url,
            schema_count = schemas.len()
        )
    )]
    pub(crate) async fn select_cached_schema(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        html: &str,
        schemas: &[ProductCssSelectorSchema],
    ) -> ExistingSchemaSelection {
        let CheckedCachedCandidates {
            applied_schema_count,
            missing_required_values,
            candidates,
        } = self.check_cached_candidates(schemas, html, url).await;

        let Some(selected) = candidates.into_iter().next() else {
            if let Some(findings) = missing_required_values {
                debug!(
                    missing_required_values = %findings,
                    "No plausible cached schema candidate; the page leaves a required value empty"
                );
                return ExistingSchemaSelection::MissingRequiredValue { findings };
            }
            let reason = if applied_schema_count == 0 {
                FreshSchemaGenerationReason::NoCachedSchemaApplied
            } else {
                FreshSchemaGenerationReason::NoCachedSchemaPlausible
            };
            debug!(
                fresh_schema_generation_reason = reason.as_str(),
                "No plausible cached schema candidate; fresh schema generation required"
            );
            return ExistingSchemaSelection::GenerateNewSchema { reason };
        };

        debug!(
            selected_schema_index = selected.schema_index,
            selected_schema_score = selected.score.as_usize(),
            selected_rank_penalty = selected.findings.rank_penalty(),
            "Cached schema selected"
        );
        ExistingSchemaSelection::Selected(Box::new(SelectedSchemaCandidate {
            raw: selected.raw,
            validated_image_urls: selected.validated_image_urls,
            schema: selected.schema.clone(),
            fresh_schema: false,
        }))
    }
}
