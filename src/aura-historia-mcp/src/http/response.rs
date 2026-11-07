use axum::{
    body::{Body, Bytes, to_bytes},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

/// Marks a credential-free discovery document whose success may be cached publicly.
#[derive(Clone, Copy)]
struct PublicDocument;

const PUBLIC_DOCUMENT_CACHE_CONTROL: &str = "public, max-age=300";

/// Every other response, including errors on a discovery path, remains `no-store`.
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
    let cache_control = if response.status() == StatusCode::OK
        && response.extensions().get::<PublicDocument>().is_some()
    {
        PUBLIC_DOCUMENT_CACHE_CONTROL
    } else {
        "no-store"
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    response
}

/// Public metadata carries no credentials or per-viewer state, so any origin may read it.
pub(super) fn public_document(media_type: &'static str, body: Bytes) -> Response {
    let mut response = (
        [
            (header::CONTENT_TYPE, media_type),
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
        ],
        body,
    )
        .into_response();
    response.extensions_mut().insert(PublicDocument);
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
