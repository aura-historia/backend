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
}
