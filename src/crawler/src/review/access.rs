//! Who may use the review console: its bearer token, the browser sessions that token opens and,
//! in the local `dev` binary only, same-origin mutations without a token. The operator contract
//! is in `docs/crawler/operations.md` (review service and review console access).

use crate::review::http::{HttpResponse, ParsedRequest};
use dashmap::DashMap;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum ReviewServerConfigError {
    #[error("CRAWLER_REVIEW_BIND_ADDR must be host:port")]
    InvalidBindAddress(#[from] std::net::AddrParseError),
    #[error("CRAWLER_REVIEW_AUTH_TOKEN is required for non-loopback bind addresses")]
    AuthenticationRequiredForNonLoopback,
}

#[derive(Clone)]
pub struct ReviewServerConfig {
    pub bind_addr: SocketAddr,
    pub auth_token: Option<String>,
    /// Lets same-origin browser requests mutate review state without a token on a
    /// loopback bind. Only the local `dev` binary opts in through
    /// [`ReviewServerConfig::allow_local_mutations_without_token`], which keeps it
    /// limited to a loopback bind without a token.
    pub(super) local_mutations_without_token: bool,
}

impl ReviewServerConfig {
    pub fn from_env() -> Result<Self, ReviewServerConfigError> {
        let bind_addr: SocketAddr = std::env::var("CRAWLER_REVIEW_BIND_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:7878".to_string())
            .parse()?;
        let auth_token = std::env::var("CRAWLER_REVIEW_AUTH_TOKEN")
            .ok()
            .filter(|token| !token.trim().is_empty());
        Self {
            bind_addr,
            auth_token,
            local_mutations_without_token: false,
        }
        .validate()
    }

    /// Enables tokenless mutations for a loopback bind without a configured token.
    /// A configured token or a non-loopback bind keeps bearer authorization.
    pub fn local_mutations_without_token(&self) -> bool {
        self.local_mutations_without_token
    }

    pub fn allow_local_mutations_without_token(mut self) -> Self {
        self.local_mutations_without_token =
            self.auth_token.is_none() && self.bind_addr.ip().is_loopback();
        self
    }

    pub fn validate(self) -> Result<Self, ReviewServerConfigError> {
        if !self.bind_addr.ip().is_loopback() && self.auth_token.is_none() {
            return Err(ReviewServerConfigError::AuthenticationRequiredForNonLoopback);
        }
        Ok(self)
    }
}

const REVIEW_SESSION_TTL: Duration = Duration::from_secs(8 * 60 * 60);
const REVIEW_SESSION_COOKIE: &str = "crawler_review_session";

/// The review console's access control: its configuration and the browser sessions its bearer
/// token opened.
#[derive(Clone)]
pub(super) struct ReviewAccess {
    config: ReviewServerConfig,
    sessions: Arc<DashMap<uuid::Uuid, std::time::Instant>>,
}

impl ReviewAccess {
    pub(super) fn new(config: ReviewServerConfig) -> Self {
        Self {
            config,
            sessions: Arc::new(DashMap::new()),
        }
    }

    pub(super) fn bind_addr(&self) -> SocketAddr {
        self.config.bind_addr
    }

    pub(super) fn request_is_authorized(&self, request: &ParsedRequest<'_>) -> bool {
        if !request.path.starts_with("/api/") {
            return true;
        }

        let bearer = self.authorized(&request.headers);
        let mutation_bearer = self.config.auth_token.is_some() && bearer;
        let session = self.has_valid_session(&request.headers);
        let local_mutation = self.config.local_mutations_without_token
            && self.config.auth_token.is_none()
            && is_same_origin_loopback_request(&request.headers);
        match (request.method.as_str(), request.path) {
            ("POST", "/api/session") => mutation_bearer,
            ("POST", "/api/session/logout") => mutation_bearer || session,
            (method, _) if is_mutation_method(method) => mutation_bearer || local_mutation,
            _ => bearer || session,
        }
    }

    fn authorized(&self, headers: &std::collections::HashMap<String, String>) -> bool {
        let Some(expected) = self.config.auth_token.as_deref() else {
            return true;
        };
        headers
            .get("authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|actual| constant_time_eq(actual.as_bytes(), expected.as_bytes()))
    }

    fn has_valid_session(&self, headers: &std::collections::HashMap<String, String>) -> bool {
        let Some(session_id) = session_id_from_headers(headers) else {
            return false;
        };
        let now = std::time::Instant::now();
        match self.sessions.get(&session_id) {
            Some(expiry) if *expiry > now => true,
            Some(_) => {
                self.sessions.remove(&session_id);
                false
            }
            None => false,
        }
    }

    pub(super) fn create_session(&self) -> HttpResponse {
        let now = std::time::Instant::now();
        self.sessions.retain(|_, expiry| *expiry > now);
        let session_id = uuid::Uuid::new_v4();
        self.sessions.insert(session_id, now + REVIEW_SESSION_TTL);
        HttpResponse::json(200, &json!({ "ok": true })).with_header(
            "Set-Cookie",
            session_cookie(&self.config, session_id, REVIEW_SESSION_TTL),
        )
    }

    pub(super) fn clear_session(&self, request: &ParsedRequest<'_>) -> HttpResponse {
        if let Some(session_id) = session_id_from_headers(&request.headers) {
            self.sessions.remove(&session_id);
        }
        HttpResponse::json(200, &json!({ "ok": true }))
            .with_header("Set-Cookie", expired_session_cookie(&self.config))
    }
}

fn session_id_from_headers(
    headers: &std::collections::HashMap<String, String>,
) -> Option<uuid::Uuid> {
    headers
        .get("cookie")?
        .split(';')
        .map(str::trim)
        .find_map(|cookie| cookie.strip_prefix(&format!("{REVIEW_SESSION_COOKIE}=")))
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
}

fn session_cookie(config: &ReviewServerConfig, session_id: uuid::Uuid, ttl: Duration) -> String {
    let secure = if config.bind_addr.ip().is_loopback() {
        ""
    } else {
        "; Secure"
    };
    format!(
        "{REVIEW_SESSION_COOKIE}={session_id}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{secure}",
        ttl.as_secs()
    )
}

fn expired_session_cookie(config: &ReviewServerConfig) -> String {
    session_cookie(config, uuid::Uuid::nil(), Duration::ZERO)
}

fn is_mutation_method(method: &str) -> bool {
    matches!(method, "POST" | "PUT" | "PATCH" | "DELETE")
}

/// Browsers send `Origin` on every cross-origin mutation, so requiring it to equal
/// the loopback `Host` rejects other sites and DNS-rebinding hosts.
fn is_same_origin_loopback_request(headers: &std::collections::HashMap<String, String>) -> bool {
    let (Some(host), Some(origin)) = (headers.get("host"), headers.get("origin")) else {
        return false;
    };
    host_is_loopback(host) && origin.eq_ignore_ascii_case(&format!("http://{host}"))
}

fn host_is_loopback(host: &str) -> bool {
    let Ok(url) = Url::parse(&format!("http://{host}")) else {
        return false;
    };
    if !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    match url.host() {
        Some(url::Host::Domain(domain)) => domain == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let max_len = left.len().max(right.len());
    for index in 0..max_len {
        difference |= usize::from(*left.get(index).unwrap_or(&0) ^ *right.get(index).unwrap_or(&0));
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_keep_bearer_authorization_when_local_mode_has_token_or_public_bind() {
        let with_token = ReviewServerConfig {
            bind_addr: "127.0.0.1:7878".parse().unwrap(),
            auth_token: Some("review-token".to_string()),
            local_mutations_without_token: false,
        }
        .allow_local_mutations_without_token();
        let public_bind = ReviewServerConfig {
            bind_addr: "0.0.0.0:7878".parse().unwrap(),
            auth_token: None,
            local_mutations_without_token: false,
        }
        .allow_local_mutations_without_token();

        assert!(!with_token.local_mutations_without_token);
        assert!(!public_bind.local_mutations_without_token);
    }

    #[test]
    fn should_require_authentication_for_non_loopback_review_bind() {
        let result = ReviewServerConfig {
            bind_addr: "0.0.0.0:7878".parse().unwrap(),
            auth_token: None,
            local_mutations_without_token: false,
        }
        .validate();

        assert!(matches!(
            result,
            Err(ReviewServerConfigError::AuthenticationRequiredForNonLoopback)
        ));
    }

    #[test]
    fn should_allow_non_loopback_review_bind_with_authentication_token() {
        let result = ReviewServerConfig {
            bind_addr: "0.0.0.0:7878".parse().unwrap(),
            auth_token: Some("review-token".to_string()),
            local_mutations_without_token: false,
        }
        .validate();

        assert!(result.is_ok());
    }

    #[test]
    fn should_allow_loopback_review_bind_without_authentication_token() {
        let result = ReviewServerConfig {
            bind_addr: "127.0.0.1:7878".parse().unwrap(),
            auth_token: None,
            local_mutations_without_token: false,
        }
        .validate();

        assert!(result.is_ok());
    }

    // Tests that touch the process environment are serialized, so no test reads the
    // environment while another one changes it.
    fn set_env(name: &str, value: Option<&str>) {
        match value {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
    }

    const REVIEW_BIND_ADDR_ENV: &str = "CRAWLER_REVIEW_BIND_ADDR";
    const REVIEW_AUTH_TOKEN_ENV: &str = "CRAWLER_REVIEW_AUTH_TOKEN";

    /// Reads the review config from exactly these review settings, leaving them unset afterwards.
    fn review_config_from(
        bind_addr: Option<&str>,
        auth_token: Option<&str>,
    ) -> Result<ReviewServerConfig, ReviewServerConfigError> {
        set_env(REVIEW_BIND_ADDR_ENV, bind_addr);
        set_env(REVIEW_AUTH_TOKEN_ENV, auth_token);
        let config = ReviewServerConfig::from_env();
        set_env(REVIEW_BIND_ADDR_ENV, None);
        set_env(REVIEW_AUTH_TOKEN_ENV, None);
        config
    }

    #[rstest::rstest]
    #[case::unset(None)]
    #[case::blank(Some("  "))]
    #[serial_test::serial]
    fn should_name_the_missing_token_when_a_non_loopback_bind_has_none(
        #[case] auth_token: Option<&'static str>,
    ) {
        let error = review_config_from(Some("0.0.0.0:7878"), auth_token)
            .err()
            .unwrap_or_else(|| panic!("a non-loopback bind without a token must fail"));

        assert_eq!(
            error.to_string(),
            "CRAWLER_REVIEW_AUTH_TOKEN is required for non-loopback bind addresses"
        );
    }

    #[test]
    #[serial_test::serial]
    fn should_name_the_bind_address_when_it_is_not_host_and_port() {
        let error = review_config_from(Some("localhost"), None)
            .err()
            .unwrap_or_else(|| panic!("a bind address without a port must fail"));

        assert_eq!(
            error.to_string(),
            "CRAWLER_REVIEW_BIND_ADDR must be host:port"
        );
    }

    #[test]
    fn should_compare_authentication_tokens_without_early_value_match() {
        assert!(constant_time_eq(b"token", b"token"));
        assert!(!constant_time_eq(b"token", b"taken"));
        assert!(!constant_time_eq(b"token", b"tokens"));
    }
}
