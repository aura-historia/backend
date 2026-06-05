use super::projection::{clean_html_for_schema_generation, html_to_schema_prompt_dsl};
use crate::scraper::css_selector::product_schema::{ApplySchemaError, ProductCssSelectorSchema};
use common::logging::LlmOperation;
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaPromptSource {
    YamlProjection,
    CleanedHtmlFallback,
}

impl SchemaPromptSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::YamlProjection => "yaml_projection",
            Self::CleanedHtmlFallback => "cleaned_html_fallback",
        }
    }

    fn sample_label(self) -> &'static str {
        match self {
            Self::YamlProjection => "YAML",
            Self::CleanedHtmlFallback => "CLEANED HTML",
        }
    }

    fn sample_description(self) -> &'static str {
        match self {
            Self::YamlProjection => {
                "The samples below are compact YAML projections of the original HTML. Derive CSS selectors from the tags, attrs, text, and tree context, and target the original raw HTML."
            }
            Self::CleanedHtmlFallback => {
                "The samples below are cleaned HTML from the original pages. Derive CSS selectors from this HTML context, and target the original raw HTML."
            }
        }
    }
}

pub(super) fn build_create_schemas_instruction(
    html_pages: &[String],
    prompt_source: SchemaPromptSource,
) -> String {
    let prompt_pages: Vec<String> = if html_pages.is_empty() {
        Vec::new()
    } else {
        html_pages
            .iter()
            .map(|html| match prompt_source {
                SchemaPromptSource::YamlProjection => html_to_schema_prompt_dsl(html),
                SchemaPromptSource::CleanedHtmlFallback => clean_html_for_schema_generation(html),
            })
            .collect()
    };

    if prompt_pages.is_empty() {
        return String::from(
            "Generate robust Extraction-Schemas for the given HTML product pages. Return ONLY ProductSchemaGenerationResponse JSON with schemas plus confidence LOW, MEDIUM, or HIGH.",
        );
    }

    let mut samples = String::new();
    for (idx, prompt_page) in prompt_pages.iter().enumerate() {
        let _ = std::fmt::Write::write_fmt(
            &mut samples,
            format_args!(
                "\n--- SAMPLE {sample_idx} {sample_label} ---\n{page_dsl}\n",
                sample_idx = idx + 1,
                sample_label = prompt_source.sample_label(),
                page_dsl = prompt_page,
            ),
        );
    }

    let template_instruction = if prompt_pages.len() > 1 {
        "Infer the distinct product-page templates represented by these samples. Return one schema per distinct template, not one schema per page. If all samples clearly share the same template, return one schema; otherwise return multiple schemas so every template has precise selectors.\n"
    } else {
        "Return one schema for the single observed product-page template.\n"
    };

    format!(
        "Generate robust Extraction-Schemas that together cover these product page HTML samples from the same shop.\n\
         {template_instruction}\
         Shops often have multiple templates/layouts. Do not collapse different templates into one overly broad schema just because fields share names.\n\
         A schema may target only the subset of samples where its selectors are precise and product-specific.\n\
         A schema applies to a sample only when every non-null extraction rule in that schema exists in that sample HTML and extracts successfully.\n\
         Optional fields are optional only when the field is null for that schema because the field is not applicable to that schema's own product template.\n\
         Never omit an applicable field from one product template just to make one broad schema also work for another template.\n\
         If an applicable field differs by template, availability state, layout, DOM presence, or selector, split the samples into multiple schemas and preserve the applicable rules in each schema.\n\
         One schema is valid only when all applicable fields and every non-null selector apply across all samples that schema covers.\n\
         Return schemas ordered by specificity and completeness: first the schema with the most non-null extraction rules, then fallback templates with fewer applicable rules. When rule counts tie, put the schema with more specific product-focused selectors first.\n\
         Examples: if template A has price and template B has no price element, generate two schemas and put the priced schema first. If an auction template has estimate fields and a buy-now template has fixed price, generate separate schemas ordered by rule count. If a sold-item template lacks buy price but has sold state, split schemas when selectors differ.\n\
         Prefer high-precision selectors that represent semantic fields rather than layout wrappers.\n\
         Return ONLY ProductSchemaGenerationResponse JSON with fields schemas, confidence, summary, risks, and page_findings. The schemas field contains one ProductCssSelectorSchema for one product template or multiple schemas ordered as described above.\n\
         Use confidence HIGH only when selectors are product-specific and likely safe for unattended approval after deterministic validation. Use MEDIUM for plausible schemas with ambiguity. Use LOW when selectors or fields are uncertain. MEDIUM and LOW require human review.\n\
         {sample_description}\n\
         Here are the page {sample_label} samples:{samples}",
        sample_description = prompt_source.sample_description(),
        sample_label = prompt_source.sample_label()
    )
}

pub(super) fn build_append_schema_instruction(
    html: &str,
    failed_schema: Option<&ProductCssSelectorSchema>,
    last_error: Option<&ApplySchemaError>,
) -> String {
    let page_dsl = html_to_schema_prompt_dsl(html);
    let failure_context = match (failed_schema, last_error) {
        (Some(schema), Some(error)) => {
            let schema_json = serde_json::to_string_pretty(schema)
                .unwrap_or_else(|_| "<failed to serialize previous schema>".to_string());
            format!(
                "\nPrevious attempt failed. Here is the schema that just failed:\n{schema_json}\n\
                 Extraction failure observed:\n{error}\n\
                 Improve/fix the failed schema for this page instead of repeating the same selectors."
            )
        }
        _ => String::new(),
    };

    format!(
        "Generate a single robust Extraction-Schema for the following product page HTML.\n\
          This schema will be appended to a set of existing schemas from the same shop.\n\
          Focus on this specific layout and make the selectors resilient.\n\
          Return ONLY ProductSchemaGenerationResponse JSON. The schemas field must contain exactly one ProductCssSelectorSchema object for this page.\n\
          Use confidence HIGH only when selectors are product-specific and likely safe for unattended approval after deterministic validation. Use MEDIUM for plausible schemas with ambiguity. Use LOW when selectors or fields are uncertain. MEDIUM and LOW require human review.\n\
          Optional fields may remain null if not confidently present.\n\
          {failure_context}\n\
          The sample below is a compact YAML projection of the original HTML. Derive CSS selectors from the tags, attrs, text, and tree context, and target the original raw HTML.\n\
          Here is the compact page YAML:\n\
          {page_dsl}"
    )
}

#[derive(Debug, Default)]
struct SchemaPromptSizeTotals {
    raw_html_bytes: usize,
    cleaned_html_bytes: usize,
    yaml_bytes: usize,
}

impl SchemaPromptSizeTotals {
    fn add(&mut self, raw_html: &str, cleaned_html: &str, yaml: &str) {
        self.raw_html_bytes += raw_html.len();
        self.cleaned_html_bytes += cleaned_html.len();
        self.yaml_bytes += yaml.len();
    }
}

pub(super) fn log_schema_prompt_size_from_raw_pages(
    operation: LlmOperation,
    html_pages: &[String],
) {
    let mut totals = SchemaPromptSizeTotals::default();
    for html in html_pages {
        let cleaned_html = clean_html_for_schema_generation(html);
        let yaml = html_to_schema_prompt_dsl(html);
        totals.add(html, &cleaned_html, &yaml);
    }

    log_schema_prompt_size(operation, html_pages.len(), totals);
}

fn log_schema_prompt_size(
    operation: LlmOperation,
    page_count: usize,
    totals: SchemaPromptSizeTotals,
) {
    info!(
        llmOperation = %operation,
        page_count,
        raw_html_bytes = totals.raw_html_bytes,
        cleaned_html_bytes = totals.cleaned_html_bytes,
        yaml_bytes = totals.yaml_bytes,
        raw_html_tokens = approx_prompt_tokens(totals.raw_html_bytes),
        cleaned_html_tokens = approx_prompt_tokens(totals.cleaned_html_bytes),
        yaml_tokens = approx_prompt_tokens(totals.yaml_bytes),
        yaml_vs_cleaned_percent = percent(totals.yaml_bytes, totals.cleaned_html_bytes),
        "Schema prompt source size summary"
    );
}

fn approx_prompt_tokens(chars: usize) -> usize {
    chars / 4
}

fn percent(part: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        (part as f64 / total as f64) * 100.0
    }
}
