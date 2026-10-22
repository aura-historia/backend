use crate::CrawlerReviewId;
use crate::review::model::{
    CrawlerReviewPage, SchemaCandidateEvaluation, SchemaMatrix, SchemaPageEvaluation,
    SchemaPageReference, SchemaReviewPageInput, SelectorFieldEvaluation,
};
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::css_selector::rule::ExtractionRule;
use crate::scraper::plausibility::{CrossPageFindings, PageExtraction, PlausibilityFindings};
use crate::scraper::scraper_service::extraction::engine::apply_schema_to_document;
use scraper::{Html, Selector};

/// A review page as the matrix evaluates it: how the matrix refers to it and its HTML.
struct MatrixPage<'a> {
    page_reference: SchemaPageReference,
    url: &'a str,
    role: &'a str,
    raw_html: &'a str,
}

pub(crate) fn evaluate_schema_matrix_for_live_review_pages(
    review_id: CrawlerReviewId,
    schemas: &[ProductCssSelectorSchema],
    pages: &[(CrawlerReviewPage, String)],
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
        })
        .collect();
    SchemaMatrix {
        review_id: Some(review_id),
        candidates: evaluate_candidates(schemas, &pages),
    }
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
        })
        .collect();
    SchemaMatrix {
        review_id: None,
        candidates: evaluate_candidates(schemas, &pages),
    }
}

fn evaluate_candidates(
    schemas: &[ProductCssSelectorSchema],
    pages: &[MatrixPage<'_>],
) -> Vec<SchemaCandidateEvaluation> {
    schemas
        .iter()
        .enumerate()
        .map(|(schema_index, schema)| {
            let mut page_evaluations: Vec<SchemaPageEvaluation> = pages
                .iter()
                .map(|page| evaluate_schema_page(schema, page))
                .collect();
            add_cross_page_findings(&mut page_evaluations);
            SchemaCandidateEvaluation {
                schema_index,
                pages: page_evaluations,
            }
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

/// Deterministic approval requires every page to be covered by some candidate, no
/// optional rule that matched nothing on every page its candidate applied to, and no
/// failed plausibility check. The generator had those exact pages in hand, so a total
/// miss is a defective selector, not page variance. A failed check, per page or across
/// pages, shows a selector capturing something other than the product's value, even
/// where runtime selection would route the page to another candidate, so the schemas
/// stay in human review.
pub(crate) fn schema_matrix_deterministic_approval_ok(matrix: &SchemaMatrix) -> bool {
    schema_matrix_has_required_coverage(matrix)
        && optional_rules_missing_on_every_page(matrix).is_empty()
        && !matrix
            .candidates
            .iter()
            .flat_map(|candidate| &candidate.pages)
            .any(SchemaPageEvaluation::fails_plausibility_checks)
}

/// A review page on which a candidate's extraction failed a per-page or cross-page
/// plausibility check.
#[derive(Debug, serde::Serialize)]
pub(crate) struct ImplausiblePage<'a> {
    schema_index: usize,
    page_reference: &'a SchemaPageReference,
    url: &'a str,
    role: &'a str,
    plausibility_findings: &'a PlausibilityFindings,
    cross_page_findings: &'a CrossPageFindings,
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
                })
        })
        .collect()
}

/// An optional rule of one candidate that matched nothing (primary and additional
/// selectors) on every page the candidate applied to.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct OptionalRuleTotalMiss {
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

    first_candidate.pages.iter().all(|target_page| {
        matrix.candidates.iter().any(|candidate| {
            candidate
                .pages
                .iter()
                .find(|page| page.page_reference == target_page.page_reference)
                .is_some_and(page_has_required_extraction)
        })
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
) -> SchemaPageEvaluation {
    let html = Html::parse_document(page.raw_html);
    let apply_result = apply_schema_to_document(schema, &html);
    let fields = evaluate_schema_fields(schema, &html);

    // A page counts as applied only when runtime selection would accept the
    // extraction, so a rejecting plausibility finding fails the page like an
    // apply error; every finding stays listed for the reviewer.
    let (apply_ok, extracted, error, plausibility_findings) = match apply_result {
        Ok(extracted) => {
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
        fields,
    }
}

fn page_has_required_extraction(page: &SchemaPageEvaluation) -> bool {
    page.apply_ok && page.extracted.is_some()
}

fn evaluate_schema_fields(
    schema: &ProductCssSelectorSchema,
    html: &Html,
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
    fields.push(evaluate_images_rule(schema, html));
    for (field, rule) in &schema.raw_attributes {
        fields.push(evaluate_rule(field, rule, html, false));
    }
    fields
}

/// Evaluates the `images` rule as runtime extraction applies it, with the count of
/// matched elements it skipped for having no value. A failed images rule fails the
/// page, whose error the page evaluation already carries.
fn evaluate_images_rule(schema: &ProductCssSelectorSchema, html: &Html) -> SelectorFieldEvaluation {
    SelectorFieldEvaluation {
        skipped_element_count: schema
            .apply_image_url_candidate_groups(html)
            .ok()
            .map(|images| images.skipped_elements),
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
            };
        }
    };
    let selector_match_count = Some(rule.match_count(html, &parsed));
    let additional_selector_match_counts = rule
        .additional_selectors
        .iter()
        .map(|additional| {
            Selector::parse(additional.as_ref()).map_or(0, |parsed| rule.match_count(html, &parsed))
        })
        .collect();

    SelectorFieldEvaluation {
        field: field.to_string(),
        selector,
        selector_match_count,
        additional_selector_match_counts,
        error: None,
        required,
        skipped_element_count: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::{PAGE_ROLE_PRIMARY, PAGE_ROLE_SEED};
    use crate::scraper::css_selector::rule::{CssSelector, ExtractionCardinality, ExtractionKind};
    use crate::scraper::plausibility::{PlausibilityCheck, PlausibilityField, PlausibilityFinding};

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
