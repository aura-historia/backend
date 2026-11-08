use crate::scraper::css_selector::product_schema::{
    ApplySchemaError, ListingSourceProductSchema, OptionalMisses, ProductCssSelectorSchema,
    RawExtractedProduct, SchemaApplication,
};
use crate::scraper::plausibility::PlausibilityFindings;
use crate::scraper::scraper_service::extraction::schema_candidates::{
    AppliedSchemaCandidate, CheckedSchemaCandidate, rank_checked_candidates,
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
    /// The index of `schema` in the ListingSource's ordered schema set; `None` for a fresh
    /// schema, which takes the next index.
    pub(crate) cached_schema_index: Option<usize>,
    /// The ranking score of the cached extraction; `None` for a fresh schema and for a
    /// title-less extraction, which is not ranked.
    pub(crate) score: Option<usize>,
    /// The title the URL was last captured with, captured in place of the blank title of
    /// `raw` (see [`CheckedCachedCandidates::untitled`]). `None` captures the extracted title.
    pub(crate) carried_forward_title: Option<String>,
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
    /// Candidates that a blank `title` alone rejected, in production ranking order, checked only
    /// when the URL was captured with a title before: the shop blanked the name of a listing
    /// the crawler already knows, as some do when an item sells, and the page still says what
    /// changed. Empty otherwise.
    pub(crate) untitled: Vec<CheckedSchemaCandidate<'a>>,
}

/// Logs how many cached schemas applied, and each apply failure with its field and error
/// (which names the selector) and, where the extraction kept them, the optional-field misses
/// before it.
fn log_cached_schema_application<'e>(
    schema_count: usize,
    applied_count: usize,
    apply_failures: impl ExactSizeIterator<
        Item = (usize, &'e ApplySchemaError, Option<&'e OptionalMisses>),
    >,
) {
    debug!(
        schema_candidates_total = schema_count,
        schema_candidates_applied = applied_count,
        schema_candidates_apply_failed = apply_failures.len(),
        "Cached schema application complete"
    );
    for (schema_index, err, optional_misses) in apply_failures {
        debug!(
            candidate_schema_index = schema_index,
            candidate_apply_field = err.field(),
            candidate_apply_error = %err,
            candidate_optional_misses = optional_misses
                .filter(|misses| !misses.is_empty())
                .map(tracing::field::display),
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
/// It keeps the optional-field misses and logs them only when selection uses it
/// ([`Self::applied_candidates`]), so a page skipped as unchanged logs none.
pub(crate) struct ExtractedSchemaSet {
    schemas: ListingSourceProductSchema,
    applications: Vec<SchemaApplication>,
}

impl ExtractedSchemaSet {
    /// Applies every schema of `schemas` to the already parsed `page`.
    pub(crate) fn extract(schemas: ListingSourceProductSchema, page: &scraper::Html) -> Self {
        let applications = schemas
            .product_schemas
            .iter()
            .map(|schema| schema.extract_keeping_misses(page))
            .collect();
        Self {
            schemas,
            applications,
        }
    }

    /// Parses `html` and applies every schema of `schemas` to it, for a schema set that was
    /// not known when the page was first parsed.
    ///
    /// A scrape parses its page once when it fetches it (see `parse_fetched_page`), and only
    /// two paths parse it again here, because the set they extract is known only after an
    /// await: the schemas obtained for a page without cached schemas, generated or found
    /// stored by then, when stored removed-page schemas made the scrape parse the page before;
    /// and the live schemas re-read after a review was approved during the scrape. The `!Send` document cannot
    /// live across those awaits, which read the database, wait on a review or call the LLM,
    /// without making the scrape future `!Send` or moving the document to a thread of its own.
    /// Both paths run at most once per page and only while a ListingSource gains or changes
    /// its schemas, so the second parse is not worth that restructure.
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
        self.applications.iter().map(|application| {
            application
                .outcome
                .as_ref()
                .ok()
                .map(|extraction| &extraction.raw)
        })
    }

    /// The page hash of the page for this schema set (see [`hash_extractions`]).
    pub(crate) fn page_hash(&self) -> String {
        hash_extractions(self.extraction_outcomes())
    }

    /// The schemas that apply, with their raw extraction, for selection. Logs the outcome, the
    /// optional-field misses of each schema that applies, and each apply failure with the
    /// misses before it.
    pub(crate) fn applied_candidates(&self) -> Vec<AppliedSchemaCandidate<'_>> {
        let mut candidates = Vec::new();
        let mut apply_failures = Vec::new();
        for (schema_index, (schema, application)) in self
            .schemas
            .product_schemas
            .iter()
            .zip(&self.applications)
            .enumerate()
        {
            match &application.outcome {
                Ok(extraction) => {
                    application.optional_misses.log();
                    candidates.push(AppliedSchemaCandidate {
                        schema_index,
                        schema,
                        raw: extraction.raw.clone(),
                    });
                }
                Err(err) => {
                    apply_failures.push((schema_index, err, Some(&application.optional_misses)))
                }
            }
        }
        log_cached_schema_application(
            self.applications.len(),
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
    /// ranks them. When the URL was captured with `last_captured_title`, the candidates a blank
    /// title alone rejected are validated and ranked apart. It never generates schemas or
    /// spends LLM budget.
    pub(crate) async fn check_cached_candidates<'a>(
        &self,
        extracted: &'a ExtractedSchemaSet,
        effective_url: &Url,
        last_captured_title: Option<&str>,
    ) -> CheckedCachedCandidates<'a> {
        let candidates = extracted.applied_candidates();
        let applied_schema_count = candidates.len();
        let mut missing_required_values = None;
        let mut checked_candidates = Vec::new();
        let mut untitled = Vec::new();
        for candidate in candidates {
            let findings = PlausibilityFindings::of(&candidate.raw);
            let carries_title_forward =
                last_captured_title.is_some() && findings.rejects_only_for_blank_title();
            if findings.rejects_candidate() {
                debug!(
                    candidate_schema_index = candidate.schema_index,
                    candidate_plausibility_findings = %findings,
                    "Cached candidate rejected by plausibility checks"
                );
                if missing_required_values.is_none() {
                    missing_required_values = findings.missing_required_values();
                }
                if !carries_title_forward {
                    continue;
                }
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
            let checked = CheckedSchemaCandidate::new(candidate, findings, validated_image_urls);
            if carries_title_forward {
                untitled.push(checked);
            } else {
                checked_candidates.push(checked);
            }
        }
        rank_checked_candidates(&mut checked_candidates);
        rank_checked_candidates(&mut untitled);
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
            untitled,
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
    /// returns [`ExistingSchemaSelection::MissingRequiredValue`] instead, unless a blank title
    /// is all that is missing and the URL was captured with `last_captured_title` before: then
    /// the best such candidate is selected and captures that title in place of the blank one
    /// (`docs/crawler/extraction-and-review.md`, missing required values).
    #[tracing::instrument(
        skip(self, extracted, last_captured_title),
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
        last_captured_title: Option<&str>,
    ) -> ExistingSchemaSelection {
        let CheckedCachedCandidates {
            applied_schema_count,
            missing_required_values,
            candidates,
            untitled,
        } = self
            .check_cached_candidates(extracted, url, last_captured_title)
            .await;

        let Some(selected) = candidates.into_iter().next() else {
            if let (Some(untitled), Some(title)) =
                (untitled.into_iter().next(), last_captured_title)
            {
                debug!(
                    selected_schema_index = untitled.schema_index,
                    "No plausible cached schema candidate; the page blanked the title it was captured with, which is carried forward"
                );
                return ExistingSchemaSelection::Selected(Box::new(SelectedSchemaCandidate {
                    raw: untitled.raw,
                    validated_image_urls: untitled.validated_image_urls,
                    schema: untitled.schema.clone(),
                    fresh_schema: false,
                    cached_schema_index: Some(untitled.schema_index),
                    score: None,
                    carried_forward_title: Some(title.to_owned()),
                }));
            }
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
            cached_schema_index: Some(selected.schema_index),
            score: Some(selected.score.as_usize()),
            carried_forward_title: None,
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
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex, PoisonError};
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

    /// Fails on its required `state` after missing [`PAGE`]'s optional description.
    fn failing_state_schema() -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            state: rule("#missing-state", ExtractionKind::Text),
            ..applying_schema()
        }
    }

    type LoggedEvent = (tracing::Level, BTreeMap<String, String>);

    /// The level and fields of each event logged while it is the default subscriber.
    #[derive(Clone, Default)]
    struct LoggedEvents(Arc<Mutex<Vec<LoggedEvent>>>);

    impl LoggedEvents {
        fn taken(&self) -> Vec<LoggedEvent> {
            std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
        }
    }

    struct EventFields(BTreeMap<String, String>);

    impl tracing::field::Visit for EventFields {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.0.insert(field.name().to_owned(), value.to_owned());
        }

        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0.insert(field.name().to_owned(), format!("{value:?}"));
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for LoggedEvents {
        fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
            let mut fields = EventFields(BTreeMap::new());
            event.record(&mut fields);
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((*event.metadata().level(), fields.0));
        }
    }

    fn optional_misses_logged(events: &[LoggedEvent]) -> Vec<(tracing::Level, &str)> {
        events
            .iter()
            .filter(|(_, fields)| {
                fields.get("message").map(String::as_str)
                    == Some("Optional schema rule yielded no value; leaving the field empty")
            })
            .map(|(level, fields)| (*level, fields["field"].as_str()))
            .collect()
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
        let logged = LoggedEvents::default();
        let subscriber = tracing_subscriber::registry().with(logged.clone());

        tracing::subscriber::with_default(subscriber, || {
            tracing::callsite::rebuild_interest_cache();
            let extracted = ExtractedSchemaSet::parse_and_extract(
                schema_set(vec![failing_schema(), applying_schema()]),
                PAGE,
            );
            extracted.page_hash();
            let logged_before_selection = logged.taken();

            let candidates = extracted.applied_candidates();

            assert!(logged_before_selection.is_empty());
            assert_eq!(
                optional_misses_logged(&logged.taken()),
                vec![(tracing::Level::DEBUG, "description")],
                "selection logs the description miss once, at debug"
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

    /// Selector drift that breaks a required rule shows the optional rules it broke too.
    #[test]
    fn should_log_a_required_failure_with_its_selector_and_the_optional_misses() {
        let logged = LoggedEvents::default();
        let subscriber = tracing_subscriber::registry().with(logged.clone());

        tracing::subscriber::with_default(subscriber, || {
            tracing::callsite::rebuild_interest_cache();
            let extracted = ExtractedSchemaSet::parse_and_extract(
                schema_set(vec![failing_state_schema()]),
                PAGE,
            );

            assert!(extracted.applied_candidates().is_empty());

            let events = logged.taken();
            assert!(
                optional_misses_logged(&events).is_empty(),
                "a failed schema logs its misses with its failure only"
            );
            let failures = events
                .iter()
                .filter(|(_, fields)| {
                    fields.get("message").map(String::as_str)
                        == Some("Cached schema failed to apply")
                })
                .collect::<Vec<_>>();
            let [(level, fields)] = failures.as_slice() else {
                panic!("expected one apply failure, logged {failures:?}");
            };
            assert_eq!(*level, tracing::Level::DEBUG);
            assert_eq!(fields["candidate_apply_field"], "state");
            assert!(
                fields["candidate_apply_error"].contains("`#missing-state`"),
                "{fields:?}"
            );
            assert_eq!(
                fields["candidate_optional_misses"],
                "description: no element matched selector `#description` (or any additional selectors)"
            );
        });
    }
}
