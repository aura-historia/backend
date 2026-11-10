use application::error::BoxError;
use rmcp::{
    ErrorData,
    model::{CallToolResult, ContentBlock},
};

/// Longest validation reason sent back to a caller, so an echoed argument cannot inflate a reply.
const MAX_REASON_CHARS: usize = 300;

/// Family adapters classify service errors without exposing their technical causes.
///
/// Missing-resource and conflict outcomes carry a static message that names the resource and a
/// next step. It is `&'static str` on purpose: it can never include request data or internals.
#[derive(Debug, thiserror::Error)]
pub enum OperationError {
    #[error("invalid arguments")]
    InvalidArguments(#[source] BoxError),
    #[error("operation input is invalid")]
    InvalidInput,
    #[error("resource not found")]
    NotFound(&'static str),
    #[error("operation conflicts with current state")]
    Conflict {
        message: &'static str,
        #[source]
        source: Option<BoxError>,
    },
    #[error("saved-search quota exceeded")]
    SearchFilterQuotaExceeded,
    #[error("saved-search feature requires a higher tier")]
    SearchFilterRestrictedFeature,
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

/// A rejected argument: its name and the validation reason. The reason is the client-safe text
/// REST returns as problem detail for the same validation, never a technical cause.
#[derive(Debug, thiserror::Error)]
#[error("{field}: {source}")]
struct ArgumentError {
    field: &'static str,
    #[source]
    source: BoxError,
}

impl OperationError {
    /// A conflict with the current state, described to the caller without a technical cause.
    pub fn conflict(message: &'static str) -> Self {
        Self::Conflict {
            message,
            source: None,
        }
    }

    /// A rejected argument whose name the validation knows, such as `sort` or `auctionId`.
    pub fn invalid_argument(
        field: &'static str,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::InvalidArguments(Box::new(ArgumentError {
            field,
            source: Box::new(source),
        }))
    }

    /// The text blocks of the tool error a caller receives, the code first.
    #[cfg(test)]
    pub(crate) fn tool_error_texts(self) -> Vec<String> {
        self.into_protocol()
            .expect("a tool error, not a protocol error")
            .content
            .iter()
            .map(|block| block.as_text().unwrap().text.clone())
            .collect()
    }

    pub(crate) fn into_protocol(self) -> Result<CallToolResult, ErrorData> {
        let (code, message) = match self {
            Self::InvalidArguments(source) => {
                return Err(ErrorData::invalid_params(
                    format!("Invalid tool arguments: {}", argument_reason(&*source)),
                    None,
                ));
            }
            Self::InvalidInput => ("INVALID_INPUT", None),
            Self::NotFound(message) => ("NOT_FOUND", Some(message)),
            Self::Conflict { message, .. } => ("CONFLICT", Some(message)),
            Self::WatchlistQuotaExceeded => ("WATCHLIST_QUOTA_EXCEEDED", None),
            Self::Forbidden => ("FORBIDDEN", None),
            Self::SearchFilterQuotaExceeded => ("SEARCH_FILTER_QUOTA_EXCEEDED", None),
            Self::SearchFilterRestrictedFeature => ("SEARCH_FILTER_RESTRICTED_FEATURE", None),
            Self::Unavailable(_) => ("TEMPORARILY_UNAVAILABLE", None),
            Self::Internal(_) => return Err(ErrorData::internal_error("Operation failed", None)),
            Self::ResultTooLarge => {
                return Err(ErrorData::internal_error(
                    "Result exceeds transport limit; narrow the request",
                    None,
                ));
            }
        };
        // The code stays the first block for clients that match on it; the message follows.
        let mut content = vec![ContentBlock::text(code)];
        content.extend(message.map(ContentBlock::text));
        Ok(CallToolResult::error(content))
    }
}

/// The reason to show for a rejected argument: for a decoding failure its value-free reason, for
/// a validation error its own text, without the decoder position and bounded in length.
fn argument_reason(source: &(dyn std::error::Error + 'static)) -> String {
    let text = match source.downcast_ref::<crate::arguments::DecodeError>() {
        Some(decode) => decode.reason(),
        None => source.to_string(),
    };
    crate::arguments::without_position(&text)
        .chars()
        .take(MAX_REASON_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(result: &CallToolResult) -> Vec<&str> {
        result
            .content
            .iter()
            .map(|block| block.as_text().unwrap().text.as_str())
            .collect()
    }

    fn invalid_params_message(error: OperationError) -> String {
        let error = error.into_protocol().unwrap_err();
        assert_eq!(-32602, error.code.0);
        error.message.into_owned()
    }

    #[test]
    fn should_follow_the_not_found_code_with_its_message() {
        let result = OperationError::NotFound("Listing not found: check the productListingId.")
            .into_protocol()
            .unwrap();

        assert_eq!(Some(true), result.is_error);
        assert_eq!(
            vec![
                "NOT_FOUND",
                "Listing not found: check the productListingId."
            ],
            texts(&result)
        );
    }

    #[test]
    fn should_follow_the_conflict_code_with_its_message_and_never_the_cause() {
        let error = OperationError::Conflict {
            message: "Stale expectedVersion: re-read the address and retry.",
            source: Some(std::io::Error::other("private token secret").into()),
        };

        let result = error.into_protocol().unwrap();

        assert_eq!(
            vec![
                "CONFLICT",
                "Stale expectedVersion: re-read the address and retry."
            ],
            texts(&result)
        );
    }

    #[test]
    fn should_keep_codes_without_a_message_as_a_single_block() {
        for (error, code) in [
            (OperationError::Forbidden, "FORBIDDEN"),
            (OperationError::InvalidInput, "INVALID_INPUT"),
            (
                OperationError::WatchlistQuotaExceeded,
                "WATCHLIST_QUOTA_EXCEEDED",
            ),
        ] {
            assert_eq!(vec![code], texts(&error.into_protocol().unwrap()));
        }
    }

    #[test]
    fn should_name_the_argument_and_reason_of_a_rejected_argument() {
        let error = OperationError::invalid_argument(
            "excludeProductListingId",
            std::io::Error::other("must be a valid ProductListing ID"),
        );

        assert_eq!(
            "Invalid tool arguments: excludeProductListingId: must be a valid ProductListing ID",
            invalid_params_message(error)
        );
    }

    #[test]
    fn should_give_the_reason_of_a_validation_error_without_the_decoder_position() {
        let decode = serde_json::from_str::<u64>("\"x\"").unwrap_err();
        assert!(decode.to_string().contains(" at line 1 column "));

        let message = invalid_params_message(OperationError::InvalidArguments(Box::new(decode)));

        assert!(
            message.starts_with("Invalid tool arguments: invalid type"),
            "{message}"
        );
        assert!(!message.contains(" at line "), "{message}");
    }

    #[test]
    fn should_keep_a_reason_that_only_resembles_a_position() {
        let message = invalid_params_message(OperationError::InvalidArguments(
            std::io::Error::other("see the entry at line two").into(),
        ));

        assert_eq!("Invalid tool arguments: see the entry at line two", message);
    }

    #[test]
    fn should_bound_the_length_of_the_reason() {
        let message = invalid_params_message(OperationError::InvalidArguments(
            std::io::Error::other("x".repeat(10_000)).into(),
        ));

        assert_eq!(
            "Invalid tool arguments: ".len() + MAX_REASON_CHARS,
            message.len()
        );
    }
}
