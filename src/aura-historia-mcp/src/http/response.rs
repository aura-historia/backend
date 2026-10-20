use axum::{
    body::{Body, to_bytes},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

pub(super) async fn finalize_response(response: Response, limit: usize) -> Response {
    let (parts, body) = response.into_parts();
    let mut response = match to_bytes(body, limit).await {
        Ok(bytes) => Response::from_parts(parts, Body::from(bytes)),
        Err(_) => protocol_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            -32603,
            "Response exceeds transport limit",
            Value::Null,
        ),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub(crate) fn deadline_response() -> Response {
    let mut response = protocol_error(
        StatusCode::REQUEST_TIMEOUT,
        -32603,
        "Request deadline exceeded",
        Value::Null,
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
pub(super) fn protocol_error(
    status: StatusCode,
    code: i32,
    message: &'static str,
    id: Value,
) -> Response {
    (
        status,
        axum::Json(
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
        ),
    )
        .into_response()
}
