use crate::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use crate::api::error_code::ApiErrorCode;
use aws_lambda_events::apigw::ApiGatewayV2httpResponse;
use http::StatusCode;
use serde::Serialize;
use std::error::Error;
use tracing::{error, warn};

#[derive(Debug, Serialize)]
pub struct ApiError {
    pub status: u16,
    pub title: &'static str,
    pub error: ApiErrorCode,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ApiErrorSource>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,

    #[serde(skip)]
    pub cause: Option<Box<dyn Error>>,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {} - {}", self.status, self.error)?;
        if let Some(msg) = &self.detail {
            write!(f, ": {msg}")?;
        }
        if let Some(cause) = &self.cause {
            write!(f, ": {cause}")?;
        }
        Ok(())
    }
}

impl Error for ApiError {}

impl ApiError {
    pub fn new(status: StatusCode, title: &'static str, error: ApiErrorCode) -> Self {
        ApiError {
            status: status.as_u16(),
            title,
            error,
            source: None,
            detail: None,
            cause: None,
        }
    }

    pub fn with_cause(mut self, err: Box<dyn Error>) -> Self {
        self.cause = Some(err);
        self
    }

    pub fn with_source(mut self, field: ApiErrorSource) -> Self {
        self.source = Some(field);
        self
    }

    pub fn with_header_field(mut self, field: &'static str) -> Self {
        self.source = Some(ApiErrorSource {
            field,
            source_type: ApiErrorSourceType::Header,
        });
        self
    }

    pub fn with_query_field(mut self, field: &'static str) -> Self {
        self.source = Some(ApiErrorSource {
            field,
            source_type: ApiErrorSourceType::Query,
        });
        self
    }

    pub fn with_path_field(mut self, field: &'static str) -> Self {
        self.source = Some(ApiErrorSource {
            field,
            source_type: ApiErrorSourceType::Path,
        });
        self
    }
    pub fn with_body_field(mut self, field: &'static str) -> Self {
        self.source = Some(ApiErrorSource {
            field,
            source_type: ApiErrorSourceType::Body,
        });
        self
    }

    pub fn with_detail(mut self, msg: impl Into<String>) -> Self {
        self.detail = Some(msg.into());
        self
    }

    pub fn bad_request(error: ApiErrorCode, cause: Box<dyn Error>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "Bad Request", error).with_cause(cause)
    }

    pub fn unauthorized(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Unauthorized", error)
    }

    pub fn forbidden(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::FORBIDDEN, "Forbidden", error)
    }

    pub fn not_found(error: ApiErrorCode, cause: Box<dyn Error>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "Not Found", error).with_cause(cause)
    }

    pub fn conflict(error: ApiErrorCode, cause: Box<dyn Error>) -> Self {
        Self::new(StatusCode::CONFLICT, "Conflict", error).with_cause(cause)
    }

    pub fn unprocessable_entity(error: ApiErrorCode, cause: Box<dyn Error>) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Unprocessable Content",
            error,
        )
        .with_cause(cause)
    }

    pub fn too_many_requests(error: ApiErrorCode, cause: Box<dyn Error>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, "Too Many Requests", error).with_cause(cause)
    }

    pub fn internal_server_error(error: ApiErrorCode, cause: Box<dyn Error>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal Server Error",
            error,
        )
        .with_cause(cause)
    }

    pub fn bad_gateway(error: ApiErrorCode, cause: Box<dyn Error>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, "Bad Gateway", error).with_cause(cause)
    }

    pub fn service_unavailable(error: ApiErrorCode, cause: Box<dyn Error>) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Service Unavailable",
            error,
        )
        .with_cause(cause)
    }

    pub fn gateway_time_out(error: ApiErrorCode, cause: Box<dyn Error>) -> Self {
        Self::new(StatusCode::GATEWAY_TIMEOUT, "Gateway Timeout", error).with_cause(cause)
    }

    pub fn is4xx(&self) -> bool {
        self.status >= 400 && self.status <= 499
    }

    pub fn is5xx(&self) -> bool {
        self.status >= 500 && self.status <= 599
    }
}

impl From<ApiError> for ApiGatewayV2httpResponse {
    fn from(api_error: ApiError) -> Self {
        ApiGatewayV2HttpResponseBuilder::new(api_error.status.into())
            .content_type("application/problem+json")
            .body(serde_json::to_string(&api_error).unwrap())
            .build()
    }
}

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
pub struct ApiErrorSource {
    pub field: &'static str,

    #[serde(rename = "type")]
    pub source_type: ApiErrorSourceType,
}

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "UPPERCASE")]
pub enum ApiErrorSourceType {
    Header,
    Path,
    Query,
    Body,
}

pub fn log_api_error(err: &ApiError) {
    if err.is4xx() {
        match err.cause {
            None => warn!(status = err.status),
            Some(ref cause) => warn!(status = err.status, error = ?cause),
        }
    } else if err.is5xx() {
        match err.cause {
            None => error!(status = err.status),
            Some(ref cause) => error!(status = err.status, error = ?cause),
        }
    }
}

#[cfg(feature = "dynamodb")]
pub mod dynamodb {
    use crate::api::error::ApiError;
    use crate::api::error_code::{
        BAD_GATEWAY, GATEWAY_TIMEOUT, INTERNAL_SERVER_ERROR, SERVICE_UNAVAILABLE,
    };
    use aws_sdk_dynamodb::error::SdkError;

    impl<S: std::error::Error + 'static> From<SdkError<S>> for ApiError {
        fn from(e: SdkError<S>) -> Self {
            // Check if the error contains an HTTP response with a specific status code
            if let Some(raw_response) = e.raw_response() {
                let status_code = raw_response.status().as_u16();
                match status_code {
                    502 => return ApiError::bad_gateway(BAD_GATEWAY, Box::new(e)),
                    503 => return ApiError::service_unavailable(SERVICE_UNAVAILABLE, Box::new(e)),
                    504 => return ApiError::gateway_time_out(GATEWAY_TIMEOUT, Box::new(e)),
                    _ => {} // Continue to match on error type
                }
            }

            match e {
                SdkError::ConstructionFailure(_) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(e))
                }
                SdkError::TimeoutError(_) => {
                    ApiError::gateway_time_out(GATEWAY_TIMEOUT, Box::new(e))
                }
                SdkError::DispatchFailure(_) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(e))
                }
                SdkError::ResponseError(_) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(e))
                }
                SdkError::ServiceError(_) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(e))
                }
                _ => ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(e)),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use aws_sdk_dynamodb::config::http::HttpResponse;
        use aws_sdk_dynamodb::error::ConnectorError;
        use aws_sdk_dynamodb::operation::get_item::GetItemError;

        #[rstest::rstest]
        #[trace]
        #[case::construction_failure(
            SdkError::construction_failure("Something went wrong"),
            500,
            "INTERNAL_SERVER_ERROR"
        )]
        #[case::timeout_error(
            SdkError::timeout_error("Something went wrong"),
            504,
            "GATEWAY_TIMEOUT"
        )]
        #[case::dispatch_failure(
            SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())),
            500,
            "INTERNAL_SERVER_ERROR"
        )]
        fn should_convert_sdk_error_to_api_error_when_no_http_response_for_errors_without_status(
            #[case] sdk_error: SdkError<GetItemError>,
            #[case] expected_status: u16,
            #[case] expected_error_code: &str,
        ) {
            let api_error: ApiError = sdk_error.into();
            assert_eq!(api_error.status, expected_status);
            assert_eq!(api_error.error.as_str(), expected_error_code);
        }

        #[rstest::rstest]
        #[trace]
        #[case::response_error_502(
            SdkError::response_error(
                "Something went wrong",
                HttpResponse::new(502u16.try_into().unwrap(), "{}".into())
            ),
            502,
            "BAD_GATEWAY"
        )]
        #[case::response_error_503(
            SdkError::response_error(
                "Something went wrong",
                HttpResponse::new(503u16.try_into().unwrap(), "{}".into())
            ),
            503,
            "SERVICE_UNAVAILABLE"
        )]
        #[case::response_error_504(
            SdkError::response_error(
                "Something went wrong",
                HttpResponse::new(504u16.try_into().unwrap(), "{}".into())
            ),
            504,
            "GATEWAY_TIMEOUT"
        )]
        #[case::response_error_500(
            SdkError::response_error(
                "Something went wrong",
                HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
            ),
            500,
            "INTERNAL_SERVER_ERROR"
        )]
        #[case::service_error_502(
            SdkError::service_error(
                GetItemError::unhandled("Something went wrong"),
                HttpResponse::new(502u16.try_into().unwrap(), "{}".into())
            ),
            502,
            "BAD_GATEWAY"
        )]
        #[case::service_error_503(
            SdkError::service_error(
                GetItemError::unhandled("Something went wrong"),
                HttpResponse::new(503u16.try_into().unwrap(), "{}".into())
            ),
            503,
            "SERVICE_UNAVAILABLE"
        )]
        #[case::service_error_504(
            SdkError::service_error(
                GetItemError::unhandled("Something went wrong"),
                HttpResponse::new(504u16.try_into().unwrap(), "{}".into())
            ),
            504,
            "GATEWAY_TIMEOUT"
        )]
        #[case::service_error_500(
            SdkError::service_error(
                GetItemError::unhandled("Something went wrong"),
                HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
            ),
            500,
            "INTERNAL_SERVER_ERROR"
        )]
        fn should_convert_sdk_error_to_api_error_when_http_response_present(
            #[case] sdk_error: SdkError<GetItemError>,
            #[case] expected_status: u16,
            #[case] expected_error_code: &str,
        ) {
            let api_error: ApiError = sdk_error.into();
            assert_eq!(api_error.status, expected_status);
            assert_eq!(api_error.error.as_str(), expected_error_code);
        }
    }
}

#[cfg(feature = "opensearch")]
pub mod opensearch {
    use crate::api::error::ApiError;
    use crate::api::error_code::{
        BAD_GATEWAY, GATEWAY_TIMEOUT, INTERNAL_SERVER_ERROR, SERVICE_UNAVAILABLE,
    };
    use opensearch::Error as OpenSearchError;

    impl From<OpenSearchError> for ApiError {
        fn from(e: OpenSearchError) -> Self {
            // Check if the error contains an HTTP response with a specific status code
            // The opensearch crate's Error has a status_code() method for server errors
            if let Some(status_code) = e.status_code() {
                match status_code.as_u16() {
                    502 => return ApiError::bad_gateway(BAD_GATEWAY, Box::new(e)),
                    503 => return ApiError::service_unavailable(SERVICE_UNAVAILABLE, Box::new(e)),
                    504 => return ApiError::gateway_time_out(GATEWAY_TIMEOUT, Box::new(e)),
                    _ => {} // Continue to default error handling
                }
            }

            // Default to internal server error for all other cases
            ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(e))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde::ser::Error;

        #[test]
        fn should_convert_opensearch_error_to_internal_server_error_by_default() {
            // Create a basic opensearch error from serde_json error
            let json_error = serde_json::Error::custom("test error");
            let opensearch_error = OpenSearchError::from(json_error);
            let api_error: ApiError = opensearch_error.into();

            assert_eq!(api_error.status, 500);
            assert_eq!(api_error.error.as_str(), "INTERNAL_SERVER_ERROR");
        }
    }
}

#[cfg(test)]
pub mod tests {
    use rstest;

    use crate::api::error::{ApiError, ApiErrorSource, ApiErrorSourceType};
    use crate::api::error_code::*;
    use aws_lambda_events::apigw::ApiGatewayV2httpResponse;
    use http::header::CONTENT_TYPE;
    use serde::ser::Error;
    use serde_json::{Value, json};

    #[test]
    fn should_have_content_type_application_problem_json() {
        let error = ApiError::bad_request(BAD_REQUEST, Box::new(serde_json::Error::custom("foo")));
        let apigw_response = ApiGatewayV2httpResponse::from(error);
        assert_eq!(
            "application/problem+json",
            apigw_response
                .headers
                .get(CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap()
        );
    }

    #[rstest::rstest]
    #[case::bad_request(ApiError::bad_request(BAD_REQUEST, Box::new(serde_json::Error::custom("foo"))), json!({ "status": 400, "title": "Bad Request", "error": "BAD_REQUEST" }))]
    #[case::bad_request_msg(ApiError::bad_request(BAD_REQUEST, Box::new(serde_json::Error::custom("foo"))).with_detail("foo"), json!({ "status": 400, "error": "BAD_REQUEST", "title": "Bad Request", "detail": "foo" }))]
    #[case::unauthorized(ApiError::unauthorized(UNAUTHORIZED), json!({ "status": 401, "error": "UNAUTHORIZED", "title": "Unauthorized" }))]
    #[case::forbidden(ApiError::forbidden(FORBIDDEN), json!({ "status": 403, "title": "Forbidden", "error": "FORBIDDEN" }))]
    #[case::not_found(ApiError::not_found(NOT_FOUND, Box::new(serde_json::Error::custom("foo"))), json!({ "status": 404, "title": "Not Found", "error": "NOT_FOUND" }))]
    #[case::conflict(ApiError::conflict(CONFLICT, Box::new(serde_json::Error::custom("foo"))), json!({ "status": 409, "title": "Conflict", "error": "CONFLICT" }))]
    #[case::unprocessable_entity(ApiError::unprocessable_entity(UNPROCESSABLE_ENTITY, Box::new(serde_json::Error::custom("foo"))), json!({ "status": 422, "title": "Unprocessable Content", "error": "UNPROCESSABLE_ENTITY" }))]
    #[case::internal_server_error(ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(serde_json::Error::custom("foo"))), json!({ "status": 500, "title": "Internal Server Error", "error": "INTERNAL_SERVER_ERROR" }))]
    #[case::bad_gateway(ApiError::bad_gateway(BAD_GATEWAY, Box::new(serde_json::Error::custom("foo"))), json!({ "status": 502, "title": "Bad Gateway", "error": "BAD_GATEWAY" }))]
    #[case::service_unavailable(ApiError::service_unavailable(SERVICE_UNAVAILABLE, Box::new(serde_json::Error::custom("foo"))), json!({ "status": 503, "title": "Service Unavailable", "error": "SERVICE_UNAVAILABLE" }))]
    #[case::gateway_timeout(ApiError::gateway_time_out(GATEWAY_TIMEOUT, Box::new(serde_json::Error::custom("foo"))), json!({ "status": 504, "title": "Gateway Timeout", "error": "GATEWAY_TIMEOUT" }))]
    #[trace]
    fn should_serialize_api_error(#[case] error: ApiError, #[case] expected: Value) {
        let actual = serde_json::to_value(error).unwrap();
        assert_eq!(expected, actual);
    }

    #[test]
    fn should_serialize_api_error_with_query_field() {
        let error = ApiError::bad_request(BAD_REQUEST, Box::new(serde_json::Error::custom("foo")))
            .with_query_field("limit");
        let json = serde_json::to_value(error).unwrap();
        assert_eq!(
            json,
            json!({
                "status": 400,
                "title": "Bad Request",
                "error": "BAD_REQUEST",
                "source": {
                    "field": "limit",
                    "type": "QUERY"
                }
            })
        );
    }

    #[test]
    fn should_serialize_api_error_with_header_field() {
        let error = ApiError::unauthorized(UNAUTHORIZED).with_header_field("Authorization");
        let json = serde_json::to_value(error).unwrap();
        assert_eq!(
            json,
            json!({
                "status": 401,
                "title": "Unauthorized",
                "error": "UNAUTHORIZED",
                "source": {
                    "field": "Authorization",
                    "type": "HEADER"
                }
            })
        );
    }

    #[test]
    fn should_serialize_api_error_with_path_field() {
        let error = ApiError::not_found(NOT_FOUND, Box::new(serde_json::Error::custom("foo")))
            .with_path_field("user_id");
        let json = serde_json::to_value(error).unwrap();
        assert_eq!(
            json,
            json!({
                "status": 404,
                "title": "Not Found",
                "error": "NOT_FOUND",
                "source": {
                    "field": "user_id",
                    "type": "PATH"
                }
            })
        );
    }

    #[test]
    fn should_serialize_api_error_with_body_field() {
        let error = ApiError::unprocessable_entity(
            UNPROCESSABLE_ENTITY,
            Box::new(serde_json::Error::custom("foo")),
        )
        .with_body_field("email");
        let json = serde_json::to_value(error).unwrap();
        assert_eq!(
            json,
            json!({
                "title": "Unprocessable Content",
                "status": 422,
                "error": "UNPROCESSABLE_ENTITY",
                "source": {
                    "field": "email",
                    "type": "BODY"
                }
            })
        );
    }

    #[test]
    fn should_serialize_api_error_with_source_struct() {
        let source = ApiErrorSource {
            field: "x-custom-header",
            source_type: ApiErrorSourceType::Header,
        };
        let error = ApiError::bad_request(BAD_REQUEST, Box::new(serde_json::Error::custom("foo")))
            .with_source(source);
        let json = serde_json::to_value(error).unwrap();
        assert_eq!(
            json,
            json!({
                "status": 400,
                "title": "Bad Request",
                "error": "BAD_REQUEST",
                "source": {
                    "field": "x-custom-header",
                    "type": "HEADER"
                }
            })
        );
    }

    #[test]
    fn should_serialize_api_error_with_message_and_source() {
        let error = ApiError::bad_request(BAD_REQUEST, Box::new(serde_json::Error::custom("foo")))
            .with_detail("Invalid format")
            .with_body_field("username");
        let json = serde_json::to_value(error).unwrap();
        assert_eq!(
            json,
            json!({
                "status": 400,
                "title": "Bad Request",
                "error": "BAD_REQUEST",
                "detail": "Invalid format",
                "source": {
                    "field": "username",
                    "type": "BODY"
                }
            })
        );
    }
}
