use std::collections::HashMap;

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

/// Policy for instrumented shop page previews. `sandbox` gives the untrusted page an
/// opaque origin even when opened outside the console iframe, so its inline handlers
/// can never use console credentials or same-origin API access.
const PREVIEW_CONTENT_SECURITY_POLICY: &str = "sandbox allow-scripts; default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; frame-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'self'";

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
    pub(crate) fn into_review_preview(mut self) -> Self {
        self.content_security_policy = PREVIEW_CONTENT_SECURITY_POLICY;
        self
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
