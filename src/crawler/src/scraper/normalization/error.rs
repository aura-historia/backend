use crate::scraper::normalization::state_mapping_service::StateMappingServiceError;

#[derive(Debug, thiserror::Error)]
pub enum NormalizationError {
    #[error("failed to resolve product state: {0}")]
    StateMappingError(#[from] StateMappingServiceError),

    #[error("failed to normalize `shops_product_id`: value is empty after trimming")]
    ShopsProductIdEmpty,

    #[error("failed to normalize `title`: value is empty after trimming")]
    TitleEmpty,

    #[error("failed to normalize `title`: could not detect language of '{text}'")]
    TitleUnknownLanguage { text: String },

    #[error("failed to normalize `description`: could not detect language of '{text}'")]
    DescriptionUnknownLanguage { text: String },

    #[error("failed to normalize `price`: could not detect currency in '{raw}'")]
    PriceUnknownCurrency { raw: String },

    #[error("failed to normalize `price`: could not parse '{raw}' as a monetary amount")]
    PriceParseError { raw: String },

    #[error("failed to normalize `price_estimate_min`: could not detect currency in '{raw}'")]
    PriceEstimateMinUnknownCurrency { raw: String },

    #[error(
        "failed to normalize `price_estimate_min`: could not parse '{raw}' as a monetary amount"
    )]
    PriceEstimateMinParseError { raw: String },

    #[error("failed to normalize `price_estimate_max`: could not detect currency in '{raw}'")]
    PriceEstimateMaxUnknownCurrency { raw: String },

    #[error(
        "failed to normalize `price_estimate_max`: could not parse '{raw}' as a monetary amount"
    )]
    PriceEstimateMaxParseError { raw: String },

    #[error("failed to normalize `images`: invalid URL '{raw}': {source}")]
    InvalidImageUrl {
        raw: String,
        #[source]
        source: url::ParseError,
    },

    #[error("failed to normalize `auction_start`: could not parse '{raw}' as a date/time")]
    AuctionStartParseError { raw: String },

    #[error("failed to normalize `auction_end`: could not parse '{raw}' as a date/time")]
    AuctionEndParseError { raw: String },

    #[error(
        "failed to normalize `state`: extracted text is too long ({len} bytes, max {max}) — CSS selector likely extracting wrong content"
    )]
    StateTextTooLong { len: usize, max: usize },
}
