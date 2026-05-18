use crate::review::model::{CrawlerReviewPage, SchemaPageEvaluation, SelectorFieldEvaluation};
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use scraper::{Html, Selector};

pub(super) fn evaluate_schema_page(
    schema: &ProductCssSelectorSchema,
    page: &CrawlerReviewPage,
) -> SchemaPageEvaluation {
    let html = Html::parse_document(&page.raw_html);
    let apply_result = schema.apply(&html);
    let fields = evaluate_schema_fields(schema, &page.raw_html);

    match apply_result {
        Ok(extracted) => SchemaPageEvaluation {
            page_id: page.review_page_id,
            url: page.url.clone(),
            role: page.role.clone(),
            apply_ok: true,
            extracted: Some(extracted),
            error: None,
            fields,
        },
        Err(err) => SchemaPageEvaluation {
            page_id: page.review_page_id,
            url: page.url.clone(),
            role: page.role.clone(),
            apply_ok: false,
            extracted: None,
            error: Some(err.to_string()),
            fields,
        },
    }
}

fn evaluate_schema_fields(
    schema: &ProductCssSelectorSchema,
    html: &str,
) -> Vec<SelectorFieldEvaluation> {
    let mut fields = Vec::new();
    fields.push(evaluate_rule(
        "shops_product_id",
        &schema.shops_product_id,
        html,
    ));
    fields.push(evaluate_rule("title", &schema.title, html));
    if let Some(rule) = &schema.description {
        fields.push(evaluate_rule("description", rule, html));
    }
    if let Some(rule) = &schema.price {
        fields.push(evaluate_rule("price", rule, html));
    }
    if let Some(rule) = &schema.price_estimate_min {
        fields.push(evaluate_rule("price_estimate_min", rule, html));
    }
    if let Some(rule) = &schema.price_estimate_max {
        fields.push(evaluate_rule("price_estimate_max", rule, html));
    }
    fields.push(evaluate_rule("state", &schema.state, html));
    fields.push(evaluate_rule("images", &schema.images, html));
    if let Some(rule) = &schema.auction_start {
        fields.push(evaluate_rule("auction_start", rule, html));
    }
    if let Some(rule) = &schema.auction_end {
        fields.push(evaluate_rule("auction_end", rule, html));
    }
    fields
}

fn evaluate_rule(
    field: &str,
    rule: &crate::scraper::css_selector::rule::ExtractionRule,
    html: &str,
) -> SelectorFieldEvaluation {
    let document = Html::parse_document(html);
    let selector = rule.selector.to_string();
    let selector_match_count = match Selector::parse(&selector) {
        Ok(parsed) => Some(document.select(&parsed).count()),
        Err(err) => {
            let error = format!("{err:?}");
            return SelectorFieldEvaluation {
                field: field.to_string(),
                selector: selector.clone(),
                selector_match_count: None,
                additional_selector_match_counts: Vec::new(),
                error: Some(error),
            };
        }
    };

    let mut additional_selector_match_counts = Vec::new();
    for additional in &rule.additional_selectors {
        match Selector::parse(additional.as_ref()) {
            Ok(parsed) => additional_selector_match_counts.push(document.select(&parsed).count()),
            Err(_) => additional_selector_match_counts.push(0),
        }
    }

    SelectorFieldEvaluation {
        field: field.to_string(),
        selector,
        selector_match_count,
        additional_selector_match_counts,
        error: None,
    }
}
