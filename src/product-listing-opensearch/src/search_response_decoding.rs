use crate::product_listing_document::ProductListingDocument;
use application::error::BoxError;
use opensearch::http::StatusCode;
use opensearch::http::response::Response;
use platform_opensearch::response::{OpenSearchResponseReadError, read_response};
use platform_opensearch::search_response::{OpenSearchTimedOutError, SearchResponse};

/// Why a ProductListing `_search` response is unusable, carrying its original cause. Each reader
/// maps these classes onto its own port error.
#[derive(Debug)]
pub(crate) enum SearchResponseError {
    Read(OpenSearchResponseReadError),
    UnsuccessfulStatus(UnsuccessfulSearchStatus),
    Undecodable(serde_json::Error),
    TimedOut(OpenSearchTimedOutError),
}

impl SearchResponseError {
    /// The original cause, so the port error's chain does not gain a classification layer.
    pub(crate) fn into_source(self) -> BoxError {
        match self {
            Self::Read(source) => source.into(),
            Self::UnsuccessfulStatus(source) => source.into(),
            Self::Undecodable(source) => source.into(),
            Self::TimedOut(source) => source.into(),
        }
    }
}

/// Status-only context for an unsuccessful response; the provider's error body is never kept.
#[derive(Debug, thiserror::Error)]
#[error("OpenSearch {request_kind} returned unsuccessful status {status}")]
pub(crate) struct UnsuccessfulSearchStatus {
    request_kind: &'static str,
    status: StatusCode,
}

pub(crate) async fn read_search_response(
    response: Response,
    request_kind: &'static str,
) -> Result<SearchResponse<ProductListingDocument>, SearchResponseError> {
    let response = read_response(response)
        .await
        .map_err(SearchResponseError::Read)?;
    decode_search_response(response.status(), response.body(), request_kind)
}

fn decode_search_response(
    status: StatusCode,
    payload: &str,
    request_kind: &'static str,
) -> Result<SearchResponse<ProductListingDocument>, SearchResponseError> {
    if !status.is_success() {
        return Err(SearchResponseError::UnsuccessfulStatus(
            UnsuccessfulSearchStatus {
                request_kind,
                status,
            },
        ));
    }
    serde_json::from_str::<SearchResponse<ProductListingDocument>>(payload)
        .map_err(SearchResponseError::Undecodable)?
        .into_non_timed_out(request_kind)
        .map_err(SearchResponseError::TimedOut)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn should_keep_only_status_context_when_search_response_is_unsuccessful() {
        let result = decode_search_response(
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{"error":"PRIVATE_PROVIDER_BODY"}"#,
            "product search",
        );

        let Err(SearchResponseError::UnsuccessfulStatus(error)) = result else {
            panic!("an unsuccessful status must be classified as such");
        };
        assert_eq!(
            "OpenSearch product search returned unsuccessful status 503 Service Unavailable",
            error.to_string()
        );
        assert!(std::error::Error::source(&error).is_none());
    }

    #[test]
    fn should_preserve_decoding_cause_when_search_response_is_malformed() {
        let result = decode_search_response(StatusCode::OK, "{", "product search");

        let Err(error @ SearchResponseError::Undecodable(_)) = result else {
            panic!("a malformed payload must be classified as undecodable");
        };
        assert!(
            error
                .into_source()
                .downcast_ref::<serde_json::Error>()
                .is_some()
        );
    }

    #[test]
    fn should_preserve_timeout_details_when_search_engine_times_out() {
        let payload = json!({
            "took": 30,
            "timed_out": true,
            "_shards": {"total": 2, "successful": 1, "skipped": 0, "failed": 1},
            "hits": {"total": {"value": 0, "relation": "eq"}, "max_score": null, "hits": []}
        })
        .to_string();

        let result = decode_search_response(StatusCode::OK, &payload, "product hybrid search");

        let Err(error @ SearchResponseError::TimedOut(_)) = result else {
            panic!("an engine timeout must be classified as timed out");
        };
        assert_eq!(
            Some(&OpenSearchTimedOutError {
                request_kind: "product hybrid search",
                took: 30,
                total_shards: 2,
                successful_shards: 1,
                skipped_shards: 0,
                failed_shards: 1,
            }),
            error
                .into_source()
                .downcast_ref::<OpenSearchTimedOutError>()
        );
    }
}
