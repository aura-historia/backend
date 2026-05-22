use crate::scraper::css_selector::product_schema::ApplySchemaError;
use crate::scraper::css_selector::rule::ExtractionError;
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
        NormalizationError::NoValidImages { .. } => Some(ApplySchemaError::Images(
            ExtractionError::NoElementMatched {
                selector: "images".to_string(),
            },
        )),
        _ => None,
    }
}
