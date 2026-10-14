use crate::error::{ApiError, BAD_HEADER_VALUE};
use application::idempotency::IdempotencyKey;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::response::Response;

pub(crate) const KEY_HEADER: HeaderName = HeaderName::from_static("idempotency-key");

#[allow(clippy::result_large_err)]
pub(crate) fn optional_key(headers: &HeaderMap) -> Result<Option<IdempotencyKey>, ApiError> {
    let invalid = || {
        ApiError::bad_request(BAD_HEADER_VALUE)
        .with_header_field("Idempotency-Key")
        .with_detail("Supply a single Idempotency-Key containing 1–128 ASCII letters, digits, hyphens, underscores, periods or colons.")
    };
    let mut values = headers.get_all(&KEY_HEADER).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(invalid());
    }
    let raw = value.to_str().map_err(|_| invalid())?;
    IdempotencyKey::new(raw.to_owned())
        .map(Some)
        .map_err(|_| invalid())
}

pub(crate) fn echo_key(response: &mut Response, key: Option<&IdempotencyKey>) {
    if let Some(key) = key {
        // Validated ASCII; no response data, authorization headers or request IDs are replayed.
        if let Ok(mut value) = HeaderValue::from_str(key.as_str()) {
            value.set_sensitive(true);
            response.headers_mut().insert(KEY_HEADER, value);
        }
    }
}

/// Transport-only response metadata. All correctness remains inside service transactions.
pub(crate) async fn command_response(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    #[derive(serde::Deserialize)]
    struct Policy {
        method: String,
        path: String,
        policy: String,
    }
    static POLICIES: std::sync::OnceLock<Vec<Policy>> = std::sync::OnceLock::new();
    let policies = POLICIES.get_or_init(|| {
        serde_json::from_str(include_str!("../../../docs/command-retry-policies.json"))
            .expect("compiled command policy inventory")
    });
    let opted = policies.iter().any(|p| {
        p.method == request.method().as_str()
            && matches!(
                p.policy.as_str(),
                "keyed" | "workflow" | "secret-fence" | "external-fence"
            )
            && route_matches(&p.path, request.uri().path())
    });
    let key = if opted {
        optional_key(request.headers()).ok().flatten()
    } else {
        None
    };
    let mut response = next.run(request).await;
    if let Some(key) = key.as_ref() {
        response.headers_mut().insert(
            axum::http::header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        );
        if response.status() != axum::http::StatusCode::UNAUTHORIZED
            && response.status() != axum::http::StatusCode::FORBIDDEN
        {
            echo_key(&mut response, Some(key));
        }
    }
    response
}
fn route_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.split('/').collect::<Vec<_>>();
    let path = path.split('/').collect::<Vec<_>>();
    pattern.len() == path.len()
        && pattern
            .iter()
            .zip(path)
            .all(|(p, v)| p == &v || p.starts_with('{') && p.ends_with('}') && !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn command_metadata_preserves_keyless_cache_headers() {
        use axum::{Router, body::Body, http::Request, middleware, routing::post};
        use tower::ServiceExt;

        let app = Router::new()
            .route(
                "/api/v1/me/search-filters",
                post(|| async { ([(axum::http::header::CACHE_CONTROL, "no-store")], "ok") }),
            )
            .layer(middleware::from_fn(command_response));

        for key in [None, Some("retry-123")] {
            let mut request = Request::post("/api/v1/me/search-filters");
            if let Some(key) = key {
                request = request.header(KEY_HEADER, key);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                if key.is_some() {
                    "private, no-store"
                } else {
                    "no-store"
                },
                response.headers()[axum::http::header::CACHE_CONTROL]
            );
            assert_eq!(
                key,
                response
                    .headers()
                    .get(KEY_HEADER)
                    .map(|value| value.to_str().unwrap())
            );
        }
    }

    #[test]
    fn optional_header_preserves_legacy_clients_and_rejects_ambiguous_values() {
        let mut headers = HeaderMap::new();
        assert_eq!(None, optional_key(&headers).unwrap());
        for value in ["", "a,b", "a b", "a/b"] {
            headers.insert(KEY_HEADER, HeaderValue::from_str(value).unwrap());
            let error = optional_key(&headers).unwrap_err();
            let problem = serde_json::to_value(&error).unwrap();
            assert_eq!(400, problem["status"]);
            assert!(problem.to_string().contains("Idempotency-Key"));
        }
        headers.insert(KEY_HEADER, HeaderValue::from_static("same"));
        headers.append(KEY_HEADER, HeaderValue::from_static("same"));
        assert!(optional_key(&headers).is_err());
        headers.insert(KEY_HEADER, HeaderValue::from_bytes(b"\xff").unwrap());
        assert!(optional_key(&headers).is_err());
    }

    #[test]
    fn accepted_key_can_be_echoed_without_diagnostic_disclosure() {
        let key = IdempotencyKey::new("command-123_.:".into()).unwrap();
        let mut response = Response::new(axum::body::Body::empty());
        echo_key(&mut response, Some(&key));
        assert_eq!(key.as_str(), response.headers()[KEY_HEADER]);
        assert!(response.headers()[KEY_HEADER].is_sensitive());
    }
}
