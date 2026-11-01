use std::collections::HashMap;
use std::sync::LazyLock;

use base64::Engine as _;
use base64::prelude::BASE64_STANDARD;
use sha2::{Digest as _, Sha256};

use crate::review::assets::INSPECTOR_JS;

pub(crate) struct ParsedRequest<'a> {
    pub(crate) method: String,
    pub(crate) path: &'a str,
    pub(crate) query: HashMap<String, String>,
    pub(crate) headers: HashMap<String, String>,
    pub(crate) body: &'a str,
}

pub(crate) fn parse_request(request: &str) -> Option<ParsedRequest<'_>> {
    let (head, body) = request.split_once("\r\n\r\n").unwrap_or((request, ""));
    let mut lines = head.lines();
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let raw_path = parts.next()?;
    let (path, query_raw) = raw_path.split_once('?').unwrap_or((raw_path, ""));
    let query = url::form_urlencoded::parse(query_raw.as_bytes())
        .into_owned()
        .collect();

    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    Some(ParsedRequest {
        method,
        path,
        query,
        headers,
        body,
    })
}

/// Policy for the console, its API and raw HTML: never framed.
const CONSOLE_CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; frame-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'";

/// Policy for instrumented shop page previews (`/api/live-inspect`,
/// `/api/review-pages/{id}/inspect`), which render untrusted shop HTML in the operator's
/// browser. It lets the page draw its layout and images and nothing else:
///
/// - `sandbox allow-scripts` gives the page an opaque origin even when opened outside the
///   console iframe, so nothing in it can use console credentials or same-origin API access.
/// - `default-src 'none'` refuses every fetch not allowed below: no `connect-src`, frames,
///   objects (`object-src` stays explicit), media, workers or manifests.
/// - `script-src` admits only the injected inspector script by its hash. Shop scripts are
///   already rewritten to `type="text/plain"`; the hash also blocks any that slip past that
///   rewrite, external scripts, inline event handlers and `javascript:` URLs.
/// - `style-src`, `img-src` and `font-src` admit the shop's stylesheets, images and fonts
///   from any `https:` host, inline styles, and `data:` images and fonts.
/// - `base-uri https:` admits the injected `<base href>` of the shop page, so relative asset
///   paths resolve against the shop.
/// - `form-action 'none'` and `frame-ancestors 'self'` let nothing submit a form and only the
///   console frame the preview.
fn preview_content_security_policy() -> &'static str {
    static POLICY: LazyLock<String> = LazyLock::new(|| {
        // The browser hashes the script text after its parser turned line breaks into `\n`.
        let inspector = INSPECTOR_JS.replace("\r\n", "\n");
        let inspector_hash = BASE64_STANDARD.encode(Sha256::digest(inspector.as_bytes()));
        format!(
            "sandbox allow-scripts; default-src 'none'; script-src 'sha256-{inspector_hash}'; \
             style-src https: 'unsafe-inline'; img-src https: data:; font-src https: data:; \
             object-src 'none'; base-uri https:; form-action 'none'; frame-ancestors 'self'"
        )
    });
    POLICY.as_str()
}

#[derive(Clone)]
pub(crate) struct HttpResponse {
    status: u16,
    content_type: &'static str,
    headers: Vec<(String, String)>,
    body: String,
    content_security_policy: &'static str,
}

impl HttpResponse {
    pub(crate) fn text(status: u16, body: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            headers: Vec::new(),
            content_security_policy: CONSOLE_CONTENT_SECURITY_POLICY,
            body: body.to_string(),
        }
    }

    pub(crate) fn html(status: u16, body: &str) -> Self {
        Self {
            status,
            content_type: "text/html; charset=utf-8",
            headers: Vec::new(),
            content_security_policy: CONSOLE_CONTENT_SECURITY_POLICY,
            body: body.to_string(),
        }
    }

    pub(crate) fn css(status: u16, body: &str) -> Self {
        Self {
            status,
            content_type: "text/css; charset=utf-8",
            headers: Vec::new(),
            content_security_policy: CONSOLE_CONTENT_SECURITY_POLICY,
            body: body.to_string(),
        }
    }

    pub(crate) fn javascript(status: u16, body: &str) -> Self {
        Self {
            status,
            content_type: "application/javascript; charset=utf-8",
            headers: Vec::new(),
            content_security_policy: CONSOLE_CONTENT_SECURITY_POLICY,
            body: body.to_string(),
        }
    }

    pub(crate) fn json(status: u16, body: &impl serde::Serialize) -> Self {
        let body = serde_json::to_string_pretty(body)
            .unwrap_or_else(|_| "{\"error\":\"failed to serialize response\"}".to_string());
        Self {
            status,
            content_type: "application/json; charset=utf-8",
            headers: Vec::new(),
            content_security_policy: CONSOLE_CONTENT_SECURITY_POLICY,
            body,
        }
    }

    /// Serves this response as a sandboxed shop page preview that only the review
    /// console may frame. Every other response keeps the console policy.
    /// The preview's own asset requests carry no referrer, so shop and third-party hosts do not
    /// learn the console's address or the previewed page id; the instrumentation disables the
    /// page's own referrer directives.
    pub(crate) fn into_review_preview(mut self) -> Self {
        self.content_security_policy = preview_content_security_policy();
        self.with_header("Referrer-Policy", "no-referrer")
    }

    pub(crate) fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

impl std::fmt::Display for HttpResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self.status {
            200 => "OK",
            201 => "Created",
            202 => "Accepted",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            409 => "Conflict",
            429 => "Too Many Requests",
            502 => "Bad Gateway",
            500 => "Internal Server Error",
            _ => "OK",
        };
        write!(
            f,
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: {}\r\n{}\r\n{}",
            self.status,
            reason,
            self.content_type,
            self.body.len(),
            self.content_security_policy,
            self.headers
                .iter()
                .map(|(name, value)| format!("{name}: {value}\r\n"))
                .collect::<String>(),
            self.body
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csp(response: &HttpResponse) -> String {
        let response = response.to_string();
        let (_, rest) = response
            .split_once("\r\nContent-Security-Policy: ")
            .unwrap_or_else(|| panic!("every response carries a policy"));
        rest.split_once("\r\n")
            .map(|(policy, _)| policy.to_string())
            .unwrap_or_default()
    }

    #[test]
    fn should_render_shop_assets_but_only_the_inspector_script_in_previews() {
        let preview = HttpResponse::html(200, "<p></p>").into_review_preview();
        let inspector_hash = BASE64_STANDARD.encode(Sha256::digest(INSPECTOR_JS.as_bytes()));

        assert_eq!(
            csp(&preview),
            format!(
                "sandbox allow-scripts; default-src 'none'; script-src 'sha256-{inspector_hash}'; \
                 style-src https: 'unsafe-inline'; img-src https: data:; font-src https: data:; \
                 object-src 'none'; base-uri https:; form-action 'none'; frame-ancestors 'self'"
            )
        );
        assert!(
            preview
                .to_string()
                .contains("\r\nReferrer-Policy: no-referrer\r\n")
        );
    }

    #[test]
    fn should_keep_the_strict_policy_for_every_other_response() {
        for response in [
            HttpResponse::html(200, ""),
            HttpResponse::text(200, ""),
            HttpResponse::json(200, &serde_json::json!({})),
            HttpResponse::css(200, ""),
            HttpResponse::javascript(200, ""),
        ] {
            assert_eq!(csp(&response), CONSOLE_CONTENT_SECURITY_POLICY);
            assert!(!response.to_string().contains("Referrer-Policy"));
        }
        assert_eq!(
            CONSOLE_CONTENT_SECURITY_POLICY,
            "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; frame-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"
        );
    }
}
