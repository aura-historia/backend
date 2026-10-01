use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;

const PRIVATE_NO_STORE: &str = "private, no-store";

#[derive(Clone, Copy)]
struct ApprovedPublicCache {
    shared_ttl_seconds: u32,
}

pub(crate) fn private_no_store(mut response: Response) -> Response {
    response.extensions_mut().remove::<ApprovedPublicCache>();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(PRIVATE_NO_STORE),
    );
    merge_cache_vary(response.headers_mut());
    response
}

pub(crate) fn anonymous_shared_success(
    mut response: Response,
    request_headers: &HeaderMap,
    is_anonymous: bool,
    shared_ttl_seconds: u32,
) -> Response {
    if response.status() != StatusCode::OK
        || !is_anonymous
        || request_headers.contains_key(header::AUTHORIZATION)
    {
        return private_no_store(response);
    }

    let approval = ApprovedPublicCache { shared_ttl_seconds };
    if !write_public_cache_control(&mut response, approval) {
        return private_no_store(response);
    }
    response.extensions_mut().insert(approval);
    merge_cache_vary(response.headers_mut());
    response
}

pub(crate) fn apply_transport_policy(
    request_headers: &HeaderMap,
    mut response: Response,
) -> Response {
    let approval = response.extensions().get::<ApprovedPublicCache>().copied();
    if let Some(approval) = approval {
        if response.status() == StatusCode::OK
            && !request_headers.contains_key(header::AUTHORIZATION)
            && write_public_cache_control(&mut response, approval)
        {
            merge_cache_vary(response.headers_mut());
            return response;
        }

        return private_no_store(response);
    }

    if has_no_store_directive(response.headers()) {
        merge_cache_vary(response.headers_mut());
        return response;
    }

    private_no_store(response)
}

fn has_no_store_directive(headers: &HeaderMap) -> bool {
    headers.get_all(header::CACHE_CONTROL).iter().any(|value| {
        value
            .as_bytes()
            .split(|byte| *byte == b',')
            .any(|directive| {
                let directive = trim_optional_whitespace(directive);
                let name = directive
                    .split(|byte| *byte == b'=')
                    .next()
                    .unwrap_or_default();
                trim_optional_whitespace(name).eq_ignore_ascii_case(b"no-store")
            })
    })
}

fn write_public_cache_control(response: &mut Response, approval: ApprovedPublicCache) -> bool {
    let value = format!(
        "public, max-age=0, s-maxage={}",
        approval.shared_ttl_seconds
    );
    let Ok(value) = HeaderValue::from_str(&value) else {
        return false;
    };
    response.headers_mut().insert(header::CACHE_CONTROL, value);
    true
}

fn merge_cache_vary(headers: &mut HeaderMap) {
    let mut tokens = Vec::<Vec<u8>>::new();
    for value in headers.get_all(header::VARY).iter() {
        for token in value.as_bytes().split(|byte| *byte == b',') {
            let token = trim_optional_whitespace(token);
            if !token.is_empty() && !contains_token(&tokens, token) {
                tokens.push(token.to_vec());
            }
        }
    }

    for required in [b"Authorization".as_slice(), b"Origin".as_slice()] {
        if !contains_token(&tokens, required) {
            tokens.push(required.to_vec());
        }
    }

    let mut merged = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if index > 0 {
            merged.extend_from_slice(b", ");
        }
        merged.extend_from_slice(token);
    }

    if let Ok(value) = HeaderValue::from_bytes(&merged) {
        headers.remove(header::VARY);
        headers.insert(header::VARY, value);
    }
}

fn contains_token(tokens: &[Vec<u8>], candidate: &[u8]) -> bool {
    tokens
        .iter()
        .any(|token| token.eq_ignore_ascii_case(candidate))
}

fn trim_optional_whitespace(mut value: &[u8]) -> &[u8] {
    while value
        .first()
        .is_some_and(|byte| *byte == b' ' || *byte == b'\t')
    {
        value = &value[1..];
    }
    while value
        .last()
        .is_some_and(|byte| *byte == b' ' || *byte == b'\t')
    {
        value = &value[..value.len() - 1];
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_merge_vary_tokens_preserving_existing_values_without_duplicates() {
        let mut headers = HeaderMap::new();
        headers.append(
            header::VARY,
            HeaderValue::from_static("Accept-Encoding, authorization"),
        );
        headers.append(
            header::VARY,
            HeaderValue::from_static("x-custom, ORIGIN, accept-encoding"),
        );

        merge_cache_vary(&mut headers);

        assert_eq!(
            "Accept-Encoding, authorization, x-custom, ORIGIN",
            headers[header::VARY]
        );
    }

    #[test]
    fn should_preserve_existing_no_store_cache_control_values() {
        for value in ["no-store", "private, no-store"] {
            let mut response = Response::new(axum::body::Body::empty());
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_str(value).unwrap());

            let response = apply_transport_policy(&HeaderMap::new(), response);

            assert_eq!(value, response.headers()[header::CACHE_CONTROL]);
        }
    }

    #[test]
    fn should_recognize_no_store_across_all_cache_control_fields() {
        let mut response = Response::new(axum::body::Body::empty());
        response.headers_mut().append(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=60"),
        );
        response.headers_mut().append(
            header::CACHE_CONTROL,
            HeaderValue::from_static(" PrIvAtE , NO-STORE "),
        );

        let response = apply_transport_policy(&HeaderMap::new(), response);
        let values = response
            .headers()
            .get_all(header::CACHE_CONTROL)
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(vec!["public, max-age=60", " PrIvAtE , NO-STORE "], values);
    }

    #[test]
    fn should_fail_closed_for_unapproved_cache_control_directives() {
        for value in ["public, max-age=60", "no-cache", "x-no-store=60"] {
            let mut response = Response::new(axum::body::Body::empty());
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_str(value).unwrap());

            let response = apply_transport_policy(&HeaderMap::new(), response);

            assert_eq!(PRIVATE_NO_STORE, response.headers()[header::CACHE_CONTROL]);
        }
    }

    #[test]
    fn should_make_private_cache_control_when_authorization_is_present() {
        let mut request_headers = HeaderMap::new();
        request_headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("not even a valid bearer token"),
        );
        let response = anonymous_shared_success(
            Response::builder()
                .status(StatusCode::OK)
                .body(axum::body::Body::empty())
                .unwrap(),
            &request_headers,
            true,
            300,
        );

        assert_eq!(
            "private, no-store",
            response.headers()[header::CACHE_CONTROL]
        );
        assert_eq!("Authorization, Origin", response.headers()[header::VARY]);
    }
}
