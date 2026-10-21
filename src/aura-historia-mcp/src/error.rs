use application::error::BoxError;
use rmcp::{
    ErrorData,
    model::{CallToolResult, ContentBlock},
};

/// Family adapters classify service errors without exposing their technical causes.
#[derive(Debug, thiserror::Error)]
pub enum OperationError {
    #[error("invalid arguments")]
    InvalidArguments(#[source] BoxError),
    #[error("operation input is invalid")]
    InvalidInput,
    #[error("resource not found")]
    NotFound,
    #[error("operation conflicts with current state")]
    Conflict,
    #[error("watchlist quota exceeded")]
    WatchlistQuotaExceeded,
    #[error("operation is not permitted")]
    Forbidden,
    #[error("service temporarily unavailable")]
    Unavailable(#[source] BoxError),
    #[error("operation failed")]
    Internal(#[source] BoxError),
    #[error("result exceeds the transport limit")]
    ResultTooLarge,
}

impl OperationError {
    pub(crate) fn into_protocol(self) -> Result<CallToolResult, ErrorData> {
        let code = match self {
            Self::InvalidArguments(_) => {
                return Err(ErrorData::invalid_params("Invalid tool arguments", None));
            }
            Self::InvalidInput => "INVALID_INPUT",
            Self::NotFound => "NOT_FOUND",
            Self::Conflict => "CONFLICT",
            Self::WatchlistQuotaExceeded => "WATCHLIST_QUOTA_EXCEEDED",
            Self::Forbidden => "FORBIDDEN",
            Self::Unavailable(_) => "TEMPORARILY_UNAVAILABLE",
            Self::Internal(_) => return Err(ErrorData::internal_error("Operation failed", None)),
            Self::ResultTooLarge => {
                return Err(ErrorData::internal_error(
                    "Result exceeds transport limit; narrow the request",
                    None,
                ));
            }
        };
        Ok(CallToolResult::error(vec![ContentBlock::text(code)]))
    }
}
