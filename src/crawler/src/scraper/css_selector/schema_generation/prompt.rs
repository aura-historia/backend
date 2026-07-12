use super::projection::html_to_schema_prompt_dsl;

const SAMPLE_LABEL: &str = "YAML";
const SAMPLE_DESCRIPTION: &str = "The samples below are compact YAML projections of the original HTML. Derive CSS selectors from the tags, attrs, text, and tree context, and target the original raw HTML.";
const SELECTOR_GROUNDING_INSTRUCTION: &str = "Only use CSS selectors that can be derived from tags, attrs, text, and tree context visible in the YAML projection. Never invent class names, ids, attributes, or selector paths that are not present in the YAML. Every non-null selector must be product-specific and must apply to every sample covered by that schema. Optional fields must be null unless an exact selector is visible in the YAML and clearly extracts non-empty product-specific content from the page or template. Prefer null over guessed selectors, generic wrappers, layout containers, or whole-page content selectors. For description, use null unless a precise product-description selector is visible; do not use generic selectors such as .description-wrapper or .HTMLPageContent unless those exact classes appear in the YAML on nodes containing product description text.";
const CONFIDENCE_INSTRUCTION: &str = "Use confidence HIGH only when selectors are product-specific and likely safe for unattended approval after deterministic validation. Use MEDIUM for plausible schemas with ambiguity. Use LOW when selectors or fields are uncertain. MEDIUM and LOW require human review.";

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
        "Generate robust Extraction-Schemas that together cover these product page HTML samples from the same shop.\n\
         {template_instruction}\
         Shops often have multiple templates/layouts. Do not collapse different templates into one overly broad schema just because fields share names.\n\
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
         Return ONLY ProductSchemaGenerationResponse JSON with fields schemas, confidence, summary, and risks. The schemas field contains one ProductCssSelectorSchema for one product template or multiple schemas ordered as described above.\n\
         {confidence_instruction}\n\
         {sample_description}\n\
         Here are the page {sample_label} samples:{samples}",
        selector_grounding_instruction = SELECTOR_GROUNDING_INSTRUCTION,
        confidence_instruction = CONFIDENCE_INSTRUCTION,
        sample_description = SAMPLE_DESCRIPTION,
        sample_label = SAMPLE_LABEL
    )
}

pub(super) fn build_append_schema_instruction(html: &str) -> String {
    let prompt_page = html_to_schema_prompt_dsl(html);

    format!(
        "Classify the following HTML from a URL expected to be a product page, then return one append response.\n\
          The page may be a product page, a removed/404-like product page, or a wrong URL type.\n\
          Choose page_kind = product when the page is a real product detail page and return exactly one ProductCssSelectorSchema in schemas.\n\
          Choose page_kind = removed when the page is a removed, gone, not-found, unavailable, deleted, or 404-like page for a product URL served with HTTP 200. Return no product schemas and set removed_schema to a selector+exact visible text snippet that proves the removed state.\n\
          Choose page_kind = not_product when the page is clearly a category, search, home, info, navigation, listing, or other non-product page, and not a removed product page. Return no product schemas, set non_product_schema to a selector+exact visible text snippet that proves the page type, and include a short reason.\n\
          For removed and not_product, selector and text must both match the original raw HTML: the CSS selector must select at least one element, and the selected normalized text must contain the returned exact visible text after trimming, whitespace collapse, and lowercasing.\n\
          For product, this schema will be appended to a set of existing schemas from the same shop. Focus on this specific layout and make the selectors resilient.\n\
          {selector_grounding_instruction}\n\
          Return ONLY ProductSchemaGenerationResponse JSON.\n\
          {confidence_instruction}\n\
          {sample_description}\n\
          Here is the page {sample_label}:\n\
          {prompt_page}",
        selector_grounding_instruction = SELECTOR_GROUNDING_INSTRUCTION,
        confidence_instruction = CONFIDENCE_INSTRUCTION,
        sample_description = SAMPLE_DESCRIPTION,
        sample_label = SAMPLE_LABEL
    )
}
