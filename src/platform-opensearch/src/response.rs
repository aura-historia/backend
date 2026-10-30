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
/// The message names only the status. Rejection bodies can quote document fields, so the body
/// stays a separate field for local diagnosis and never becomes part of the error text.
#[derive(Debug, thiserror::Error)]
#[error("OpenSearch returned unsuccessful status {status}")]
pub struct OpenSearchStatusError {
    pub status: StatusCode,
    pub body: String,
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
