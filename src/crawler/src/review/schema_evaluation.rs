use crate::CrawlerReviewId;
use crate::review::model::{
    CrawlerReviewPage, MatchedRemovedPageTemplate, RemovedPageTemplateMatch,
    RemovedPageTemplateOutcome, SchemaCandidateEvaluation, SchemaMatrix, SchemaPageEvaluation,
    SchemaPageReference, SchemaReviewPageInput, SelectorFieldEvaluation,
};
use crate::scraper::css_selector::product_schema::{
    ApplySchemaError, ProductCssSelectorSchema, SchemaExtraction,
};
use crate::scraper::css_selector::removed_page_schema::{RemovedPageMatch, RemovedPageSchema};
use crate::scraper::css_selector::rule::ExtractionRule;
use crate::scraper::plausibility::{
    CrossPageFindings, IdentityAgreementFindings, PageExtraction, PlausibilityFindings,
};
use crate::scraper::scraper_service::pipeline::scrape_product::{
    RemovedPageVerdict, removed_page_verdict_in,
};
use scraper::{Html, Selector};
use std::collections::HashMap;

/// A review page as the matrix evaluates it: how the matrix refers to it, its HTML and
/// the identity its listing was captured under, if known.
struct MatrixPage<'a> {
    page_reference: SchemaPageReference,
    url: &'a str,
    role: &'a str,
    raw_html: &'a str,
    captured_identity: Option<&'a str>,
}

fn parse_page(raw_html: &str) -> Html {
    #[cfg(test)]
    tests::PAGE_PARSES.with(|parses| parses.set(parses.get() + 1));
    Html::parse_document(raw_html)
}

/// Evaluates the schemas and removed-page templates on the live review pages.
/// `captured_identities` maps a page URL to the source listing id its listing was captured
/// under.
pub(crate) fn evaluate_schema_matrix_for_live_review_pages(
    review_id: CrawlerReviewId,
    schemas: &[ProductCssSelectorSchema],
    removed_page_schemas: &[RemovedPageSchema],
    pages: &[(CrawlerReviewPage, String)],
    captured_identities: &HashMap<String, String>,
) -> SchemaMatrix {
    let pages: Vec<MatrixPage<'_>> = pages
        .iter()
        .map(|(page, raw_html)| MatrixPage {
            page_reference: SchemaPageReference::Persisted {
                review_page_id: page.review_page_id,
            },
            url: &page.url,
            role: &page.role,
            raw_html,
            captured_identity: captured_identities.get(&page.url).map(String::as_str),
        })
        .collect();
    evaluate_matrix(Some(review_id), schemas, removed_page_schemas, &pages)
}

pub(crate) fn evaluate_schema_matrix_for_inputs(
    schemas: &[ProductCssSelectorSchema],
    pages: &[SchemaReviewPageInput],
) -> SchemaMatrix {
    let pages: Vec<MatrixPage<'_>> = pages
        .iter()
        .enumerate()
        .map(|(page_index, page)| MatrixPage {
            page_reference: SchemaPageReference::Input { page_index },
            url: &page.url,
            role: &page.role,
            raw_html: &page.raw_html,
            // Generated-schema inputs carry no capture state.
            captured_identity: None,
        })
        .collect();
    // Generated-schema inputs carry no removed-page templates.
    evaluate_matrix(None, schemas, &[], &pages)
}

/// Evaluates every candidate and removed-page template on every page. Each page is parsed
/// once: every candidate evaluates it, the identities they derive from it are compared and the
/// removed-page templates are matched on it before the next page is parsed, so a single parsed
/// page is held at a time.
fn evaluate_matrix(
    review_id: Option<CrawlerReviewId>,
    schemas: &[ProductCssSelectorSchema],
    removed_page_schemas: &[RemovedPageSchema],
    pages: &[MatrixPage<'_>],
) -> SchemaMatrix {
    let mut candidates: Vec<SchemaCandidateEvaluation> = (0..schemas.len())
        .map(|schema_index| SchemaCandidateEvaluation {
            schema_index,
            pages: Vec::with_capacity(pages.len()),
        })
        .collect();
    let mut removed_page_matches = Vec::new();
    for page in pages {
        let html = parse_page(page.raw_html);
        let mut page_evaluations: Vec<SchemaPageEvaluation> = schemas
            .iter()
            .map(|schema| evaluate_schema_page(schema, page, &html))
            .collect();
        add_identity_agreement_findings(&mut page_evaluations, page.captured_identity);
        for (candidate, evaluation) in candidates.iter_mut().zip(page_evaluations) {
            candidate.pages.push(evaluation);
        }
        removed_page_matches.extend(removed_page_match(
            removed_page_schemas,
            schemas,
            page,
            &html,
        ));
    }
    for candidate in &mut candidates {
        add_cross_page_findings(&mut candidate.pages);
    }
    SchemaMatrix {
        review_id,
        candidates,
        removed_page_matches,
        live_evaluation: None,
    }
}

/// The verdict the scraper's removal guard reaches on a parsed review page once `schemas`
/// are live, so a reviewer sees an over-broad template before approving it; `None` when no
/// removed-page template matches the page. The page URL stands in for the crawler URL's
/// record key: it only matters for a page that yields no identity of its own.
fn removed_page_match(
    removed_page_schemas: &[RemovedPageSchema],
    schemas: &[ProductCssSelectorSchema],
    page: &MatrixPage<'_>,
    html: &Html,
) -> Option<RemovedPageTemplateMatch> {
    let (outcome, matches) = match removed_page_verdict_in(
        removed_page_schemas.to_vec(),
        schemas,
        html,
        page.url,
        page.captured_identity,
    ) {
        RemovedPageVerdict::NotRemoved => return None,
        RemovedPageVerdict::Removed { matches } => (RemovedPageTemplateOutcome::Removed, matches),
        RemovedPageVerdict::ProductStillExtracts { matches } => {
            (RemovedPageTemplateOutcome::ProductStillExtracts, matches)
        }
    };
    Some(RemovedPageTemplateMatch {
        page_reference: page.page_reference.clone(),
        url: page.url.to_owned(),
        role: page.role.to_owned(),
        outcome,
        templates: matched_templates(removed_page_schemas, matches),
    })
}

fn matched_templates(
    removed_page_schemas: &[RemovedPageSchema],
    matches: Vec<RemovedPageMatch>,
) -> Vec<MatchedRemovedPageTemplate> {
    matches
        .into_iter()
        .filter_map(|matched| {
            let template_index = removed_page_schemas
                .iter()
                .position(|schema| *schema == matched.schema)?;
            Some(MatchedRemovedPageTemplate {
                template_index,
                notices: matched.notices,
            })
        })
        .collect()
}

/// Compares one candidate's extractions of the review pages, each of a different
/// product. An extraction a rejecting finding withdrew takes part too: its selectors
/// still produced it.
fn add_cross_page_findings(pages: &mut [SchemaPageEvaluation]) {
    let (page_indices, extractions): (Vec<usize>, Vec<PageExtraction<'_>>) = pages
        .iter()
        .enumerate()
        .filter_map(|(index, page)| {
            let raw = page.extracted.as_ref()?;
            Some((
                index,
                PageExtraction {
                    url: &page.url,
                    raw,
                },
            ))
        })
        .unzip();
    let findings = CrossPageFindings::of(&extractions);
    for (index, findings) in page_indices.into_iter().zip(findings) {
        pages[index].cross_page_findings = findings;
    }
}

/// Compares the identities the candidates that apply to one page derive from it, with
/// each other and with the page's captured identity. `page_evaluations` holds every
/// candidate's evaluation of that page, in candidate order. A withdrawn extraction is left
/// out: runtime selection never captures it.
fn add_identity_agreement_findings(
    page_evaluations: &mut [SchemaPageEvaluation],
    captured_identity: Option<&str>,
) {
    let findings = {
        let extractions: Vec<_> = page_evaluations
            .iter()
            .map(|evaluation| {
                evaluation
                    .apply_ok
                    .then_some(evaluation.extracted.as_ref())
                    .flatten()
            })
            .collect();
        IdentityAgreementFindings::of(&extractions, captured_identity)
    };
    for (evaluation, findings) in page_evaluations.iter_mut().zip(findings) {
        evaluation.identity_agreement_findings = findings;
    }
}

/// Deterministic approval requires every page to be covered by some candidate, no
/// optional rule that matched nothing on every page its candidate applied to, and no
/// failed plausibility check. The generator had those exact pages in hand, so a total
/// miss is a defective selector, not page variance. A failed check, per page, across
/// pages or across the schemas of a page, shows a selector capturing something other than
/// the product's value, even where runtime selection would route the page to another
/// candidate, so the schemas stay in human review.
pub(crate) fn schema_matrix_deterministic_approval_ok(matrix: &SchemaMatrix) -> bool {
    schema_matrix_has_required_coverage(matrix)
        && optional_rules_missing_on_every_page(matrix).is_empty()
        && !matrix
            .candidates
            .iter()
            .flat_map(|candidate| &candidate.pages)
            .any(SchemaPageEvaluation::fails_plausibility_checks)
}

/// A review page on which a candidate's extraction failed a per-page, cross-page or
/// identity agreement check.
#[derive(Debug, serde::Serialize)]
pub(crate) struct ImplausiblePage<'a> {
    schema_index: usize,
    page_reference: &'a SchemaPageReference,
    url: &'a str,
    role: &'a str,
    plausibility_findings: &'a PlausibilityFindings,
    cross_page_findings: &'a CrossPageFindings,
    identity_agreement_findings: &'a IdentityAgreementFindings,
}

pub(crate) fn implausible_pages(matrix: &SchemaMatrix) -> Vec<ImplausiblePage<'_>> {
    matrix
        .candidates
        .iter()
        .flat_map(|candidate| {
            candidate
                .pages
                .iter()
                .filter(|page| page.fails_plausibility_checks())
                .map(|page| ImplausiblePage {
                    schema_index: candidate.schema_index,
                    page_reference: &page.page_reference,
                    url: &page.url,
                    role: &page.role,
                    plausibility_findings: &page.plausibility_findings,
                    cross_page_findings: &page.cross_page_findings,
                    identity_agreement_findings: &page.identity_agreement_findings,
                })
        })
        .collect()
}

/// An optional rule of one candidate that matched nothing (primary and additional
/// selectors) on every page the candidate applied to.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OptionalRuleTotalMiss {
    pub schema_index: usize,
    pub field: String,
    pub selector: String,
    pub page_count: usize,
}

pub(crate) fn optional_rules_missing_on_every_page(
    matrix: &SchemaMatrix,
) -> Vec<OptionalRuleTotalMiss> {
    let mut misses = Vec::new();
    for candidate in &matrix.candidates {
        let applied_pages: Vec<&SchemaPageEvaluation> = candidate
            .pages
            .iter()
            .filter(|page| page.apply_ok)
            .collect();
        let Some(first_page) = applied_pages.first() else {
            continue;
        };
        for field in &first_page.fields {
            if !selector_field_is_soft(field) {
                continue;
            }
            let missed_everywhere = applied_pages.iter().all(|page| {
                page.fields
                    .iter()
                    .find(|candidate_field| candidate_field.field == field.field)
                    .is_some_and(|candidate_field| {
                        selector_field_is_soft(candidate_field)
                            && selector_field_matched_nothing(candidate_field)
                    })
            });
            if missed_everywhere {
                misses.push(OptionalRuleTotalMiss {
                    schema_index: candidate.schema_index,
                    field: field.field.clone(),
                    selector: field.selector.clone(),
                    page_count: applied_pages.len(),
                });
            }
        }
    }
    misses
}

/// A rule whose miss leaves the field empty instead of failing the page. `images`
/// is excluded because its soft case (an existing but empty container) is decided by
/// the page-level apply result, not by match counts.
pub(crate) fn selector_field_is_soft(field: &SelectorFieldEvaluation) -> bool {
    !field.required && field.error.is_none() && field.field != "images"
}

/// Primary and every additional selector matched nothing; for a text rule, nothing with
/// visible text (see [`ExtractionRule::match_count`]).
pub(crate) fn selector_field_matched_nothing(field: &SelectorFieldEvaluation) -> bool {
    field.selector_match_count == Some(0)
        && field
            .additional_selector_match_counts
            .iter()
            .all(|count| *count == 0)
}

fn schema_matrix_has_required_coverage(matrix: &SchemaMatrix) -> bool {
    let Some(first_candidate) = matrix.candidates.first() else {
        return false;
    };
    if first_candidate.pages.is_empty() {
        return false;
    }

    first_candidate
        .pages
        .iter()
        .all(|target_page| schema_matrix_covers_page(matrix, &target_page.page_reference))
}

/// A page is covered when at least one candidate's extraction of it applies, so runtime
/// selection would capture it.
pub(crate) fn schema_matrix_covers_page(
    matrix: &SchemaMatrix,
    page_reference: &SchemaPageReference,
) -> bool {
    matrix.candidates.iter().any(|candidate| {
        candidate
            .pages
            .iter()
            .find(|page| page.page_reference == *page_reference)
            .is_some_and(page_has_required_extraction)
    })
}

pub(crate) fn unused_schema_indices(matrix: &SchemaMatrix) -> Vec<usize> {
    matrix
        .candidates
        .iter()
        .filter(|candidate| !candidate.pages.iter().any(page_has_required_extraction))
        .map(|candidate| candidate.schema_index)
        .collect()
}

fn evaluate_schema_page(
    schema: &ProductCssSelectorSchema,
    page: &MatrixPage<'_>,
    html: &Html,
) -> SchemaPageEvaluation {
    let extraction = schema.extract(html);
    let fields = evaluate_schema_fields(
        schema,
        html,
        skipped_image_elements(schema, html, &extraction),
    );

    // A page counts as applied only when runtime selection would accept the
    // extraction, so a rejecting plausibility finding fails the page like an
    // apply error; every finding stays listed for the reviewer.
    let (apply_ok, extracted, error, plausibility_findings) = match extraction {
        Ok(SchemaExtraction { raw: extracted, .. }) => {
            let findings = PlausibilityFindings::of(&extracted);
            let rejection = findings.rejection_reason();
            (rejection.is_none(), Some(extracted), rejection, findings)
        }
        Err(err) => (
            false,
            None,
            Some(err.to_string()),
            PlausibilityFindings::default(),
        ),
    };

    SchemaPageEvaluation {
        page_reference: page.page_reference.clone(),
        url: page.url.to_owned(),
        role: page.role.to_owned(),
        apply_ok,
        extracted,
        error,
        plausibility_findings,
        // Filled once every page is evaluated. Runtime selection never sees them, so
        // they leave `apply_ok` alone.
        cross_page_findings: CrossPageFindings::default(),
        // Filled once every candidate has evaluated the page, for the same reason.
        identity_agreement_findings: IdentityAgreementFindings::default(),
        fields,
    }
}

fn page_has_required_extraction(page: &SchemaPageEvaluation) -> bool {
    page.apply_ok && page.extracted.is_some()
}

/// The `images` elements the page's extraction skipped for having no value, or `None`
/// when the `images` rule fails. A page that fails on another rule may not reach its
/// `images` rule, so the rule is applied on its own for such a page.
fn skipped_image_elements(
    schema: &ProductCssSelectorSchema,
    html: &Html,
    extraction: &Result<SchemaExtraction, ApplySchemaError>,
) -> Option<usize> {
    match extraction {
        Ok(extraction) => Some(extraction.skipped_image_elements),
        Err(ApplySchemaError::Images(_)) => None,
        Err(_) => schema
            .apply_image_url_candidate_groups(html)
            .ok()
            .map(|images| images.skipped_elements),
    }
}

fn evaluate_schema_fields(
    schema: &ProductCssSelectorSchema,
    html: &Html,
    skipped_image_elements: Option<usize>,
) -> Vec<SelectorFieldEvaluation> {
    let mut fields = Vec::new();
    // `required` mirrors `ProductCssSelectorSchema::apply`: only `title` and `state`
    // fail the page on a miss.
    if let Some(rule) = &schema.source_listing_id {
        fields.push(evaluate_rule("source_listing_id", rule, html, false));
    }
    fields.push(evaluate_rule("title", &schema.title, html, true));
    if let Some(rule) = &schema.description {
        fields.push(evaluate_rule("description", rule, html, false));
    }
    if let Some(rule) = &schema.price {
        fields.push(evaluate_rule("price", rule, html, false));
    }
    if let Some(rule) = &schema.price_estimate_min {
        fields.push(evaluate_rule("price_estimate_min", rule, html, false));
    }
    if let Some(rule) = &schema.price_estimate_max {
        fields.push(evaluate_rule("price_estimate_max", rule, html, false));
    }
    fields.push(evaluate_rule("state", &schema.state, html, true));
    fields.push(evaluate_images_rule(schema, html, skipped_image_elements));
    for (field, rule) in &schema.raw_attributes {
        fields.push(evaluate_rule(field, rule, html, false));
    }
    fields
}

/// Evaluates the `images` rule as runtime extraction applies it, with the count of
/// matched elements it skipped for having no value ([`skipped_image_elements`]). A
/// failed images rule fails the page, whose error the page evaluation already carries.
fn evaluate_images_rule(
    schema: &ProductCssSelectorSchema,
    html: &Html,
    skipped_element_count: Option<usize>,
) -> SelectorFieldEvaluation {
    SelectorFieldEvaluation {
        skipped_element_count,
        ..evaluate_rule("images", &schema.images, html, false)
    }
}

fn evaluate_rule(
    field: &str,
    rule: &ExtractionRule,
    html: &Html,
    required: bool,
) -> SelectorFieldEvaluation {
    let selector = rule.selector.to_string();
    let parsed = match Selector::parse(&selector) {
        Ok(parsed) => parsed,
        Err(err) => {
            let error = format!("{err:?}");
            return SelectorFieldEvaluation {
                field: field.to_string(),
                selector: selector.clone(),
                selector_match_count: None,
                additional_selector_match_counts: Vec::new(),
                error: Some(error),
                required,
                skipped_element_count: None,
                empty_element_count: 0,
            };
        }
    };
    let additional_selectors: Vec<Option<Selector>> = rule
        .additional_selectors
        .iter()
        .map(|additional| Selector::parse(additional.as_ref()).ok())
        .collect();
    let selector_match_count = Some(rule.match_count(html, &parsed));
    let additional_selector_match_counts = additional_selectors
        .iter()
        .map(|additional| {
            additional
                .as_ref()
                .map_or(0, |parsed| rule.match_count(html, parsed))
        })
        .collect();
    let empty_element_count = std::iter::once(&parsed)
        .chain(additional_selectors.iter().flatten())
        .map(|parsed| rule.empty_element_count(html, parsed))
        .sum();

    SelectorFieldEvaluation {
        field: field.to_string(),
        selector,
        selector_match_count,
        additional_selector_match_counts,
        error: None,
        required,
        skipped_element_count: None,
        empty_element_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::{PAGE_ROLE_PRIMARY, PAGE_ROLE_SEED};
    use crate::scraper::css_selector::rule::{CssSelector, ExtractionCardinality, ExtractionKind};
    use crate::scraper::plausibility::{
        IdentityAgreementCheck, IdentityAgreementFinding, PlausibilityCheck, PlausibilityField,
        PlausibilityFinding,
    };
    use std::cell::Cell;

    fn text_rule(selector: &str) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: Vec::new(),
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        }
    }

    fn image_rule(selector: &str) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: Vec::new(),
            extract: ExtractionKind::Attribute { name: "src".into() },
            cardinality: ExtractionCardinality::All,
        }
    }

    fn schema(title_selector: &str) -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule(title_selector),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: image_rule("img"),
            raw_attributes: Default::default(),
        }
    }

    fn schema_with_price(title_selector: &str) -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            price: Some(text_rule("#price")),
            ..schema(title_selector)
        }
    }

    fn schema_with_raw_attributes(title_selector: &str) -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            raw_attributes: [
                ("rawShipment".to_string(), text_rule("#shipping")),
                ("rawCondition".to_string(), text_rule("#condition")),
                ("rawMaterial".to_string(), text_rule("#material")),
                ("rawYear".to_string(), text_rule("#year")),
                ("rawCategory".to_string(), text_rule("#category")),
                ("rawMeasurements".to_string(), text_rule("#measurements")),
                ("rawOrigin".to_string(), text_rule("#origin")),
            ]
            .into(),
            ..schema(title_selector)
        }
    }

    fn page(role: &str, title_tag: &str) -> SchemaReviewPageInput {
        SchemaReviewPageInput {
            url: format!("https://example.com/{role}"),
            role: role.to_string(),
            raw_html: format!(
                "<html><body><span id=\"product-id\">SKU</span><{title_tag}>Title</{title_tag}><span id=\"state\">Available</span><img src=\"a.jpg\"></body></html>"
            ),
        }
    }

    fn page_with_price(role: &str, title_tag: &str, price: Option<&str>) -> SchemaReviewPageInput {
        let price_html = price
            .map(|value| format!("<span id=\"price\">{value}</span>"))
            .unwrap_or_default();

        SchemaReviewPageInput {
            url: format!("https://example.com/{role}"),
            role: role.to_string(),
            raw_html: format!(
                "<html><body><span id=\"product-id\">SKU</span><{title_tag}>Title</{title_tag}>{price_html}<span id=\"state\">Available</span><img src=\"a.jpg\"></body></html>"
            ),
        }
    }

    thread_local! {
        /// How many pages this test thread parsed for matrix evaluation.
        pub(super) static PAGE_PARSES: Cell<usize> = const { Cell::new(0) };
    }

    /// Runs `evaluate` and returns its result with the number of pages it parsed.
    fn with_page_parses<T>(evaluate: impl FnOnce() -> T) -> (T, usize) {
        PAGE_PARSES.with(|parses| parses.set(0));
        let result = evaluate();
        (result, PAGE_PARSES.with(Cell::get))
    }

    fn live_review_page(page: &SchemaReviewPageInput) -> (CrawlerReviewPage, String) {
        let review_page = CrawlerReviewPage {
            review_page_id: crate::CrawlerReviewPageId::new(),
            review_id: CrawlerReviewId::new(),
            url: page.url.clone(),
            role: page.role.clone(),
            html_hash: String::new(),
            fetched: time::OffsetDateTime::now_utc(),
        };
        (review_page, page.raw_html.clone())
    }

    #[test]
    fn should_parse_each_page_once_for_every_candidate_and_removed_page_template() {
        // The third candidate reads the identity from another element of the primary page,
        // so the candidates that apply there disagree on it.
        let schemas = vec![
            schema("h1"),
            schema("h2"),
            ProductCssSelectorSchema {
                source_listing_id: Some(text_rule("#sku")),
                ..schema_with_price("h1")
            },
        ];
        let mut primary = page(PAGE_ROLE_PRIMARY, "h1");
        primary.raw_html = primary
            .raw_html
            .replace("<body>", "<body><span id=\"sku\">SKU-2</span>");
        let pages = vec![primary, page(PAGE_ROLE_SEED, "h2")];
        let live_pages: Vec<_> = pages.iter().map(live_review_page).collect();
        // The seed page's listing was captured under another identity.
        let captured = HashMap::from([(pages[1].url.clone(), "SKU-OLD".to_owned())]);
        // The manual candidate's removed-page template matches the state of both pages.
        let removed_page_templates = [RemovedPageSchema {
            selector: CssSelector::from("#state"),
            text: Some("available".to_owned()),
            regex: None,
        }];

        let (input_matrix, input_parses) =
            with_page_parses(|| evaluate_schema_matrix_for_inputs(&schemas, &pages));
        let (live_matrix, live_parses) = with_page_parses(|| {
            evaluate_schema_matrix_for_live_review_pages(
                CrawlerReviewId::new(),
                &schemas,
                &removed_page_templates,
                &live_pages,
                &captured,
            )
        });

        assert_eq!(input_parses, pages.len());
        assert_eq!(live_parses, pages.len());
        // The template is matched on the shared parse of each page: a candidate still
        // extracts the primary page's product, while the seed page's candidate extracts
        // another identity than its listing was captured under, so only the seed page reads
        // as removed.
        let removed_page_outcomes: Vec<_> = live_matrix
            .removed_page_matches
            .iter()
            .map(|matched| (matched.role.as_str(), matched.outcome))
            .collect();
        assert_eq!(
            removed_page_outcomes,
            vec![
                (
                    PAGE_ROLE_PRIMARY,
                    RemovedPageTemplateOutcome::ProductStillExtracts
                ),
                (PAGE_ROLE_SEED, RemovedPageTemplateOutcome::Removed),
            ]
        );
        let differs = |check, value: &str| {
            vec![IdentityAgreementFinding {
                field: PlausibilityField::SourceListingId,
                check,
                values: vec![value.to_owned()],
            }]
        };
        let differs_from_captured = differs(
            IdentityAgreementCheck::DiffersFromCapturedIdentity,
            "SKU-OLD",
        );
        for (matrix, seed_page_findings) in [
            (input_matrix, Vec::new()),
            (live_matrix, differs_from_captured),
        ] {
            // Every candidate applies its own selectors to the shared parse of each page,
            // and the identities they derive from it are compared, in schema and page order.
            let evaluated: Vec<_> = matrix
                .candidates
                .iter()
                .map(|candidate| {
                    let pages: Vec<_> = candidate
                        .pages
                        .iter()
                        .map(|page| {
                            let findings = page.identity_agreement_findings.as_slice().to_vec();
                            (page.apply_ok, findings)
                        })
                        .collect();
                    (candidate.schema_index, pages)
                })
                .collect();
            assert_eq!(
                evaluated,
                vec![
                    (
                        0,
                        vec![
                            (
                                true,
                                differs(IdentityAgreementCheck::DiffersFromOtherSchemas, "SKU-2")
                            ),
                            (false, Vec::new()),
                        ]
                    ),
                    (1, vec![(false, Vec::new()), (true, seed_page_findings)]),
                    (
                        2,
                        vec![
                            (
                                true,
                                differs(IdentityAgreementCheck::DiffersFromOtherSchemas, "SKU")
                            ),
                            (false, Vec::new()),
                        ]
                    ),
                ]
            );
        }
    }

    mod removed_page_templates {
        use super::*;

        const NOTICE_HTML: &str = "<html><body><main><p class=\"notice\">This item is no longer available</p>\
            <p class=\"notice\">Please browse our catalogue</p></main></body></html>";

        fn notice_template(selector: &str, text: &str) -> RemovedPageSchema {
            RemovedPageSchema {
                selector: CssSelector::from(selector),
                text: Some(text.to_owned()),
                regex: None,
            }
        }

        /// The removed-page matches of the matrix evaluated on `pages`.
        fn removed_page_matches(
            templates: &[RemovedPageSchema],
            schemas: &[ProductCssSelectorSchema],
            pages: &[MatrixPage<'_>],
        ) -> Vec<RemovedPageTemplateMatch> {
            evaluate_matrix(None, schemas, templates, pages).removed_page_matches
        }

        fn matrix_page<'a>(role: &'a str, url: &'a str, raw_html: &'a str) -> MatrixPage<'a> {
            MatrixPage {
                page_reference: SchemaPageReference::Input { page_index: 0 },
                url,
                role,
                raw_html,
                captured_identity: None,
            }
        }

        #[test]
        fn should_show_the_notice_page_as_removed_and_leave_unmatched_pages_out() {
            let product = page(PAGE_ROLE_SEED, "h1");
            let templates = [
                notice_template("footer", "never shown"),
                notice_template(".notice", "no longer available"),
            ];

            let matches = removed_page_matches(
                &templates,
                &[schema("h1")],
                &[
                    matrix_page(PAGE_ROLE_SEED, &product.url, &product.raw_html),
                    matrix_page("WAITING_PAGE", "https://example.com/sold", NOTICE_HTML),
                ],
            );

            assert_eq!(
                matches,
                vec![RemovedPageTemplateMatch {
                    page_reference: SchemaPageReference::Input { page_index: 0 },
                    url: "https://example.com/sold".to_owned(),
                    role: "WAITING_PAGE".to_owned(),
                    outcome: RemovedPageTemplateOutcome::Removed,
                    templates: vec![MatchedRemovedPageTemplate {
                        template_index: 1,
                        notices: vec!["this item is no longer available".to_owned()],
                    }],
                }]
            );
        }

        /// An over-broad template matching a page a candidate still extracts is shown with the
        /// scraper's precedence: the product wins.
        #[test]
        fn should_show_a_page_a_candidate_still_extracts_as_a_product() {
            let product = page(PAGE_ROLE_SEED, "h1");

            let matches = removed_page_matches(
                &[notice_template("body", "available")],
                &[schema("h1")],
                &[matrix_page(PAGE_ROLE_SEED, &product.url, &product.raw_html)],
            );

            assert_eq!(matches.len(), 1);
            assert_eq!(
                matches[0].outcome,
                RemovedPageTemplateOutcome::ProductStillExtracts
            );
        }

        #[test]
        fn should_list_no_page_without_templates() {
            let matches = removed_page_matches(
                &[],
                &[schema("h1")],
                &[matrix_page(
                    "WAITING_PAGE",
                    "https://example.com/sold",
                    NOTICE_HTML,
                )],
            );

            assert!(matches.is_empty());
        }
    }

    #[test]
    fn should_require_all_pages_to_be_covered_by_some_schema() {
        let schemas = vec![schema("h1"), schema("h2")];
        let pages = vec![page(PAGE_ROLE_PRIMARY, "h1"), page(PAGE_ROLE_SEED, "h2")];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

        assert!(schema_matrix_has_required_coverage(&matrix));
    }

    #[test]
    fn should_reject_matrix_when_a_page_has_no_applicable_schema() {
        let schemas = vec![schema("h1")];
        let pages = vec![page(PAGE_ROLE_PRIMARY, "h1"), page(PAGE_ROLE_SEED, "h2")];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

        assert!(!schema_matrix_has_required_coverage(&matrix));
    }

    #[test]
    fn should_cover_page_when_non_null_optional_rule_is_absent() {
        let schemas = vec![schema_with_price("h1")];
        let pages = vec![page_with_price(PAGE_ROLE_PRIMARY, "h1", None)];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);
        let page = &matrix.candidates[0].pages[0];

        assert!(schema_matrix_has_required_coverage(&matrix));
        assert!(page.apply_ok);
        assert!(
            page.extracted
                .as_ref()
                .is_some_and(|raw| raw.price.is_none())
        );
        // The rule missed on every page the candidate applied to, so the schema is
        // covered but not deterministically approvable.
        assert!(!schema_matrix_deterministic_approval_ok(&matrix));
        assert_eq!(
            optional_rules_missing_on_every_page(&matrix),
            vec![OptionalRuleTotalMiss {
                schema_index: 0,
                field: "price".to_string(),
                selector: "#price".to_string(),
                page_count: 1,
            }]
        );
        let price_field = page
            .fields
            .iter()
            .find(|field| field.field == "price")
            .unwrap_or_else(|| panic!("price field must stay visible in the matrix"));
        assert_eq!(price_field.selector_match_count, Some(0));
    }

    /// A text rule takes no usable value from an element without visible text, so the
    /// matrix does not count such an element as a match. Extraction is unchanged.
    mod blank_text_matches {
        use super::*;

        fn product_page(price_html: &str) -> SchemaReviewPageInput {
            SchemaReviewPageInput {
                url: "https://example.com/product".to_string(),
                role: PAGE_ROLE_PRIMARY.to_string(),
                raw_html: format!(
                    "<html><body><span id=\"product-id\">SKU</span><h1>Title</h1>{price_html}<span id=\"state\">Available</span><img src=\"a.jpg\"></body></html>"
                ),
            }
        }

        fn matrix_for_price(price: ExtractionRule, price_html: &str) -> SchemaMatrix {
            let schema = ProductCssSelectorSchema {
                price: Some(price),
                ..schema("h1")
            };
            evaluate_schema_matrix_for_inputs(&[schema], &[product_page(price_html)])
        }

        fn price_field(matrix: &SchemaMatrix) -> &SelectorFieldEvaluation {
            matrix.candidates[0].pages[0]
                .fields
                .iter()
                .find(|field| field.field == "price")
                .unwrap_or_else(|| panic!("price field must stay visible in the matrix"))
        }

        fn extracted_price(matrix: &SchemaMatrix) -> Option<&str> {
            matrix.candidates[0].pages[0]
                .extracted
                .as_ref()
                .and_then(|raw| raw.price.as_deref())
        }

        fn missing_on_the_page(field: &str, selector: &str) -> OptionalRuleTotalMiss {
            OptionalRuleTotalMiss {
                schema_index: 0,
                field: field.to_string(),
                selector: selector.to_string(),
                page_count: 1,
            }
        }

        #[test]
        fn should_count_a_price_rule_as_missing_when_its_only_match_is_empty() {
            let matrix = matrix_for_price(text_rule(".price"), "<span class=\"price\"></span>");

            assert_eq!(price_field(&matrix).selector_match_count, Some(0));
            assert_eq!(
                optional_rules_missing_on_every_page(&matrix),
                vec![missing_on_the_page("price", ".price")]
            );
            assert!(!schema_matrix_deterministic_approval_ok(&matrix));
            assert_eq!(extracted_price(&matrix), None);
        }

        /// Both leave the price empty; the empty-element count tells the reviewer which.
        #[test]
        fn should_tell_a_price_rule_matching_only_empty_elements_from_one_matching_nothing() {
            let empty = matrix_for_price(text_rule(".price"), "<span class=\"price\"> </span>");
            let unmatched = matrix_for_price(text_rule(".price"), "");

            assert_eq!(price_field(&empty).selector_match_count, Some(0));
            assert_eq!(price_field(&empty).empty_element_count, 1);
            assert_eq!(price_field(&unmatched).selector_match_count, Some(0));
            assert_eq!(price_field(&unmatched).empty_element_count, 0);
        }

        #[test]
        fn should_count_a_price_rule_as_missing_when_its_only_match_is_whitespace() {
            let matrix =
                matrix_for_price(text_rule(".price"), "<p class=\"price\">\n\t&nbsp; </p>");

            assert_eq!(price_field(&matrix).selector_match_count, Some(0));
            assert_eq!(
                optional_rules_missing_on_every_page(&matrix),
                vec![missing_on_the_page("price", ".price")]
            );
            assert!(!schema_matrix_deterministic_approval_ok(&matrix));
            assert_eq!(extracted_price(&matrix), None);
        }

        #[test]
        fn should_count_a_price_rule_as_matched_when_a_blank_match_precedes_a_non_empty_one() {
            let matrix = matrix_for_price(
                text_rule(".price"),
                "<p class=\"price\"> </p><p class=\"price\">£450</p>",
            );

            assert_eq!(price_field(&matrix).selector_match_count, Some(1));
            assert!(optional_rules_missing_on_every_page(&matrix).is_empty());
            assert!(schema_matrix_deterministic_approval_ok(&matrix));
            assert_eq!(extracted_price(&matrix), Some("£450"));
        }

        #[test]
        fn should_count_a_price_rule_as_missing_when_its_additional_selector_matches_only_blank() {
            let price = ExtractionRule {
                additional_selectors: vec![CssSelector::from(".sale-price")],
                ..text_rule(".price")
            };

            let matrix = matrix_for_price(price, "<span class=\"sale-price\"> </span>");

            assert_eq!(price_field(&matrix).additional_selector_match_counts, [0]);
            assert_eq!(price_field(&matrix).empty_element_count, 1);
            assert_eq!(
                optional_rules_missing_on_every_page(&matrix),
                vec![missing_on_the_page("price", ".price")]
            );
        }

        #[test]
        fn should_count_a_price_rule_as_matched_when_its_additional_selector_has_text() {
            let price = ExtractionRule {
                additional_selectors: vec![CssSelector::from(".sale-price")],
                ..text_rule(".price")
            };

            let matrix = matrix_for_price(
                price,
                "<span class=\"price\"> </span><span class=\"sale-price\">£450</span>",
            );

            let field = price_field(&matrix);
            assert_eq!(field.selector_match_count, Some(0));
            assert_eq!(field.additional_selector_match_counts, [1]);
            assert!(optional_rules_missing_on_every_page(&matrix).is_empty());
            assert!(schema_matrix_deterministic_approval_ok(&matrix));
            assert_eq!(extracted_price(&matrix), Some("£450"));
        }

        #[test]
        fn should_count_estimate_rules_as_missing_when_their_only_matches_are_blank() {
            let schema = ProductCssSelectorSchema {
                price_estimate_min: Some(text_rule("#estimate-min")),
                price_estimate_max: Some(text_rule("#estimate-max")),
                ..schema("h1")
            };
            let page =
                product_page("<span id=\"estimate-min\"></span><span id=\"estimate-max\"> </span>");

            let matrix = evaluate_schema_matrix_for_inputs(&[schema], &[page]);

            assert_eq!(
                optional_rules_missing_on_every_page(&matrix),
                vec![
                    missing_on_the_page("price_estimate_min", "#estimate-min"),
                    missing_on_the_page("price_estimate_max", "#estimate-max"),
                ]
            );
            assert!(!schema_matrix_deterministic_approval_ok(&matrix));
        }

        /// An attribute rule extracts the blank value, which the `BLANK` check reports.
        #[test]
        fn should_leave_a_blank_price_attribute_to_the_blank_check() {
            let price = ExtractionRule {
                selector: CssSelector::from("meta[itemprop='price']"),
                additional_selectors: Vec::new(),
                extract: ExtractionKind::Attribute {
                    name: "content".into(),
                },
                cardinality: ExtractionCardinality::First,
            };

            let matrix = matrix_for_price(price, "<meta itemprop=\"price\" content=\"\">");

            assert_eq!(price_field(&matrix).selector_match_count, Some(1));
            assert!(optional_rules_missing_on_every_page(&matrix).is_empty());
            assert_eq!(
                matrix.candidates[0].pages[0]
                    .plausibility_findings
                    .as_slice(),
                [finding(PlausibilityField::Price, PlausibilityCheck::Blank)]
            );
            assert!(!schema_matrix_deterministic_approval_ok(&matrix));
        }

        /// Every text rule counts the same way, not only the prices.
        #[test]
        fn should_count_other_text_rules_as_missing_when_their_only_matches_are_blank() {
            let schema = ProductCssSelectorSchema {
                description: Some(ExtractionRule {
                    cardinality: ExtractionCardinality::All,
                    ..text_rule(".description")
                }),
                raw_attributes: [("rawMeasurements".to_string(), text_rule("#measurements"))]
                    .into(),
                ..schema("h1")
            };
            let page = product_page(
                "<div class=\"description\"> </div><div class=\"description\"><p></p></div><span id=\"measurements\">\n</span>",
            );

            let matrix = evaluate_schema_matrix_for_inputs(&[schema], &[page]);

            assert_eq!(
                optional_rules_missing_on_every_page(&matrix),
                vec![
                    missing_on_the_page("description", ".description"),
                    missing_on_the_page("rawMeasurements", "#measurements"),
                ]
            );
            assert!(!schema_matrix_deterministic_approval_ok(&matrix));
        }

        #[test]
        fn should_count_a_title_rule_without_its_blank_matches() {
            let page = SchemaReviewPageInput {
                url: "https://example.com/product".to_string(),
                role: PAGE_ROLE_PRIMARY.to_string(),
                raw_html: "<html><body><span id=\"product-id\">SKU</span><h1> </h1><h1>Title</h1><span id=\"state\">Available</span><img src=\"a.jpg\"></body></html>".to_string(),
            };

            let matrix = evaluate_schema_matrix_for_inputs(&[schema("h1")], &[page]);
            let title = matrix.candidates[0].pages[0]
                .fields
                .iter()
                .find(|field| field.field == "title")
                .unwrap_or_else(|| panic!("title field must be in the matrix"));

            assert_eq!(title.selector_match_count, Some(1));
            assert!(matrix.candidates[0].pages[0].apply_ok);
        }
    }

    #[test]
    fn should_cover_mixed_templates_with_split_schemas_for_price_presence() {
        let schemas = vec![schema_with_price("h1"), schema("h2")];
        let pages = vec![
            page_with_price(PAGE_ROLE_PRIMARY, "h1", Some("10 EUR")),
            page_with_price(PAGE_ROLE_SEED, "h2", None),
        ];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

        assert!(schema_matrix_has_required_coverage(&matrix));
    }

    #[test]
    fn should_cover_product_page_when_images_are_absent() {
        let schemas = vec![ProductCssSelectorSchema {
            images: image_rule("#gallery img"),
            ..schema("h1")
        }];
        let pages = vec![SchemaReviewPageInput {
            url: "https://example.com/no-image-product".to_string(),
            role: PAGE_ROLE_PRIMARY.to_string(),
            raw_html: "<html><body><span id=\"product-id\">SKU</span><h1>Title</h1><span id=\"state\">Available</span><div id=\"gallery\"></div></body></html>".to_string(),
        }];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

        assert!(schema_matrix_has_required_coverage(&matrix));
        assert!(
            matrix.candidates[0].pages[0]
                .extracted
                .as_ref()
                .is_some_and(|raw| raw.images.is_empty())
        );
    }

    #[test]
    fn should_reject_candidate_when_price_and_state_contain_css_rules() {
        let schemas = vec![ProductCssSelectorSchema {
            price: Some(text_rule("#notice")),
            state: text_rule("#notice"),
            ..schema("h1")
        }];
        let pages = vec![SchemaReviewPageInput {
            url: "https://example.com/product".to_string(),
            role: PAGE_ROLE_PRIMARY.to_string(),
            raw_html: "<html><body><span id=\"product-id\">SKU</span><h1>Title</h1><div id=\"notice\">Price on application .notice { color: red; } .x { display: none; }</div><img src=\"a.jpg\"></body></html>".to_string(),
        }];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);
        let page = &matrix.candidates[0].pages[0];

        assert!(!schema_matrix_has_required_coverage(&matrix));
        assert!(!page.apply_ok);
        assert_eq!(
            page.error.as_deref(),
            Some(
                "implausible extraction: `price` contains CSS or script tokens; `state` contains CSS or script tokens"
            )
        );
        assert!(page.extracted.is_some());
        assert_eq!(
            page.plausibility_findings.as_slice(),
            [
                finding(PlausibilityField::Price, PlausibilityCheck::MarkupTokens),
                finding(PlausibilityField::State, PlausibilityCheck::MarkupTokens),
            ]
        );
    }

    #[test]
    fn should_reject_candidate_when_title_contains_script_source() {
        let schemas = vec![schema("h1")];
        let pages = vec![SchemaReviewPageInput {
            url: "https://example.com/product".to_string(),
            role: PAGE_ROLE_PRIMARY.to_string(),
            raw_html: "<html><body><span id=\"product-id\">SKU</span><h1>Title function(){ track() }</h1><span id=\"state\">Available</span><img src=\"a.jpg\"></body></html>".to_string(),
        }];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);
        let page = &matrix.candidates[0].pages[0];

        assert!(!schema_matrix_has_required_coverage(&matrix));
        assert_eq!(
            page.error.as_deref(),
            Some("implausible extraction: `title` contains CSS or script tokens")
        );
    }

    /// A title element the page leaves empty shows as a blank title next to the rest of
    /// the extraction; a title selector that matches nothing shows as an apply error.
    #[test]
    fn should_tell_an_empty_title_element_from_an_unmatched_title_selector() {
        let pages = vec![
            page(PAGE_ROLE_PRIMARY, "h2"),
            SchemaReviewPageInput {
                url: "https://example.com/untitled".to_string(),
                role: PAGE_ROLE_SEED.to_string(),
                raw_html: "<html><body><span id=\"product-id\">SKU</span><h1 itemprop=\"name\"></h1><span id=\"state\">Available</span><img src=\"a.jpg\"></body></html>".to_string(),
            },
        ];

        let matrix = evaluate_schema_matrix_for_inputs(&[schema("h1")], &pages);

        let [unmatched, empty] = matrix.candidates[0].pages.as_slice() else {
            panic!("expected one evaluation per page");
        };
        assert_eq!(
            unmatched.error.as_deref(),
            Some(
                "failed to extract `title`: no element matched selector `h1` (or any additional selectors)"
            )
        );
        assert!(unmatched.extracted.is_none());
        assert!(!empty.apply_ok);
        assert_eq!(
            empty.error.as_deref(),
            Some("implausible extraction: `title` is blank")
        );
        assert_eq!(
            empty.plausibility_findings.as_slice(),
            [finding(PlausibilityField::Title, PlausibilityCheck::Blank)]
        );
        assert_eq!(
            empty.extracted.as_ref().map(|raw| raw.state.as_str()),
            Some("Available")
        );
        let title_empty_elements = |page: &SchemaPageEvaluation| {
            page.fields
                .iter()
                .find(|field| field.field == "title")
                .map(|field| (field.selector_match_count, field.empty_element_count))
        };
        assert_eq!(title_empty_elements(unmatched), Some((Some(0), 0)));
        assert_eq!(title_empty_elements(empty), Some((Some(0), 1)));
    }

    #[test]
    fn should_accept_candidate_when_inline_style_sits_next_to_visible_text() {
        let schemas = vec![ProductCssSelectorSchema {
            price: Some(text_rule("#notice")),
            state: text_rule("#notice"),
            ..schema("h1")
        }];
        let pages = vec![SchemaReviewPageInput {
            url: "https://example.com/product".to_string(),
            role: PAGE_ROLE_PRIMARY.to_string(),
            raw_html: "<html><body><span id=\"product-id\">SKU</span><h1>Title</h1><div id=\"notice\"><style>.notice { color: red; }</style>Price on application</div><img src=\"a.jpg\"></body></html>".to_string(),
        }];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);
        let page = &matrix.candidates[0].pages[0];

        assert!(schema_matrix_has_required_coverage(&matrix));
        assert_eq!(
            page.extracted.as_ref().map(|raw| raw.state.as_str()),
            Some("Price on application")
        );
    }

    #[test]
    fn should_accept_candidate_when_prose_mentions_window_or_document() {
        let schemas = vec![schema("h1")];
        let pages = vec![SchemaReviewPageInput {
            url: "https://example.com/product".to_string(),
            role: PAGE_ROLE_PRIMARY.to_string(),
            raw_html: "<html><body><span id=\"product-id\">SKU</span><h1>Georgian sash window. Original glass {rare}</h1><span id=\"state\">Sold - see signed document.</span><img src=\"a.jpg\"></body></html>".to_string(),
        }];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);
        let page = &matrix.candidates[0].pages[0];

        assert!(schema_matrix_has_required_coverage(&matrix));
        assert!(page.apply_ok);
        assert_eq!(page.error, None);
    }

    #[test]
    fn should_reject_candidate_when_price_contains_single_declaration_css_rule() {
        let schemas = vec![ProductCssSelectorSchema {
            price: Some(text_rule("#notice")),
            ..schema("h1")
        }];
        let pages = vec![SchemaReviewPageInput {
            url: "https://example.com/product".to_string(),
            role: PAGE_ROLE_PRIMARY.to_string(),
            raw_html: "<html><body><span id=\"product-id\">SKU</span><h1>Title</h1><div id=\"notice\">POA .x{display:none}</div><span id=\"state\">Available</span><img src=\"a.jpg\"></body></html>".to_string(),
        }];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);
        let page = &matrix.candidates[0].pages[0];

        assert!(!page.apply_ok);
        assert_eq!(
            page.error.as_deref(),
            Some("implausible extraction: `price` contains CSS or script tokens")
        );
    }

    fn finding(field: PlausibilityField, check: PlausibilityCheck) -> PlausibilityFinding {
        PlausibilityFinding { field, check }
    }

    #[test]
    fn should_report_the_same_per_page_checks_for_every_candidate_on_every_page() {
        let schemas = vec![schema_with_price("h1"), schema_with_price("h2")];
        let sold_page = SchemaReviewPageInput {
            url: "https://example.com/sold".to_string(),
            role: PAGE_ROLE_SEED.to_string(),
            raw_html: "<html><body><h1>Title</h1><h2>Title</h2><span id=\"price\">£0.00</span><span id=\"state\">Sold</span><img src=\"a.jpg\"><img src=\"a.jpg\"></body></html>".to_string(),
        };
        let pages = vec![
            page_with_price(PAGE_ROLE_PRIMARY, "h1", Some("£450")),
            sold_page,
        ];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

        let findings_of = |candidate: usize, page: usize| {
            matrix.candidates[candidate].pages[page]
                .plausibility_findings
                .as_slice()
                .to_vec()
        };
        assert_eq!(findings_of(0, 0), vec![]);
        let sold_page_findings = vec![
            finding(PlausibilityField::SourceListingId, PlausibilityCheck::Blank),
            finding(PlausibilityField::Price, PlausibilityCheck::AllZeroDigits),
            finding(
                PlausibilityField::Images,
                PlausibilityCheck::DuplicateValues,
            ),
        ];
        assert_eq!(findings_of(0, 1), sold_page_findings);
        assert_eq!(findings_of(1, 1), sold_page_findings);
        assert!(
            matrix.candidates[0].pages[1].apply_ok,
            "findings that only lower the rank keep the page covered"
        );
        assert!(schema_matrix_has_required_coverage(&matrix));
    }

    #[test]
    fn should_fail_page_when_state_is_longer_than_a_label() {
        let schemas = vec![ProductCssSelectorSchema {
            state: text_rule("#details"),
            ..schema("h1")
        }];
        let pages = vec![SchemaReviewPageInput {
            url: "https://example.com/product".to_string(),
            role: PAGE_ROLE_PRIMARY.to_string(),
            raw_html: format!(
                "<html><body><span id=\"product-id\">SKU</span><h1>Title</h1><div id=\"details\">{}</div><img src=\"a.jpg\"></body></html>",
                "Mahogany chest of drawers with original brass handles. ".repeat(3)
            ),
        }];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);
        let page = &matrix.candidates[0].pages[0];

        assert!(!page.apply_ok);
        assert!(!schema_matrix_has_required_coverage(&matrix));
        assert_eq!(
            page.plausibility_findings.as_slice(),
            [finding(
                PlausibilityField::State,
                PlausibilityCheck::TooLong
            )]
        );
        assert_eq!(
            page.error.as_deref(),
            Some("implausible extraction: `state` is longer than a label")
        );
    }

    #[test]
    fn should_report_unused_schema_without_failing_page_coverage() {
        let schemas = vec![schema("h1"), schema("h3")];
        let pages = vec![page(PAGE_ROLE_PRIMARY, "h1")];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

        assert!(schema_matrix_has_required_coverage(&matrix));
        assert_eq!(unused_schema_indices(&matrix), vec![1]);
    }

    /// Any failed plausibility check, per page or across pages, keeps generated
    /// schemas out of deterministic approval.
    mod plausibility_gate {
        use super::*;
        use crate::scraper::plausibility::{CrossPageCheck, CrossPageFinding};

        /// The page of a distinct product: its own identity, title and image, below a
        /// document `<title>` that only names the shop's category.
        fn product_page(path: &str, extra_html: &str) -> SchemaReviewPageInput {
            SchemaReviewPageInput {
                url: format!("https://shop.example/products/{path}"),
                role: PAGE_ROLE_SEED.to_string(),
                raw_html: format!(
                    "<html><head><title>Antiques - Shop</title></head><body><span id=\"product-id\">{path}</span><h1>Product {path}</h1><span id=\"state\">Available</span><img src=\"/images/{path}.jpg\">{extra_html}</body></html>"
                ),
            }
        }

        fn cross_page_findings_of(
            matrix: &SchemaMatrix,
            candidate: usize,
        ) -> Vec<Vec<CrossPageFinding>> {
            matrix.candidates[candidate]
                .pages
                .iter()
                .map(|page| page.cross_page_findings.as_slice().to_vec())
                .collect()
        }

        fn repeated(field: PlausibilityField, value: &str) -> CrossPageFinding {
            CrossPageFinding {
                field,
                check: CrossPageCheck::RepeatedAcrossPages,
                values: vec![value.to_string()],
            }
        }

        #[test]
        fn should_approve_deterministically_when_distinct_products_pass_every_check() {
            let pages = vec![product_page("chair", ""), product_page("table", "")];

            let matrix = evaluate_schema_matrix_for_inputs(&[schema("h1")], &pages);

            assert!(implausible_pages(&matrix).is_empty());
            assert!(schema_matrix_deterministic_approval_ok(&matrix));
        }

        #[test]
        fn should_report_a_document_title_shared_by_every_page_for_its_candidate_only() {
            let schemas = vec![schema("h1"), schema("title")];
            let pages = vec![product_page("chair", ""), product_page("table", "")];

            let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

            assert_eq!(cross_page_findings_of(&matrix, 0), vec![vec![], vec![]]);
            let shared_title = vec![repeated(PlausibilityField::Title, "Antiques - Shop")];
            assert_eq!(
                cross_page_findings_of(&matrix, 1),
                vec![shared_title.clone(), shared_title]
            );
            assert!(
                matrix.candidates[1].pages.iter().all(|page| page.apply_ok),
                "runtime selection cannot see a cross-page finding, so the page stays applied"
            );
            assert!(schema_matrix_has_required_coverage(&matrix));
            assert!(!schema_matrix_deterministic_approval_ok(&matrix));
        }

        #[test]
        fn should_compare_only_pages_the_candidate_extracted() {
            let schemas = vec![ProductCssSelectorSchema {
                images: ExtractionRule {
                    selector: CssSelector::from("meta[property='og:image']"),
                    additional_selectors: Vec::new(),
                    extract: ExtractionKind::ImageUrl,
                    cardinality: ExtractionCardinality::All,
                },
                ..schema("h1")
            }];
            let share_image =
                "<meta property=\"og:image\" content=\"https://shop.example/share.jpg\">";
            let mut without_state = product_page("lamp", share_image);
            without_state.raw_html = without_state
                .raw_html
                .replace("<span id=\"state\">Available</span>", "");
            let pages = vec![
                product_page("ewer", share_image),
                product_page("saint", share_image),
                without_state,
            ];

            let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

            let shared_image = vec![repeated(
                PlausibilityField::Images,
                "https://shop.example/share.jpg",
            )];
            assert_eq!(
                cross_page_findings_of(&matrix, 0),
                vec![shared_image.clone(), shared_image, vec![]]
            );
            assert!(matrix.candidates[0].pages[2].extracted.is_none());
        }

        #[test]
        fn should_block_deterministic_approval_on_a_rank_lowering_finding_of_a_covered_page() {
            let schemas = vec![schema_with_price("h1")];
            let pages = vec![
                product_page("chair", "<span id=\"price\">£450</span>"),
                product_page("sold-table", "<span id=\"price\">£0.00</span>"),
            ];

            let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

            assert!(schema_matrix_has_required_coverage(&matrix));
            assert_eq!(
                matrix.candidates[0].pages[1]
                    .plausibility_findings
                    .as_slice(),
                [finding(
                    PlausibilityField::Price,
                    PlausibilityCheck::AllZeroDigits
                )]
            );
            assert!(!schema_matrix_deterministic_approval_ok(&matrix));
        }

        /// The capture falls back to the URL hash, which changes when the product moves.
        #[test]
        fn should_block_deterministic_approval_when_the_schema_has_no_identity_rule() {
            let schemas = vec![ProductCssSelectorSchema {
                source_listing_id: None,
                ..schema("h1")
            }];
            let pages = vec![product_page("chair", ""), product_page("table", "")];

            let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

            assert!(schema_matrix_has_required_coverage(&matrix));
            assert!(matrix.candidates[0].pages.iter().all(|page| {
                page.plausibility_findings.as_slice()
                    == [finding(
                        PlausibilityField::SourceListingId,
                        PlausibilityCheck::Blank,
                    )]
            }));
            assert!(!schema_matrix_deterministic_approval_ok(&matrix));
        }

        /// Vauclair's `<body class>` carries the post id and the product's tags, so it
        /// differs per page and only its shape shows that it is not an identifier.
        #[test]
        fn should_block_deterministic_approval_on_a_body_class_identity() {
            let schemas = vec![ProductCssSelectorSchema {
                source_listing_id: Some(ExtractionRule {
                    selector: CssSelector::from("body"),
                    additional_selectors: Vec::new(),
                    extract: ExtractionKind::Attribute {
                        name: "class".into(),
                    },
                    cardinality: ExtractionCardinality::First,
                }),
                ..schema("h1")
            }];
            let pages: Vec<SchemaReviewPageInput> = [("chair", 13803), ("table", 13892)]
                .into_iter()
                .map(|(path, post_id)| {
                    let mut page = product_page(path, "");
                    page.raw_html = page.raw_html.replace(
                        "<body>",
                        &format!(
                            "<body class=\"portfolio-template-default single single-portfolio postid-{post_id} post_tag-{path}\">"
                        ),
                    );
                    page
                })
                .collect();

            let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

            assert_eq!(cross_page_findings_of(&matrix, 0), vec![vec![], vec![]]);
            assert!(matrix.candidates[0].pages.iter().all(|page| {
                page.apply_ok
                    && page.plausibility_findings.as_slice()
                        == [finding(
                            PlausibilityField::SourceListingId,
                            PlausibilityCheck::WordList,
                        )]
            }));
            assert!(!schema_matrix_deterministic_approval_ok(&matrix));
        }

        #[test]
        fn should_block_deterministic_approval_when_another_candidate_covers_a_rejected_page() {
            let schemas = vec![
                ProductCssSelectorSchema {
                    state: text_rule("#details"),
                    ..schema("h1")
                },
                schema("h1"),
            ];
            let details = format!(
                "<div id=\"details\">{}</div>",
                "Mahogany chest of drawers with original brass handles. ".repeat(3)
            );
            let pages = vec![
                product_page("chest", &details),
                product_page("table", &details),
            ];

            let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

            assert!(matrix.candidates[0].pages.iter().all(|page| !page.apply_ok));
            assert!(schema_matrix_has_required_coverage(&matrix));
            assert!(!schema_matrix_deterministic_approval_ok(&matrix));
        }
    }

    #[test]
    fn should_preserve_image_candidate_groups_in_review_extraction() {
        let schema = ProductCssSelectorSchema {
            images: ExtractionRule {
                selector: CssSelector::from("img"),
                additional_selectors: Vec::new(),
                extract: ExtractionKind::ImageUrl,
                cardinality: ExtractionCardinality::All,
            },
            ..schema("h1")
        };
        let pages = vec![SchemaReviewPageInput {
            url: "https://example.com/product".to_string(),
            role: PAGE_ROLE_PRIMARY.to_string(),
            raw_html: "<html><body><span id=\"product-id\">SKU</span><h1>Title</h1><span id=\"state\">Available</span><img data-large_image=\"/images/primary.jpg\" src=\"/images/fallback-800x600.jpg\"><img data-large_image=\"/images/primary.jpg\" src=\"/images/fallback-800x600.jpg\"></body></html>".to_string(),
        }];

        let matrix = evaluate_schema_matrix_for_inputs(&[schema], &pages);

        assert_eq!(
            matrix.candidates[0].pages[0]
                .extracted
                .as_ref()
                .map(|raw| raw.images.clone()),
            Some(vec![
                "/images/primary.jpg\u{1f}/images/fallback-800x600.jpg".to_string(),
                "/images/primary.jpg\u{1f}/images/fallback-800x600.jpg".to_string(),
            ])
        );
    }

    /// Hingstons Antiques: the add-to-cart button carries the identity on an active page
    /// and disappears when the product sells, so a schema written for the sold page reads
    /// the identity from another element.
    mod identity_agreement {
        use super::*;
        use crate::review::model::CrawlerReviewPage;
        use crate::scraper::plausibility::{IdentityAgreementCheck, IdentityAgreementFinding};

        const ACTIVE_URL: &str = "https://shop.example/product/chair";
        const SOLD_URL: &str = "https://shop.example/product/sold-chair";

        fn attribute_rule(selector: &str, name: &str) -> ExtractionRule {
            ExtractionRule {
                selector: CssSelector::from(selector),
                additional_selectors: Vec::new(),
                extract: ExtractionKind::Attribute { name: name.into() },
                cardinality: ExtractionCardinality::First,
            }
        }

        fn identity_schema(identity: ExtractionRule) -> ProductCssSelectorSchema {
            ProductCssSelectorSchema {
                source_listing_id: Some(identity),
                state: text_rule(".stock"),
                ..schema("h1")
            }
        }

        /// The active-template schema: its identity and its state are the add-to-cart
        /// button, so it does not apply to a sold page.
        fn button_schema() -> ProductCssSelectorSchema {
            ProductCssSelectorSchema {
                state: text_rule("button.single_add_to_cart_button"),
                ..identity_schema(attribute_rule("button.single_add_to_cart_button", "value"))
            }
        }

        fn container_schema() -> ProductCssSelectorSchema {
            identity_schema(attribute_rule("div.product", "id"))
        }

        fn product_html(post_id: u32, sold: bool) -> String {
            let (stock, button) = if sold {
                ("Sold".to_owned(), String::new())
            } else {
                (
                    "In stock".to_owned(),
                    format!(
                        "<button class=\"single_add_to_cart_button\" value=\"{post_id}\">Add</button>"
                    ),
                )
            };
            format!(
                "<html><body><div class=\"product\" id=\"product-{post_id}\"><h1>Chair {post_id}</h1><p class=\"stock\">{stock}</p>{button}<img src=\"/images/{post_id}.jpg\"></div></body></html>"
            )
        }

        fn input_page(url: &str, role: &str, html: String) -> SchemaReviewPageInput {
            SchemaReviewPageInput {
                url: url.to_owned(),
                role: role.to_owned(),
                raw_html: html,
            }
        }

        fn identity_findings_of(
            matrix: &SchemaMatrix,
            candidate: usize,
        ) -> Vec<Vec<IdentityAgreementFinding>> {
            matrix.candidates[candidate]
                .pages
                .iter()
                .map(|page| page.identity_agreement_findings.as_slice().to_vec())
                .collect()
        }

        fn differs(check: IdentityAgreementCheck, value: &str) -> IdentityAgreementFinding {
            IdentityAgreementFinding {
                field: PlausibilityField::SourceListingId,
                check,
                values: vec![value.to_owned()],
            }
        }

        #[test]
        fn should_block_deterministic_approval_when_two_schemas_derive_different_identities_from_a_page()
         {
            let schemas = vec![button_schema(), container_schema()];
            let pages = vec![
                input_page(ACTIVE_URL, PAGE_ROLE_PRIMARY, product_html(53036, false)),
                input_page(SOLD_URL, PAGE_ROLE_SEED, product_html(53037, true)),
            ];

            let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

            assert!(schema_matrix_has_required_coverage(&matrix));
            assert_eq!(
                identity_findings_of(&matrix, 0),
                vec![
                    vec![differs(
                        IdentityAgreementCheck::DiffersFromOtherSchemas,
                        "product-53036"
                    )],
                    vec![],
                ]
            );
            assert_eq!(
                identity_findings_of(&matrix, 1),
                vec![
                    vec![differs(
                        IdentityAgreementCheck::DiffersFromOtherSchemas,
                        "53036"
                    )],
                    vec![],
                ],
                "the sold page has no other applying schema to disagree with"
            );
            assert!(!schema_matrix_deterministic_approval_ok(&matrix));
            assert_eq!(implausible_pages(&matrix).len(), 2);
        }

        #[test]
        fn should_report_a_conflict_with_the_captured_identity_when_only_the_new_schema_applies() {
            let schemas = vec![button_schema(), container_schema()];
            let review_id = CrawlerReviewId::new();
            let sold_page = CrawlerReviewPage {
                review_page_id: crate::CrawlerReviewPageId::new(),
                review_id,
                url: SOLD_URL.to_owned(),
                role: PAGE_ROLE_PRIMARY.to_owned(),
                html_hash: String::new(),
                fetched: time::OffsetDateTime::now_utc(),
            };
            let captured = HashMap::from([(SOLD_URL.to_owned(), "53036".to_owned())]);

            let matrix = evaluate_schema_matrix_for_live_review_pages(
                review_id,
                &schemas,
                &[],
                &[(sold_page, product_html(53036, true))],
                &captured,
            );

            assert!(!matrix.candidates[0].pages[0].apply_ok);
            assert_eq!(identity_findings_of(&matrix, 0), vec![vec![]]);
            assert_eq!(
                identity_findings_of(&matrix, 1),
                vec![vec![differs(
                    IdentityAgreementCheck::DiffersFromCapturedIdentity,
                    "53036"
                )]]
            );
            assert!(!schema_matrix_deterministic_approval_ok(&matrix));
        }

        #[test]
        fn should_pass_identity_agreement_when_the_sold_schema_derives_the_captured_identity() {
            let schemas = vec![
                button_schema(),
                identity_schema(attribute_rule("div.product", "data-product_id")),
            ];
            let pages = vec![
                input_page(ACTIVE_URL, PAGE_ROLE_PRIMARY, product_html(53036, false)),
                input_page(SOLD_URL, PAGE_ROLE_SEED, product_html(53037, true)),
            ]
            .into_iter()
            .map(|mut page| {
                page.raw_html = page.raw_html.replace(
                    "<div class=\"product\" id=\"product-",
                    "<div class=\"product\" data-product_id=\"",
                );
                page
            })
            .collect::<Vec<_>>();

            let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

            assert!(
                matrix
                    .candidates
                    .iter()
                    .flat_map(|candidate| &candidate.pages)
                    .all(|page| page.identity_agreement_findings.is_empty())
            );
            assert!(schema_matrix_deterministic_approval_ok(&matrix));
        }

        /// Wick's inquiry form, Lennox Cato's SKU span and Hickmet's product-id input are
        /// absent on sold, archived and POA pages: the identity rule is applied to every
        /// review page, and a page where it yields nothing fails the gate.
        #[test]
        fn should_block_deterministic_approval_when_the_identity_is_empty_on_a_sold_variant() {
            let schemas = vec![identity_schema(attribute_rule(
                "button.single_add_to_cart_button",
                "value",
            ))];
            let pages = vec![
                input_page(ACTIVE_URL, PAGE_ROLE_PRIMARY, product_html(53036, false)),
                input_page(SOLD_URL, PAGE_ROLE_SEED, product_html(53037, true)),
            ];

            let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

            assert!(matrix.candidates[0].pages.iter().all(|page| page.apply_ok));
            assert_eq!(
                matrix.candidates[0].pages[1]
                    .plausibility_findings
                    .as_slice(),
                [finding(
                    PlausibilityField::SourceListingId,
                    PlausibilityCheck::Blank
                )]
            );
            assert!(!schema_matrix_deterministic_approval_ok(&matrix));
            assert_eq!(implausible_pages(&matrix).len(), 1);
        }
    }

    /// The `images` field reports how many matched gallery elements the rule skipped
    /// for having no image URL candidate.
    mod skipped_image_elements {
        use super::*;

        fn gallery_schema() -> ProductCssSelectorSchema {
            ProductCssSelectorSchema {
                images: ExtractionRule {
                    selector: CssSelector::from("#gallery a"),
                    additional_selectors: Vec::new(),
                    extract: ExtractionKind::ImageUrl,
                    cardinality: ExtractionCardinality::All,
                },
                ..schema("h1")
            }
        }

        fn gallery_page(anchors: &str) -> SchemaReviewPageInput {
            SchemaReviewPageInput {
                url: "https://example.com/product".to_string(),
                role: PAGE_ROLE_PRIMARY.to_string(),
                raw_html: format!(
                    "<html><body><span id=\"product-id\">SKU</span><h1>Title</h1><span id=\"state\">Available</span><div id=\"gallery\">{anchors}</div></body></html>"
                ),
            }
        }

        fn field<'a>(page: &'a SchemaPageEvaluation, name: &str) -> &'a SelectorFieldEvaluation {
            page.fields
                .iter()
                .find(|field| field.field == name)
                .unwrap_or_else(|| panic!("{name} field must be in the matrix"))
        }

        #[test]
        fn should_cover_page_and_count_anchors_without_image_url_candidate() {
            let pages = vec![gallery_page(
                "<a href=\"/images/1.jpg\"></a><a href=\"/images/2.jpg\"></a><a href=\"#\"></a><a href=\"#\"></a><a href=\"#\"></a>",
            )];

            let matrix = evaluate_schema_matrix_for_inputs(&[gallery_schema()], &pages);
            let page = &matrix.candidates[0].pages[0];

            assert!(page.apply_ok);
            assert_eq!(
                page.extracted.as_ref().map(|raw| raw.images.clone()),
                Some(vec![
                    "/images/1.jpg".to_string(),
                    "/images/2.jpg".to_string()
                ])
            );
            let images = field(page, "images");
            assert_eq!(images.selector_match_count, Some(5));
            assert_eq!(images.skipped_element_count, Some(3));
            assert_eq!(field(page, "title").skipped_element_count, None);
            assert!(
                schema_matrix_deterministic_approval_ok(&matrix),
                "skipped elements are review information and do not block approval"
            );
        }

        #[test]
        fn should_count_gallery_images_without_the_attribute_of_an_attribute_rule() {
            let schema = ProductCssSelectorSchema {
                images: ExtractionRule {
                    selector: CssSelector::from("#gallery img"),
                    additional_selectors: Vec::new(),
                    extract: ExtractionKind::Attribute { name: "src".into() },
                    cardinality: ExtractionCardinality::All,
                },
                ..schema("h1")
            };
            let pages = vec![gallery_page(
                "<img src=\"/images/1.jpg\"><img data-src=\"/images/2.jpg\"><img src=\"/images/3.jpg\">",
            )];

            let matrix = evaluate_schema_matrix_for_inputs(&[schema], &pages);
            let page = &matrix.candidates[0].pages[0];

            assert!(page.apply_ok);
            let images = field(page, "images");
            assert_eq!(images.selector_match_count, Some(3));
            assert_eq!(images.skipped_element_count, Some(1));
        }

        #[test]
        fn should_count_skipped_anchors_when_the_page_fails_before_its_images_rule() {
            let schema = ProductCssSelectorSchema {
                title: text_rule("h2"),
                ..gallery_schema()
            };
            let pages = vec![gallery_page(
                "<a href=\"/images/1.jpg\"></a><a href=\"#\"></a><a href=\"#\"></a>",
            )];

            let matrix = evaluate_schema_matrix_for_inputs(&[schema], &pages);
            let page = &matrix.candidates[0].pages[0];

            assert!(!page.apply_ok);
            assert_eq!(
                page.error.as_deref(),
                Some(
                    "failed to extract `title`: no element matched selector `h2` (or any additional selectors)"
                )
            );
            assert_eq!(field(page, "images").skipped_element_count, Some(2));
        }

        /// Pairs one gallery's extraction with another gallery's HTML, so the count shows
        /// whether it came from the extraction or from applying the `images` rule again.
        #[test]
        fn should_apply_the_images_rule_again_only_when_another_rule_failed_the_page() {
            let html_of = |anchors: &str| Html::parse_document(&gallery_page(anchors).raw_html);
            let two_skipped =
                html_of("<a href=\"/images/1.jpg\"></a><a href=\"#\"></a><a href=\"#\"></a>");
            let none_skipped = html_of("<a href=\"/images/1.jpg\"></a>");
            let all_skipped = html_of("<a href=\"#\"></a>");
            let applies = gallery_schema();
            let fails_on_title = ProductCssSelectorSchema {
                title: text_rule("h2"),
                ..gallery_schema()
            };

            let applied = applies.extract(&two_skipped);
            assert!(applied.is_ok());
            assert_eq!(
                skipped_image_elements(&applies, &none_skipped, &applied),
                Some(2),
                "an applied page reports its extraction's count"
            );

            let title_failed = fails_on_title.extract(&two_skipped);
            assert!(matches!(title_failed, Err(ApplySchemaError::Title(_))));
            assert_eq!(
                skipped_image_elements(&fails_on_title, &none_skipped, &title_failed),
                Some(0),
                "a page another rule failed applies the images rule again"
            );

            let images_failed = applies.extract(&all_skipped);
            assert!(matches!(images_failed, Err(ApplySchemaError::Images(_))));
            assert_eq!(
                skipped_image_elements(&applies, &two_skipped, &images_failed),
                None,
                "a page the images rule failed has no count"
            );
        }

        #[test]
        fn should_fail_page_without_a_count_when_every_anchor_lacks_an_image_url_candidate() {
            let pages = vec![gallery_page("<a href=\"#\"></a><a href=\"#\"></a>")];

            let matrix = evaluate_schema_matrix_for_inputs(&[gallery_schema()], &pages);
            let page = &matrix.candidates[0].pages[0];

            assert!(!page.apply_ok);
            assert_eq!(
                page.error.as_deref(),
                Some(
                    "failed to extract `images`: element matched by `#gallery a` does not have attribute `image URL candidate`"
                )
            );
            assert_eq!(field(page, "images").skipped_element_count, None);
        }
    }

    #[test]
    fn should_report_raw_attribute_selector_fields_when_configured() {
        let schemas = vec![schema_with_raw_attributes("h1")];
        let pages = vec![SchemaReviewPageInput {
            url: "https://example.com/product".to_string(),
            role: PAGE_ROLE_PRIMARY.to_string(),
            raw_html: "<html><body><span id=\"product-id\">SKU</span><h1>Title</h1><span id=\"state\">Available</span><img src=\"a.jpg\"><span id=\"shipping\">4 - 6 weeks</span><span id=\"condition\">Good</span><span id=\"material\">Walnut</span><span id=\"year\">1830</span><span id=\"category\">Furniture</span><span id=\"measurements\">H 90 cm</span><span id=\"origin\">Germany</span></body></html>".to_string(),
        }];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);
        let fields = &matrix.candidates[0].pages[0].fields;

        assert!(fields.iter().any(|field| field.field == "rawShipment"));
        assert!(fields.iter().any(|field| field.field == "rawCondition"));
        assert!(fields.iter().any(|field| field.field == "rawMaterial"));
        assert!(fields.iter().any(|field| field.field == "rawYear"));
        assert!(fields.iter().any(|field| field.field == "rawCategory"));
        assert!(fields.iter().any(|field| field.field == "rawMeasurements"));
        assert!(fields.iter().any(|field| field.field == "rawOrigin"));
        assert_eq!(
            matrix.candidates[0].pages[0]
                .extracted
                .as_ref()
                .and_then(|raw| raw.raw_attributes.get("rawShipment")),
            Some(&vec!["4 - 6 weeks".to_string()])
        );
        assert_eq!(
            matrix.candidates[0].pages[0]
                .extracted
                .as_ref()
                .and_then(|raw| raw.raw_attributes.get("rawMaterial")),
            Some(&vec!["Walnut".to_string()])
        );
    }
}
