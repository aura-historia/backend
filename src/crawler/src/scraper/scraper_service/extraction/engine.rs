use crate::scraper::css_selector::product_schema::{
    ApplySchemaError, ProductCssSelectorSchema, RawExtractedProduct,
};
use crate::scraper::css_selector::rule::ExtractionError;
use scraper::Html;

/// Applies `schema` to an already-parsed `html` document.
///
/// Use this when the same document is queried by multiple schemas so the HTML
/// is parsed only once. Retains the same raw extraction semantics as
/// [`apply_schema`], including image extraction behavior.
pub(crate) fn apply_schema_to_document(
    schema: &ProductCssSelectorSchema,
    html: &Html,
) -> Result<RawExtractedProduct, ApplySchemaError> {
    let mut raw = schema.apply(html)?;
    raw.images = schema
        .apply_image_url_candidate_groups(html)
        .map_err(ApplySchemaError::Images)?;
    Ok(raw)
}

/// Applies `schema` to `html` synchronously (`scraper::Html` is `!Send`).
pub(crate) fn apply_schema(
    schema: &ProductCssSelectorSchema,
    html: &str,
) -> Result<RawExtractedProduct, ApplySchemaError> {
    let parsed_html = Html::parse_document(html);
    apply_schema_to_document(schema, &parsed_html)
}

/// Tries every schema variant in `schemas` in sequence and returns the first
/// one that successfully extracts a [`RawExtractedProduct`].
pub(crate) fn try_apply_schemas<'a, I>(
    schemas: I,
    html: &str,
) -> Result<(ProductCssSelectorSchema, RawExtractedProduct), ApplySchemaError>
where
    I: IntoIterator<Item = &'a ProductCssSelectorSchema>,
{
    let mut last_error: Option<ApplySchemaError> = None;
    for schema in schemas {
        match apply_schema(schema, html) {
            Ok(raw) => return Ok((schema.clone(), raw)),
            Err(err) => {
                last_error = Some(err);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        ApplySchemaError::Title(ExtractionError::NoElementMatched {
            selector: "title".to_string(),
        })
    }))
}
