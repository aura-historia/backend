use crate::scraper::normalization::error::NormalizationError;
use scraper::Selector;
use std::sync::OnceLock;

static MAIN_SEL: OnceLock<Selector> = OnceLock::new();

fn main_selector() -> &'static Selector {
    MAIN_SEL.get_or_init(|| Selector::parse("main").expect("valid selector"))
}

pub(crate) fn extract_main_fragment(html: &str) -> Option<String> {
    let document = scraper::Html::parse_document(html);
    document
        .select(main_selector())
        .next()
        .map(|el| el.inner_html())
}

/// Returns true when the error can be caused by a schema-specific extraction.
pub(crate) fn is_schema_specific_normalization_error(err: &NormalizationError) -> bool {
    matches!(
        err,
        NormalizationError::PriceUnknownCurrency { .. }
            | NormalizationError::PriceParseError { .. }
            | NormalizationError::PriceEstimateMinUnknownCurrency { .. }
            | NormalizationError::PriceEstimateMinParseError { .. }
            | NormalizationError::PriceEstimateMaxUnknownCurrency { .. }
            | NormalizationError::PriceEstimateMaxParseError { .. }
            | NormalizationError::TitleEmpty
            | NormalizationError::TitleUnknownLanguage { .. }
            | NormalizationError::StateTextTooLong { .. }
    )
}
