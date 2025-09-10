/// Utilities for serializing complex errors as JSON for logging
///
/// This module provides helper functions to convert complex error types
/// (like AWS SDK errors and OpenSearch errors) into JSON format for better
/// structured logging instead of using Debug formatting.
#[cfg(any(
    feature = "dynamodb",
    feature = "sqs",
    feature = "opensearch",
    feature = "api"
))]
use serde_json::{Value, json};

/// A wrapper type that makes any error serialize as JSON for tracing
#[cfg(any(
    feature = "dynamodb",
    feature = "sqs",
    feature = "opensearch",
    feature = "api"
))]
pub struct JsonError<E>(pub E);

#[cfg(any(
    feature = "dynamodb",
    feature = "sqs",
    feature = "opensearch",
    feature = "api"
))]
impl<E> std::fmt::Display for JsonError<E>
where
    E: std::fmt::Debug + std::fmt::Display,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let json = error_to_json(&self.0);
        write!(f, "{}", json)
    }
}

/// Convert any error to a JsonError wrapper for automatic JSON serialization in tracing
#[cfg(any(
    feature = "dynamodb",
    feature = "sqs",
    feature = "opensearch",
    feature = "api"
))]
pub fn json_error<E>(error: &E) -> JsonError<&E> {
    JsonError(error)
}

/// Convert AWS SDK SdkError to JSON representation
///
/// Extracts relevant information from AWS SDK errors and formats them
/// as a structured JSON object for logging.
#[cfg(feature = "dynamodb")]
pub fn sdk_error_to_json<T>(error: &aws_sdk_dynamodb::error::SdkError<T>) -> Value
where
    T: std::fmt::Debug + std::fmt::Display,
{
    use aws_sdk_dynamodb::error::SdkError;

    match error {
        SdkError::ConstructionFailure(_) => json!({
            "type": "ConstructionFailure",
            "message": format!("{}", error),
        }),
        SdkError::TimeoutError(_) => json!({
            "type": "TimeoutError",
            "message": format!("{}", error),
        }),
        SdkError::DispatchFailure(_) => json!({
            "type": "DispatchFailure",
            "message": format!("{}", error),
        }),
        SdkError::ResponseError(_) => json!({
            "type": "ResponseError",
            "message": format!("{}", error),
        }),
        SdkError::ServiceError(service_err) => json!({
            "type": "ServiceError",
            "message": format!("{}", service_err.err()),
        }),
        _ => json!({
            "type": "UnknownSdkError",
            "message": format!("{}", error)
        }),
    }
}

/// Convert SQS SDK SdkError to JSON representation
#[cfg(feature = "sqs")]
pub fn sqs_sdk_error_to_json<T>(error: &aws_sdk_sqs::error::SdkError<T>) -> Value
where
    T: std::fmt::Debug + std::fmt::Display,
{
    use aws_sdk_sqs::error::SdkError;

    match error {
        SdkError::ConstructionFailure(_) => json!({
            "type": "ConstructionFailure",
            "message": format!("{}", error),
        }),
        SdkError::TimeoutError(_) => json!({
            "type": "TimeoutError",
            "message": format!("{}", error),
        }),
        SdkError::DispatchFailure(_) => json!({
            "type": "DispatchFailure",
            "message": format!("{}", error),
        }),
        SdkError::ResponseError(_) => json!({
            "type": "ResponseError",
            "message": format!("{}", error),
        }),
        SdkError::ServiceError(service_err) => json!({
            "type": "ServiceError",
            "message": format!("{}", service_err.err()),
        }),
        _ => json!({
            "type": "UnknownSdkError",
            "message": format!("{}", error)
        }),
    }
}

/// Convert OpenSearch Error to JSON representation
#[cfg(feature = "opensearch")]
pub fn opensearch_error_to_json(error: &opensearch::Error) -> Value {
    // OpenSearch Error struct doesn't expose variants, so we use Display/Debug
    json!({
        "type": "OpenSearchError",
        "message": format!("{}", error),
        "debug": format!("{:?}", error)
    })
}

/// Generic fallback for any error that doesn't have a specific handler
///
/// This function attempts to serialize the error as JSON and falls back
/// to a string representation if serialization fails.
#[cfg(any(
    feature = "dynamodb",
    feature = "sqs",
    feature = "opensearch",
    feature = "api"
))]
pub fn error_to_json_fallback<E>(error: &E) -> Value
where
    E: std::fmt::Debug,
{
    json!({
        "type": std::any::type_name::<E>(),
        "debug": format!("{:?}", error)
    })
}

/// Smart error to JSON converter that chooses the appropriate serialization method
///
/// This function automatically detects the error type and uses the most appropriate
/// JSON serialization method.
#[cfg(any(
    feature = "dynamodb",
    feature = "sqs",
    feature = "opensearch",
    feature = "api"
))]
pub fn error_to_json<E>(error: &E) -> Value
where
    E: std::fmt::Debug + std::fmt::Display,
{
    // Check for specific error types and use appropriate handlers
    let type_name = std::any::type_name::<E>();

    if type_name.contains("aws_sdk_dynamodb::error::SdkError") {
        #[cfg(feature = "dynamodb")]
        {
            // We can't directly call sdk_error_to_json due to type constraints,
            // so we use the fallback approach
            return error_to_json_fallback(error);
        }
    }

    if type_name.contains("aws_sdk_sqs::error::SdkError") {
        #[cfg(feature = "sqs")]
        {
            return error_to_json_fallback(error);
        }
    }

    if type_name.contains("opensearch::Error") {
        #[cfg(feature = "opensearch")]
        {
            return error_to_json_fallback(error);
        }
    }

    // Default fallback
    error_to_json_fallback(error)
}

/// Macro to easily convert errors to JSON for logging
///
/// Usage: `error_json!(err)` instead of `?err` in tracing macros
#[cfg(any(
    feature = "dynamodb",
    feature = "sqs",
    feature = "opensearch",
    feature = "api"
))]
#[macro_export]
macro_rules! error_json {
    ($error:expr) => {
        $crate::error_json::error_to_json(&$error)
    };
}

/// Macro to wrap errors for automatic JSON serialization in tracing
///
/// Usage: `error!(error = %json_error!(err))` 
/// This will automatically serialize the error as JSON
#[cfg(any(
    feature = "dynamodb",
    feature = "sqs",
    feature = "opensearch",
    feature = "api"
))]
#[macro_export]
macro_rules! json_error {
    ($error:expr) => {
        $crate::error_json::json_error(&($error))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(
        feature = "dynamodb",
        feature = "sqs",
        feature = "opensearch",
        feature = "api"
    ))]
    #[test]
    fn should_serialize_fallback_error_when_generic_error_for_testing() {
        let error = std::io::Error::new(std::io::ErrorKind::NotFound, "test error");
        let json = error_to_json_fallback(&error);

        assert!(json.is_object());
        assert_eq!(json["type"], "std::io::error::Error");
        assert!(json["debug"].is_string());
        assert!(json["debug"].as_str().unwrap().contains("test error"));
    }

    #[cfg(any(
        feature = "dynamodb",
        feature = "sqs",
        feature = "opensearch",
        feature = "api"
    ))]
    #[test]
    fn should_serialize_any_error_with_smart_converter_when_using_error_to_json_for_testing() {
        let error = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
        let json = error_to_json(&error);

        assert!(json.is_object());
        assert_eq!(json["type"], "std::io::error::Error");
        assert!(json["debug"].is_string());
        assert!(json["debug"].as_str().unwrap().contains("access denied"));
    }

    #[cfg(any(
        feature = "dynamodb",
        feature = "sqs",
        feature = "opensearch",
        feature = "api"
    ))]
    #[test]
    fn should_work_with_error_json_macro_when_called_for_testing() {
        let error = std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout");
        let json = error_json!(error);

        assert!(json.is_object());
        assert_eq!(json["type"], "std::io::error::Error");
        assert!(json["debug"].is_string());
        assert!(json["debug"].as_str().unwrap().contains("timeout"));
    }

    #[cfg(any(
        feature = "dynamodb",
        feature = "sqs",
        feature = "opensearch",
        feature = "api"
    ))]
    #[test]
    fn should_format_json_error_wrapper_when_displayed_for_testing() {
        let error = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let json_err = json_error!(error);
        let formatted = format!("{}", json_err);

        // The formatted output should be a JSON string
        assert!(formatted.contains("type"));
        assert!(formatted.contains("std::io::error::Error"));
        assert!(formatted.contains("file not found"));
    }

    #[cfg(any(
        feature = "dynamodb",
        feature = "sqs",
        feature = "opensearch",
        feature = "api"
    ))]
    #[test]
    fn should_create_json_error_wrapper_when_using_json_error_function_for_testing() {
        let error = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
        let json_err = json_error(&error);
        
        // Test that it can be formatted (which internally uses our JSON conversion)
        let formatted = format!("{}", json_err);
        assert!(formatted.contains("type"));
        assert!(formatted.contains("access denied"));
    }
}
