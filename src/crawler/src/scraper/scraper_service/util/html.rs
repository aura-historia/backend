use crate::scraper::css_selector::product_schema::ApplySchemaError;
use crate::scraper::css_selector::rule::ExtractionError;
use crate::scraper::normalization::error::NormalizationError;

/// Byte-level case-insensitive substring search.  Returns the byte offset of
/// the first occurrence of `search` in `text`, or `None`.
pub(crate) fn find_case_insensitive(text: &str, search: &str) -> Option<usize> {
    let search_bytes = search.as_bytes();
    if search_bytes.is_empty() {
        return Some(0);
    }
    text.as_bytes()
        .windows(search_bytes.len())
        .position(|window| window.eq_ignore_ascii_case(search_bytes))
}

/// Extracts the raw string content between `<main …>` and `</main>`, or
/// `None` if no `<main>` tag is present.
pub(crate) fn extract_main_fragment(html: &str) -> Option<&str> {
    let main_start = find_case_insensitive(html, "<main")?;
    let tag_end_rel = html[main_start..].find('>')?;
    let content_start = main_start + tag_end_rel + 1;
    let main_end_rel = find_case_insensitive(&html[content_start..], "</main>")?;
    let content_end = content_start + main_end_rel;
    Some(&html[content_start..content_end])
}

/// Maps a [`NormalizationError`] to the [`ApplySchemaError`] hint that should
/// be forwarded to schema regeneration, or `None` when the error is not
/// fixable via schema changes.
pub(crate) fn normalization_error_to_schema_hint(
    err: &NormalizationError,
) -> Option<ApplySchemaError> {
    match err {
        NormalizationError::PriceUnknownCurrency { .. }
        | NormalizationError::PriceParseError { .. } => {
            Some(ApplySchemaError::Price(ExtractionError::NoElementMatched {
                selector: "price".to_string(),
            }))
        }
        NormalizationError::PriceEstimateMinUnknownCurrency { .. }
        | NormalizationError::PriceEstimateMinParseError { .. } => Some(
            ApplySchemaError::PriceEstimateMin(ExtractionError::NoElementMatched {
                selector: "price_estimate_min".to_string(),
            }),
        ),
        NormalizationError::PriceEstimateMaxUnknownCurrency { .. }
        | NormalizationError::PriceEstimateMaxParseError { .. } => Some(
            ApplySchemaError::PriceEstimateMax(ExtractionError::NoElementMatched {
                selector: "price_estimate_max".to_string(),
            }),
        ),
        NormalizationError::TitleEmpty | NormalizationError::TitleUnknownLanguage { .. } => {
            Some(ApplySchemaError::Title(ExtractionError::NoElementMatched {
                selector: "title".to_string(),
            }))
        }
        NormalizationError::StateTextTooLong { .. } => {
            Some(ApplySchemaError::State(ExtractionError::NoElementMatched {
                selector: "state".to_string(),
            }))
        }
        _ => None,
    }
}
