use crate::scraper::css_selector::product_schema::{
    ApplySchemaError, ExtractionLogging, ListingSourceProductSchema, ProductCssSelectorSchema,
    RawExtractedProduct, SchemaExtraction,
};
use crate::scraper::plausibility::PlausibilityFindings;
use crate::scraper::scraper_service::extraction::schema_candidates::{
    AppliedSchemaCandidate, CheckedSchemaCandidate, collect_applicable_candidates,
    rank_checked_candidates,
};
use crate::scraper::scraper_service::image_validation::filter_valid_image_urls;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use crate::scraper::scraper_service::util::hash::hash_extractions;
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
    log_cached_schema_application(
        schemas.len(),
        candidate_set.candidates.len(),
        candidate_set
            .apply_failures
            .iter()
            .map(|(schema_index, err)| (*schema_index, err)),
    );
    candidate_set.candidates
}

fn log_cached_schema_application<'e>(
    schema_count: usize,
    applied_count: usize,
    apply_failures: impl ExactSizeIterator<Item = (usize, &'e ApplySchemaError)>,
) {
    debug!(
        schema_candidates_total = schema_count,
        schema_candidates_applied = applied_count,
        schema_candidates_apply_failed = apply_failures.len(),
        "Cached schema application complete"
    );
    for (schema_index, err) in apply_failures {
        debug!(
            candidate_schema_index = schema_index,
            candidate_apply_error = ?err,
            "Cached schema failed to apply"
        );
    }
}

// ---------------------------------------------------------------------------
// ExtractedSchemaSet
// ---------------------------------------------------------------------------

/// What each schema of a product schema set extracts from one parse of a page, in schema
/// order.
///
/// It keeps none of the `!Send` parsed document, so one parse of a fetched page serves the
/// removal check, the page hash and cached-schema selection across the awaits between them.
/// It extracts silently and logs the optional-field misses only when selection uses it
/// ([`Self::applied_candidates`]), so a page skipped as unchanged logs none.
pub(crate) struct ExtractedSchemaSet {
    schemas: ListingSourceProductSchema,
    extractions: Vec<Result<SchemaExtraction, ApplySchemaError>>,
}

impl ExtractedSchemaSet {
    /// Applies every schema of `schemas` to the already parsed `page`.
    pub(crate) fn extract(schemas: ListingSourceProductSchema, page: &scraper::Html) -> Self {
        let extractions = schemas
            .product_schemas
            .iter()
            .map(|schema| schema.extract(page, ExtractionLogging::Silent))
            .collect();
        Self {
            schemas,
            extractions,
        }
    }

    /// Parses `html` and applies every schema of `schemas` to it, for a schema set that was
    /// not known when the page was first parsed.
    pub(crate) fn parse_and_extract(schemas: ListingSourceProductSchema, html: &str) -> Self {
        Self::extract(schemas, &scraper::Html::parse_document(html))
    }

    pub(crate) fn product_schemas(&self) -> &[ProductCssSelectorSchema] {
        &self.schemas.product_schemas
    }

    pub(crate) fn into_schemas(self) -> ListingSourceProductSchema {
        self.schemas
    }

    /// The raw extraction of each schema that applies, in schema order.
    pub(crate) fn raw_extractions(&self) -> impl Iterator<Item = &RawExtractedProduct> {
        self.extraction_outcomes().flatten()
    }

    /// The raw extraction of each schema in schema order, or `None` where it does not apply.
    pub(crate) fn extraction_outcomes(&self) -> impl Iterator<Item = Option<&RawExtractedProduct>> {
        self.extractions
            .iter()
            .map(|extraction| extraction.as_ref().ok().map(|extraction| &extraction.raw))
    }

    /// The page hash of the page for this schema set (see [`hash_extractions`]).
    pub(crate) fn page_hash(&self) -> String {
        hash_extractions(self.extraction_outcomes())
    }

    /// The schemas that apply, with their raw extraction, for selection. Logs the outcome and
    /// the optional-field misses the extraction kept.
    pub(crate) fn applied_candidates(&self) -> Vec<AppliedSchemaCandidate<'_>> {
        let mut candidates = Vec::new();
        let mut apply_failures = Vec::new();
        for (schema_index, (schema, extraction)) in self
            .schemas
            .product_schemas
            .iter()
            .zip(&self.extractions)
            .enumerate()
        {
            match extraction {
                Ok(extraction) => {
                    extraction.log_unlogged_misses();
                    candidates.push(AppliedSchemaCandidate {
                        schema_index,
                        schema,
                        raw: extraction.raw.clone(),
                    });
                }
                Err(err) => apply_failures.push((schema_index, err)),
            }
        }
        log_cached_schema_application(
            self.extractions.len(),
            candidates.len(),
            apply_failures.into_iter(),
        );
        candidates
    }
}

// ---------------------------------------------------------------------------
// impl ScraperServiceImpl
// ---------------------------------------------------------------------------

impl ScraperServiceImpl {
    /// Takes the cached schemas that applied to the current page, rejects extractions that
    /// fail a rejecting plausibility check, validates the images of the rest and
    /// ranks them. It never generates schemas or spends LLM budget.
    pub(crate) async fn check_cached_candidates<'a>(
        &self,
        extracted: &'a ExtractedSchemaSet,
        effective_url: &Url,
    ) -> CheckedCachedCandidates<'a> {
        let candidates = extracted.applied_candidates();
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

    /// Selects the best-ranked plausible extraction of the cached schemas from the
    /// current page.
    ///
    /// When no cached candidate is plausible — either because none applied or
    /// every extraction failed a rejecting check — returns
    /// [`ExistingSchemaSelection::GenerateNewSchema`] so the caller falls back to
    /// fresh schema generation. No cached schema is ever selected as generation
    /// input. A page that one cached candidate fails only for blank required values
    /// returns [`ExistingSchemaSelection::MissingRequiredValue`] instead.
    #[tracing::instrument(
        skip(self, extracted),
        fields(
            listing_source_id = %listing_source_id,
            url = %url,
            schema_count = extracted.product_schemas().len()
        )
    )]
    pub(crate) async fn select_cached_schema(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        extracted: &ExtractedSchemaSet,
    ) -> ExistingSchemaSelection {
        let CheckedCachedCandidates {
            applied_schema_count,
            missing_required_values,
            candidates,
        } = self.check_cached_candidates(extracted, url).await;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::css_selector::rule::{
        CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule,
    };
    use crate::scraper::scraper_service::util::hash::hash_page;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tracing_subscriber::layer::{Context, SubscriberExt};

    const PAGE: &str = r#"<html><body><main>
        <h1>Walnut Dumb Waiter</h1>
        <span id="price">£795</span>
        <span id="state">In Stock</span>
        <a class="gallery" href="/images/waiter-1800.jpg"><img src="/images/waiter-300.jpg"></a>
        </main></body></html>"#;

    fn rule(selector: &str, extract: ExtractionKind) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: vec![],
            extract,
            cardinality: ExtractionCardinality::First,
        }
    }

    /// Applies to [`PAGE`], and misses its optional description.
    fn applying_schema() -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            source_listing_id: None,
            title: rule("h1", ExtractionKind::Text),
            description: Some(rule("#description", ExtractionKind::Text)),
            price: Some(rule("#price", ExtractionKind::Text)),
            price_estimate_min: None,
            price_estimate_max: None,
            state: rule("#state", ExtractionKind::Text),
            images: ExtractionRule {
                cardinality: ExtractionCardinality::All,
                ..rule("a.gallery", ExtractionKind::ImageUrl)
            },
            raw_attributes: Default::default(),
        }
    }

    fn failing_schema() -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            title: rule("h2.missing", ExtractionKind::Text),
            ..applying_schema()
        }
    }

    fn schema_set(product_schemas: Vec<ProductCssSelectorSchema>) -> ListingSourceProductSchema {
        let now = time::OffsetDateTime::now_utc();
        ListingSourceProductSchema {
            listing_source_id: ListingSourceId::new(),
            product_schemas,
            created: now,
            updated: now,
        }
    }

    /// Counts warnings, the level of a description miss; application outcomes log at debug.
    struct CountWarnings(Arc<AtomicUsize>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CountWarnings {
        fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
            if *event.metadata().level() == tracing::Level::WARN {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    /// A stored page hash must keep matching the hash of the same extraction, or every
    /// unchanged page would be extracted again.
    #[test]
    fn should_hash_the_page_like_the_stored_page_hash() {
        let schemas = vec![failing_schema(), applying_schema()];

        let extracted = ExtractedSchemaSet::parse_and_extract(schema_set(schemas.clone()), PAGE);

        assert_eq!(extracted.page_hash(), hash_page(PAGE, &schemas));
        assert_eq!(extracted.raw_extractions().count(), 1);
    }

    #[test]
    fn should_log_the_optional_field_misses_only_when_selection_uses_the_extraction() {
        let warnings = Arc::new(AtomicUsize::new(0));
        let subscriber = tracing_subscriber::registry().with(CountWarnings(Arc::clone(&warnings)));

        tracing::subscriber::with_default(subscriber, || {
            let extracted = ExtractedSchemaSet::parse_and_extract(
                schema_set(vec![failing_schema(), applying_schema()]),
                PAGE,
            );
            extracted.page_hash();
            let logged_before_selection = warnings.load(Ordering::SeqCst);

            let candidates = extracted.applied_candidates();

            assert_eq!(logged_before_selection, 0);
            assert_eq!(
                warnings.load(Ordering::SeqCst),
                1,
                "selection logs the description miss once"
            );
            assert_eq!(
                candidates
                    .iter()
                    .map(|candidate| candidate.schema_index)
                    .collect::<Vec<_>>(),
                vec![1]
            );
        });
    }
}
