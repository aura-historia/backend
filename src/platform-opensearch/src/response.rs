use opensearch::http::{StatusCode, response::Response};

/// A fully read OpenSearch response.
#[derive(Debug)]
pub struct OpenSearchResponse {
    status: StatusCode,
    body: String,
}

impl OpenSearchResponse {
    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn body(&self) -> &str {
        &self.body
    }

    pub fn into_body(self) -> String {
        self.body
    }

    /// Converts a response its caller does not accept into an error that keeps the status
    /// separately from the body.
    pub fn into_status_error(self) -> OpenSearchStatusError {
        OpenSearchStatusError {
            status: self.status,
            body: self.body,
        }
    }
}

/// OpenSearch answered with a status the caller does not accept.
///
/// Rejection bodies can quote document fields, so neither the message nor `Debug` includes the
/// body; it stays a separate field for controlled diagnosis.
#[derive(thiserror::Error)]
#[error("OpenSearch returned unsuccessful status {status}")]
pub struct OpenSearchStatusError {
    pub status: StatusCode,
    pub body: String,
}

impl std::fmt::Debug for OpenSearchStatusError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenSearchStatusError")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("could not read OpenSearch response body for status {status}")]
pub struct OpenSearchResponseReadError {
    pub status: StatusCode,
    #[source]
    pub source: opensearch::Error,
}

/// Reads the response body before callers classify its HTTP status.
///
/// This preserves unsuccessful response payloads for adapter-specific errors. If reading the
/// body fails, the response status and underlying client error remain available.
pub async fn read_response(
    response: Response,
) -> Result<OpenSearchResponse, OpenSearchResponseReadError> {
    let status = response.status_code();
    let body = response
        .text()
        .await
        .map_err(|source| OpenSearchResponseReadError { status, source })?;

    Ok(OpenSearchResponse { status, body })
}
