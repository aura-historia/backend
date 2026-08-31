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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_retain_status_and_body() {
        let response = OpenSearchResponse {
            status: StatusCode::BAD_GATEWAY,
            body: "upstream failed".to_owned(),
        };

        assert_eq!(StatusCode::BAD_GATEWAY, response.status());
        assert_eq!("upstream failed", response.body());
        assert_eq!("upstream failed", response.into_body());
    }
}
