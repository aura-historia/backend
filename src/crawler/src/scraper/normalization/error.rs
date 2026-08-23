use crate::scraper::normalization::state_mapping_service::StateMappingServiceError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NormalizationFailureScope {
    CandidateData,
    External,
}

#[derive(Debug, thiserror::Error)]
pub enum NormalizationError {
    #[error("failed to resolve product state: {0}")]
    StateMappingError(#[from] StateMappingServiceError),

    /// Emitted by the strict (test-only) `normalize_shops_product_id` function
    /// when the extracted value is empty after trimming.
    ///
    /// **Unreachable from the main pipeline.**  The production normalization
    /// path calls `normalize_shops_product_id_with_url_sha_fallback` instead,
    /// which substitutes a SHA-256 hash of the product page URL when the
    /// extracted ID is blank.
    /// This variant is retained so that unit tests can exercise the strict
    /// variant.
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

    #[error(
        "failed to normalize `images`: no valid image remained after validating {candidates} candidate(s)"
    )]
    NoValidImages { candidates: usize },

    #[error("failed to normalize `auction_start`: could not parse '{raw}' as a date/time")]
    AuctionStartParseError { raw: String },

    #[error("failed to normalize `auction_end`: could not parse '{raw}' as a date/time")]
    AuctionEndParseError { raw: String },

    #[error(
        "failed to normalize `state`: extracted text is too long ({len} bytes, max {max}) — CSS selector likely extracting wrong content"
    )]
    StateTextTooLong { len: usize, max: usize },
}

impl NormalizationError {
    pub(crate) const fn failure_reason(&self) -> &'static str {
        match self {
            Self::StateMappingError(error) => match error {
                StateMappingServiceError::LargeLanguageModelError(_) => "state_llm_error",
                StateMappingServiceError::UnparsableResponse => "state_unparsable_response",
                StateMappingServiceError::ResponseJsonSchemaSerialization(_) => {
                    "state_response_schema_serialization"
                }
                StateMappingServiceError::RawStateTooLong { .. } => "state_text_too_long",
                StateMappingServiceError::DatabaseError(_) => "state_database_error",
                StateMappingServiceError::DatabaseErrorAfterLlm(_) => {
                    "state_database_error_after_llm"
                }
            },
            Self::ShopsProductIdEmpty => "shops_product_id_empty",
            Self::TitleEmpty => "title_empty",
            Self::TitleUnknownLanguage { .. } => "title_unknown_language",
            Self::DescriptionUnknownLanguage { .. } => "description_unknown_language",
            Self::PriceUnknownCurrency { .. } => "price_unknown_currency",
            Self::PriceParseError { .. } => "price_parse_error",
            Self::PriceEstimateMinUnknownCurrency { .. } => "price_estimate_min_unknown_currency",
            Self::PriceEstimateMinParseError { .. } => "price_estimate_min_parse_error",
            Self::PriceEstimateMaxUnknownCurrency { .. } => "price_estimate_max_unknown_currency",
            Self::PriceEstimateMaxParseError { .. } => "price_estimate_max_parse_error",
            Self::InvalidImageUrl { .. } => "invalid_image_url",
            Self::NoValidImages { .. } => "no_valid_images",
            Self::AuctionStartParseError { .. } => "auction_start_parse_error",
            Self::AuctionEndParseError { .. } => "auction_end_parse_error",
            Self::StateTextTooLong { .. } => "state_text_too_long",
        }
    }

    pub(crate) const fn failure_scope(&self) -> NormalizationFailureScope {
        match self {
            Self::StateMappingError(error) => match error {
                StateMappingServiceError::LargeLanguageModelError(_)
                | StateMappingServiceError::UnparsableResponse
                | StateMappingServiceError::ResponseJsonSchemaSerialization(_)
                | StateMappingServiceError::DatabaseError(_)
                | StateMappingServiceError::DatabaseErrorAfterLlm(_) => {
                    NormalizationFailureScope::External
                }
                StateMappingServiceError::RawStateTooLong { .. } => {
                    NormalizationFailureScope::CandidateData
                }
            },
            Self::ShopsProductIdEmpty
            | Self::TitleEmpty
            | Self::TitleUnknownLanguage { .. }
            | Self::DescriptionUnknownLanguage { .. }
            | Self::PriceUnknownCurrency { .. }
            | Self::PriceParseError { .. }
            | Self::PriceEstimateMinUnknownCurrency { .. }
            | Self::PriceEstimateMinParseError { .. }
            | Self::PriceEstimateMaxUnknownCurrency { .. }
            | Self::PriceEstimateMaxParseError { .. }
            | Self::InvalidImageUrl { .. }
            | Self::NoValidImages { .. }
            | Self::AuctionStartParseError { .. }
            | Self::AuctionEndParseError { .. }
            | Self::StateTextTooLong { .. } => NormalizationFailureScope::CandidateData,
        }
    }
}
