use super::projection::html_to_schema_prompt_dsl;
use crate::scraper::css_selector::raw_attribute_registry::RAW_ATTRIBUTE_GROUPS;

const SAMPLE_LABEL: &str = "YAML";
const SAMPLE_DESCRIPTION: &str = "The samples below are compact YAML projections of the original HTML. Derive CSS selectors from the tags, attrs, text, and tree context, and target the original raw HTML.";
const SELECTOR_GROUNDING_INSTRUCTION: &str = "Only use CSS selectors that can be derived from tags, attrs, text, and tree context visible in the YAML projection. Never invent class names, ids, attributes, or selector paths that are not present in the YAML. Every non-null selector must be product-specific and must apply to every sample covered by that schema. Optional fields must be null unless an exact selector is visible in the YAML and clearly extracts non-empty product-specific content from the page or template. Prefer null over guessed selectors, generic wrappers, layout containers, or whole-page content selectors. For description, use null unless a precise product-description selector is visible; do not use generic selectors such as .description-wrapper or .HTMLPageContent unless those exact classes appear in the YAML on nodes containing product description text.";
const PRODUCT_SAMPLES_INSTRUCTION: &str = "Every sample was fetched from a URL matching the product-page pattern of this ListingSource and is expected to be a product detail page. Do not classify the samples: this response has no page_kind, removed_schema, or reason field. If a sample looks like a removed, sold-out, not-found, category, search, or other non-product page, exclude that sample from schema coverage, still return schemas for the remaining product samples, and name the excluded sample number in risks. The schemas array must contain at least one schema; when a sample is ambiguous, still cover the samples that are product detail pages and lower the confidence.";
const PRICE_INSTRUCTION: &str = "Text extraction drops struck-through descendants (del, s, strike). For a sale price shown as a struck-through old amount next to the current amount, select the price element that contains both, not an ins element or the struck-through element, so the same selector also matches regular-price pages.";
const CONFIDENCE_INSTRUCTION: &str = "Use confidence HIGH only when selectors are product-specific and likely safe for unattended approval after deterministic validation. Use MEDIUM for plausible schemas with ambiguity. Use LOW when selectors or fields are uncertain. MEDIUM and LOW require human review.";

fn raw_attributes_instruction() -> String {
    let mut definitions = String::new();
    for group in RAW_ATTRIBUTE_GROUPS {
        let _ = std::fmt::Write::write_fmt(
            &mut definitions,
            format_args!(" Raw attribute group `{}` supports:", group.name),
        );
        for definition in group.definitions {
            let _ = std::fmt::Write::write_fmt(
                &mut definitions,
                format_args!(" `{}` ({})", definition.key, definition.description),
            );
        }
    }

    format!(
        "Generate raw_attributes selector rules only for configured raw attribute groups.{definitions} Use these keys exactly. Do not generate arbitrary raw attribute keys. Prefer the broad group key when a page exposes combined text for a group. Prefer specific creator keys when a field label explicitly names artist, maker, designer, brand, or signature; use rawCreatorNote for combined or attribution-style creator text. Use specific measurement or origin keys only when the exact value is separately visible. Add a raw_attributes rule only when exact visible product-specific data from a configured group is present in the YAML projection. Extract raw text only; do not derive, split, normalize, translate, calculate, or infer values that are not separately visible. Do not infer artist, maker, designer, brand, or signature from title, URL, navigation, meta author, seller, or page boilerplate. Omit raw_attributes or omit individual keys when no precise selector-bound value exists."
    )
}

pub(super) fn build_create_schemas_instruction(html_pages: &[String]) -> String {
    let prompt_pages: Vec<String> = if html_pages.is_empty() {
        Vec::new()
    } else {
        html_pages
            .iter()
            .map(|html| html_to_schema_prompt_dsl(html))
            .collect()
    };

    if prompt_pages.is_empty() {
        return String::from(
            "Generate robust Extraction-Schemas for the given HTML product pages. Return ONLY ProductListingSchemasGenerationResponse JSON with schemas plus confidence LOW, MEDIUM, or HIGH.",
        );
    }

    let mut samples = String::new();
    for (idx, prompt_page) in prompt_pages.iter().enumerate() {
        let _ = std::fmt::Write::write_fmt(
            &mut samples,
            format_args!(
                "\n--- SAMPLE {sample_idx} {sample_label} ---\n{page_dsl}\n",
                sample_idx = idx + 1,
                sample_label = SAMPLE_LABEL,
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
        "Generate robust Extraction-Schemas that together cover these product page HTML samples from the same ListingSource.\n\
         {product_samples_instruction}\n\
         {template_instruction}\
         ListingSources often have multiple templates/layouts. Do not collapse different templates into one overly broad schema just because fields share names.\n\
         A schema may target only the subset of samples where its selectors are precise and product-specific.\n\
         A schema applies to a sample only when every non-null extraction rule in that schema exists in that sample HTML and extracts successfully.\n\
         Optional fields are optional only when the field is null for that schema because the field is not applicable to that schema's own product template.\n\
         Never omit an applicable field from one product template just to make one broad schema also work for another template.\n\
         If an applicable field differs by template, availability state, layout, DOM presence, or selector, split the samples into multiple schemas and preserve the applicable rules in each schema.\n\
         One schema is valid only when all applicable fields and every non-null selector apply across all samples that schema covers.\n\
         {selector_grounding_instruction}\n\
         Return schemas ordered by specificity and completeness: first the schema with the most non-null extraction rules, then fallback templates with fewer applicable rules. When rule counts tie, put the schema with more specific product-focused selectors first.\n\
         Examples: if template A has price and template B has no price element, generate two schemas and put the priced schema first. If an auction template has estimate fields and a buy-now template has fixed price, generate separate schemas ordered by rule count. If a sold-item template lacks buy price but has sold state, split schemas when selectors differ.\n\
         Prefer high-precision selectors that represent semantic fields rather than layout wrappers.\n\
         {price_instruction}\n\
         {raw_attributes_instruction}\n\
         Return ONLY ProductListingSchemasGenerationResponse JSON with fields schemas, confidence, summary, and risks. The schemas field contains one ProductCssSelectorSchema for one product template or multiple schemas ordered as described above.\n\
         {confidence_instruction}\n\
         {sample_description}\n\
         Here are the page {sample_label} samples:{samples}",
        product_samples_instruction = PRODUCT_SAMPLES_INSTRUCTION,
        selector_grounding_instruction = SELECTOR_GROUNDING_INSTRUCTION,
        raw_attributes_instruction = raw_attributes_instruction(),
        price_instruction = PRICE_INSTRUCTION,
        confidence_instruction = CONFIDENCE_INSTRUCTION,
        sample_description = SAMPLE_DESCRIPTION,
        sample_label = SAMPLE_LABEL
    )
}

pub(super) fn build_single_schema_instruction(html: &str) -> String {
    let prompt_page = html_to_schema_prompt_dsl(html);

    format!(
        "Classify the following HTML from a URL expected to be a product page, then return one single-schema response.\n\
          The page may be a product page, a removed/404-like product page, or a wrong URL type.\n\
          Choose page_kind = product when the page is a real product detail page and return exactly one ProductCssSelectorSchema in schemas.\n\
          Choose page_kind = removed when the page is a removed, gone, not-found, deleted, or 404-like page for a product URL served with HTTP 200. Return no product schemas and set removed_schema to selector-bound evidence that proves the removed state.\n\
          Choose page_kind = not_product when the page is clearly a category, search, home, info, navigation, listing, or other non-product page, and not a removed product page. Return no schemas and include a short reason.\n\
          For removed, removed_schema must include selector and exactly one of text or regex. Use text for stable exact visible text. Use regex for variable removed messages like \"the table from 2020 is not available anymore\"; regex must be valid Rust regex syntax and match the selected normalized text after trimming, whitespace collapse, and lowercasing.\n\
           For product, generate one new schema for this specific layout and make the selectors resilient.\n\
          {selector_grounding_instruction}\n\
          {price_instruction}\n\
          {raw_attributes_instruction}\n\
          Return ONLY ProductListingSchemaGenerationResponse JSON.\n\
          {confidence_instruction}\n\
          {sample_description}\n\
          Here is the page {sample_label}:\n\
          {prompt_page}",
        selector_grounding_instruction = SELECTOR_GROUNDING_INSTRUCTION,
        raw_attributes_instruction = raw_attributes_instruction(),
        price_instruction = PRICE_INSTRUCTION,
        confidence_instruction = CONFIDENCE_INSTRUCTION,
        sample_description = SAMPLE_DESCRIPTION,
        sample_label = SAMPLE_LABEL
    )
}
