use crate::api::error::ApiError;

/// Log API errors based on their status code
/// - 4xx errors are logged at WARN level
/// - 5xx errors are logged at ERROR level
pub fn log_api_error(error: &ApiError) {
    if error.status >= 500 {
        tracing::error!(
            status = error.status,
            error_code = %error.error,
            message = ?error.message,
            source = ?error.source,
            "API error response"
        );
    } else if error.status >= 400 {
        tracing::warn!(
            status = error.status,
            error_code = %error.error,
            message = ?error.message,
            source = ?error.source,
            "API error response"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::error_code::*;

    #[test]
    fn should_log_4xx_at_warn_level() {
        // This test verifies the function doesn't panic
        // Actual log level verification would require a tracing subscriber mock
        let error = ApiError::bad_request(BAD_REQUEST);
        log_api_error(&error);
    }

    #[test]
    fn should_log_5xx_at_error_level() {
        // This test verifies the function doesn't panic
        // Actual log level verification would require a tracing subscriber mock
        let error = ApiError::internal_server_error(INTERNAL_SERVER_ERROR);
        log_api_error(&error);
    }

    #[test]
    fn should_log_with_message() {
        let error = ApiError::bad_request(BAD_REQUEST).with_message("test message");
        log_api_error(&error);
    }

    #[test]
    fn should_log_with_source() {
        let error = ApiError::bad_request(BAD_REQUEST).with_body_field("test_field");
        log_api_error(&error);
    }

    #[test]
    fn should_log_all_4xx_status_codes() {
        // Test various 4xx errors
        log_api_error(&ApiError::bad_request(BAD_REQUEST));
        log_api_error(&ApiError::unauthorized(UNAUTHORIZED));
        log_api_error(&ApiError::forbidden(FORBIDDEN));
        log_api_error(&ApiError::not_found(NOT_FOUND));
        log_api_error(&ApiError::conflict(CONFLICT));
        log_api_error(&ApiError::unprocessable_entity(UNPROCESSABLE_ENTITY));
    }

    #[test]
    fn should_log_all_5xx_status_codes() {
        // Test various 5xx errors
        log_api_error(&ApiError::internal_server_error(INTERNAL_SERVER_ERROR));
        log_api_error(&ApiError::service_unavailable(SERVICE_UNAVAILABLE));
        log_api_error(&ApiError::gateway_time_out(GATEWAY_TIMEOUT));
    }

    #[test]
    fn should_not_log_below_400() {
        // Create a custom error with status < 400 (though this shouldn't happen in practice)
        let error = ApiError::new(http::StatusCode::OK, BAD_REQUEST);
        // Should not panic, just won't log anything
        log_api_error(&error);
    }
}
