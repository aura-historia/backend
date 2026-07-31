use crate::auth::AuthError;
use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use shop_service::use_cases::queries::get_shop::GetShopError;
use std::error::Error;
use std::fmt::{Display, Formatter};

#[derive(Debug, Serialize)]
pub(crate) struct ApiError {
    status: u16,
    title: &'static str,
    error: ApiErrorCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<ApiErrorSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    #[serde(skip)]
    cause: Option<Box<dyn Error + Send + Sync>>,
}

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(transparent)]
pub(crate) struct ApiErrorCode(&'static str);

pub(crate) const AUTH_INTERNAL_ERROR: ApiErrorCode = ApiErrorCode("AUTH_INTERNAL_ERROR");
pub(crate) const AUTH_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("AUTH_TEMPORARILY_UNAVAILABLE");
pub(crate) const INVALID_CREDENTIALS: ApiErrorCode = ApiErrorCode("INVALID_CREDENTIALS");
pub(crate) const INVALID_UUID: ApiErrorCode = ApiErrorCode("INVALID_UUID");
pub(crate) const SHOP_INTERNAL_ERROR: ApiErrorCode = ApiErrorCode("SHOP_INTERNAL_ERROR");
pub(crate) const SHOP_NOT_FOUND: ApiErrorCode = ApiErrorCode("SHOP_NOT_FOUND");
pub(crate) const SHOP_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("SHOP_TEMPORARILY_UNAVAILABLE");

impl Display for ApiErrorCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
pub(crate) struct ApiErrorSource {
    field: &'static str,
    #[serde(rename = "type")]
    source_type: ApiErrorSourceType,
}

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "UPPERCASE")]
enum ApiErrorSourceType {
    Header,
    Path,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, title: &'static str, error: ApiErrorCode) -> Self {
        Self {
            status: status.as_u16(),
            title,
            error,
            source: None,
            detail: None,
            cause: None,
        }
    }

    pub(crate) fn bad_request(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "Bad Request", error)
    }

    pub(crate) fn unauthorized(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Unauthorized", error)
    }

    pub(crate) fn not_found(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::NOT_FOUND, "Not Found", error)
    }

    pub(crate) fn internal_server_error(error: ApiErrorCode) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal Server Error",
            error,
        )
    }

    pub(crate) fn service_unavailable(error: ApiErrorCode) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Service Unavailable",
            error,
        )
    }

    pub(crate) fn with_header_field(mut self, field: &'static str) -> Self {
        self.source = Some(ApiErrorSource {
            field,
            source_type: ApiErrorSourceType::Header,
        });
        self
    }

    pub(crate) fn with_path_field(mut self, field: &'static str) -> Self {
        self.source = Some(ApiErrorSource {
            field,
            source_type: ApiErrorSourceType::Path,
        });
        self
    }

    pub(crate) fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn status_code(&self) -> StatusCode {
        match StatusCode::from_u16(self.status) {
            Ok(status) => status,
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl Display for ApiError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {} - {}", self.status, self.error)?;
        if let Some(detail) = &self.detail {
            write!(f, ": {detail}")?;
        }
        if let Some(cause) = &self.cause {
            write!(f, ": {cause}")?;
        }
        Ok(())
    }
}

impl Error for ApiError {}

impl From<AuthError> for ApiError {
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::TemporarilyUnavailable => {
                ApiError::service_unavailable(AUTH_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Authentication is temporarily unavailable.")
            }
            AuthError::Internal(_) => ApiError::internal_server_error(AUTH_INTERNAL_ERROR)
                .with_detail("Authentication failed internally."),
            AuthError::MissingCredentials
            | AuthError::InvalidAuthorizationHeader
            | AuthError::MalformedCredentials
            | AuthError::InvalidCredentials
            | AuthError::MissingClaim(_)
            | AuthError::InvalidClaimType(_)
            | AuthError::JwksKeyNotFound
            | AuthError::JwksFetch(_) => ApiError::unauthorized(INVALID_CREDENTIALS)
                .with_header_field("Authorization")
                .with_detail("Bearer token is invalid."),
        }
    }
}

impl From<GetShopError> for ApiError {
    fn from(error: GetShopError) -> Self {
        match error {
            GetShopError::NotFound => {
                ApiError::not_found(SHOP_NOT_FOUND).with_detail("Shop was not found.")
            }
            GetShopError::TemporarilyUnavailable { .. }
            | GetShopError::BeginTransactionFailed
            | GetShopError::CommitTransactionFailed => {
                ApiError::service_unavailable(SHOP_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Shop details are temporarily unavailable.")
            }
            GetShopError::InvalidReadModel { .. } | GetShopError::Internal { .. } => {
                ApiError::internal_server_error(SHOP_INTERNAL_ERROR)
                    .with_detail("Shop details failed internally.")
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status_code(),
            [(header::CONTENT_TYPE, "application/problem+json")],
            Json(self),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn should_render_problem_json_response() -> Result<(), Box<dyn std::error::Error>> {
        let response = ApiError::bad_request(INVALID_UUID)
            .with_path_field("shopId")
            .with_detail("Path parameter 'shopId' must be a UUID.")
            .into_response();

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!(
            "application/problem+json",
            response.headers()[header::CONTENT_TYPE]
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body = serde_json::from_slice::<serde_json::Value>(&bytes)?;
        assert_eq!(
            json!({
                "status": 400,
                "title": "Bad Request",
                "error": INVALID_UUID.to_string(),
                "source": {"field": "shopId", "type": "PATH"},
                "detail": "Path parameter 'shopId' must be a UUID."
            }),
            body
        );
        Ok(())
    }
}
