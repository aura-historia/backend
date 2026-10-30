use crate::response::{OpenSearchResponseReadError, OpenSearchStatusError};
use opensearch::http::StatusCode;
use std::error::Error;

/// Bounded, loggable facts about the first OpenSearch failure in an error chain.
///
/// The kind is an allowlisted label and the status a standardized code. Neither carries the
/// error text, which can quote response bodies, document fields or endpoint details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenSearchFailure {
    kind: &'static str,
    status: Option<StatusCode>,
}

impl OpenSearchFailure {
    /// Returns `None` when the chain holds no OpenSearch client or response error, for example
    /// when an adapter rejected its own document before sending it.
    pub fn find(error: &(dyn Error + 'static)) -> Option<Self> {
        std::iter::successors(Some(error), |error| (*error).source()).find_map(Self::classify)
    }

    pub fn kind(&self) -> &'static str {
        self.kind
    }

    pub fn http_status(&self) -> Option<u16> {
        self.status.map(|status| status.as_u16())
    }

    fn classify(error: &(dyn Error + 'static)) -> Option<Self> {
        if let Some(error) = error.downcast_ref::<OpenSearchStatusError>() {
            return Some(Self {
                kind: "http_status",
                status: Some(error.status),
            });
        }
        if let Some(error) = error.downcast_ref::<OpenSearchResponseReadError>() {
            return Some(Self {
                kind: "response_read",
                status: Some(error.status),
            });
        }
        let error = error.downcast_ref::<opensearch::Error>()?;
        let kind = if error.is_timeout() {
            "timeout"
        } else if error.is_json() {
            "response_invalid"
        } else if error.status_code().is_some() {
            "http_status"
        } else {
            "transport"
        };
        Some(Self {
            kind,
            status: error.status_code(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENTINEL: &str = "SENTINEL_DOCUMENT_FIELD";

    #[derive(Debug, thiserror::Error)]
    #[error("projection write failed")]
    struct WriteFailed(#[source] Box<dyn Error + Send + Sync>);

    #[test]
    fn should_find_the_status_of_a_rejected_response_without_its_body() {
        let error = WriteFailed(Box::new(OpenSearchStatusError {
            status: StatusCode::FORBIDDEN,
            body: SENTINEL.to_owned(),
        }));

        let failure = OpenSearchFailure::find(&error);

        assert_eq!(Some("http_status"), failure.map(|failure| failure.kind()));
        assert_eq!(Some(403), failure.and_then(|failure| failure.http_status()));
        assert!(!error.0.to_string().contains(SENTINEL));
        assert!(!format!("{error:?}").contains(SENTINEL));
    }

    #[test]
    fn should_classify_a_client_failure_without_a_status_as_transport() {
        let error = WriteFailed(Box::new(opensearch::Error::from(std::io::Error::other(
            SENTINEL,
        ))));

        let failure = OpenSearchFailure::find(&error);

        assert_eq!(Some("transport"), failure.map(|failure| failure.kind()));
        assert_eq!(None, failure.and_then(|failure| failure.http_status()));
    }

    #[test]
    fn should_find_nothing_when_no_opensearch_error_is_in_the_chain() {
        let error = WriteFailed(Box::new(std::io::Error::other(SENTINEL)));

        assert_eq!(None, OpenSearchFailure::find(&error));
    }
}
