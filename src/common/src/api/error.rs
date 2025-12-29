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

    pub fn internal_server_error(error: ApiErrorCode, cause: Box<dyn Error>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal Server Error",
            error,
        )
        .with_cause(cause)
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
    use crate::api::error_code::{GATEWAY_TIMEOUT, INTERNAL_SERVER_ERROR};
    use aws_sdk_dynamodb::error::SdkError;

    impl<S: std::error::Error + 'static> From<SdkError<S>> for ApiError {
        fn from(e: SdkError<S>) -> Self {
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
