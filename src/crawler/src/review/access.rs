//! Who may use the review console and as which actor: the human review token, the browser
//! sessions that token opens, the review agent's own token and, in the local `dev` binary only,
//! same-origin mutations without a token. The operator contract is in
//! `docs/crawler/operations.md` (review service and review console access).

use crate::review::http::{HttpResponse, ParsedRequest};
use crate::review::model::ReviewActor;
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
    #[error("CRAWLER_REVIEW_AGENT_TOKEN is accepted only together with CRAWLER_REVIEW_AUTH_TOKEN")]
    AgentTokenWithoutReviewToken,
    #[error("CRAWLER_REVIEW_AGENT_TOKEN must differ from CRAWLER_REVIEW_AUTH_TOKEN")]
    AgentTokenEqualsReviewToken,
}

#[derive(Clone)]
pub struct ReviewServerConfig {
    pub bind_addr: SocketAddr,
    pub auth_token: Option<String>,
    /// The review agent's own bearer token: a request with it acts as [`ReviewActor::Agent`].
    /// Unset, nothing acts as the agent. It is accepted only next to a different `auth_token`,
    /// so a request never claims the agent without its token, and humans keep their own
    /// credential when it is revoked.
    pub agent_token: Option<String>,
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
        Self {
            bind_addr,
            auth_token: token_from_env("CRAWLER_REVIEW_AUTH_TOKEN"),
            agent_token: token_from_env("CRAWLER_REVIEW_AGENT_TOKEN"),
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
        self.local_mutations_without_token = self.auth_token.is_none()
            && self.agent_token.is_none()
            && self.bind_addr.ip().is_loopback();
        self
    }

    pub fn validate(self) -> Result<Self, ReviewServerConfigError> {
        if !self.bind_addr.ip().is_loopback() && self.auth_token.is_none() {
            return Err(ReviewServerConfigError::AuthenticationRequiredForNonLoopback);
        }
        if let Some(agent_token) = self.agent_token.as_deref() {
            let Some(auth_token) = self.auth_token.as_deref() else {
                return Err(ReviewServerConfigError::AgentTokenWithoutReviewToken);
            };
            if constant_time_eq(agent_token.as_bytes(), auth_token.as_bytes()) {
                return Err(ReviewServerConfigError::AgentTokenEqualsReviewToken);
            }
        }
        Ok(self)
    }
}

/// A token setting, where an empty or whitespace-only value counts as unset.
fn token_from_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|token| !token.trim().is_empty())
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

    /// Whether the review agent can act on the console, which the startup log reports.
    pub(super) fn agent_access(&self) -> bool {
        self.config.agent_token.is_some()
    }

    /// Authorizes a request and resolves who it acts as, or `None` when it is refused as
    /// unauthorized. The agent token reads the console API and makes review mutations, never
    /// opens a browser session and never administers domains; the review token, a browser
    /// session (reads only) and the `dev` binary's local same-origin mutations act as
    /// [`ReviewActor::Human`]. Without a configured review token the API is open to reads, which
    /// record nothing; only a configured agent token is ever resolved as the agent.
    pub(super) fn authorize(&self, request: &ParsedRequest<'_>) -> Option<ReviewActor> {
        if !request.path.starts_with("/api/") {
            return Some(ReviewActor::Human);
        }

        let bearer = self.bearer(&request.headers);
        let session = self.has_valid_session(&request.headers);
        let local_mutation = self.config.local_mutations_without_token
            && self.config.auth_token.is_none()
            && is_same_origin_loopback_request(&request.headers);
        match (request.method.as_str(), request.path) {
            ("POST", "/api/session") => {
                (bearer == Bearer::ReviewToken).then_some(ReviewActor::Human)
            }
            ("POST", "/api/session/logout") => {
                (bearer == Bearer::ReviewToken || session).then_some(ReviewActor::Human)
            }
            (method, path) if is_mutation_method(method) => match bearer {
                Bearer::ReviewToken => Some(ReviewActor::Human),
                Bearer::AgentToken if is_review_route(path) => Some(ReviewActor::Agent),
                Bearer::AgentToken | Bearer::None => local_mutation.then_some(ReviewActor::Human),
            },
            _ => match bearer {
                Bearer::ReviewToken => Some(ReviewActor::Human),
                Bearer::AgentToken => Some(ReviewActor::Agent),
                Bearer::None => {
                    (session || self.config.auth_token.is_none()).then_some(ReviewActor::Human)
                }
            },
        }
    }

    /// The configured token the request's bearer is. Both tokens are compared in constant time,
    /// each on every request, so the timing tells neither apart.
    fn bearer(&self, headers: &std::collections::HashMap<String, String>) -> Bearer {
        let Some(presented) = headers
            .get("authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
        else {
            return Bearer::None;
        };
        let review_token = token_matches(self.config.auth_token.as_deref(), presented);
        let agent_token = token_matches(self.config.agent_token.as_deref(), presented);
        if review_token {
            Bearer::ReviewToken
        } else if agent_token {
            Bearer::AgentToken
        } else {
            Bearer::None
        }
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

/// The configured token a request's bearer matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bearer {
    ReviewToken,
    AgentToken,
    None,
}

fn token_matches(expected: Option<&str>, presented: &str) -> bool {
    expected.is_some_and(|expected| constant_time_eq(presented.as_bytes(), expected.as_bytes()))
}

fn is_mutation_method(method: &str) -> bool {
    matches!(method, "POST" | "PUT" | "PATCH" | "DELETE")
}

/// The review routes the agent token may mutate: reviews and their pages, not domain
/// administration or sessions.
fn is_review_route(path: &str) -> bool {
    path.starts_with("/api/reviews/") || path.starts_with("/api/review-pages/")
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
    use crate::review::http::parse_request;

    #[test]
    fn should_keep_bearer_authorization_when_local_mode_has_token_or_public_bind() {
        let with_token = ReviewServerConfig {
            bind_addr: "127.0.0.1:7878".parse().unwrap(),
            auth_token: Some("review-token".to_string()),
            agent_token: None,
            local_mutations_without_token: false,
        }
        .allow_local_mutations_without_token();
        let public_bind = ReviewServerConfig {
            bind_addr: "0.0.0.0:7878".parse().unwrap(),
            auth_token: None,
            agent_token: None,
            local_mutations_without_token: false,
        }
        .allow_local_mutations_without_token();
        let with_agent_token = ReviewServerConfig {
            bind_addr: "127.0.0.1:7878".parse().unwrap(),
            auth_token: None,
            agent_token: Some(AGENT_TOKEN.to_string()),
            local_mutations_without_token: false,
        }
        .allow_local_mutations_without_token();

        assert!(!with_token.local_mutations_without_token);
        assert!(!public_bind.local_mutations_without_token);
        assert!(!with_agent_token.local_mutations_without_token);
    }

    #[test]
    fn should_require_authentication_for_non_loopback_review_bind() {
        let result = ReviewServerConfig {
            bind_addr: "0.0.0.0:7878".parse().unwrap(),
            auth_token: None,
            agent_token: None,
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
            agent_token: None,
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
            agent_token: None,
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
    const REVIEW_AGENT_TOKEN_ENV: &str = "CRAWLER_REVIEW_AGENT_TOKEN";
    const REVIEW_TOKEN: &str = "review-token";
    const AGENT_TOKEN: &str = "agent-token";

    /// Reads the review config from exactly these review settings, leaving them unset afterwards.
    fn review_config_from(
        bind_addr: Option<&str>,
        auth_token: Option<&str>,
        agent_token: Option<&str>,
    ) -> Result<ReviewServerConfig, ReviewServerConfigError> {
        set_env(REVIEW_BIND_ADDR_ENV, bind_addr);
        set_env(REVIEW_AUTH_TOKEN_ENV, auth_token);
        set_env(REVIEW_AGENT_TOKEN_ENV, agent_token);
        let config = ReviewServerConfig::from_env();
        set_env(REVIEW_BIND_ADDR_ENV, None);
        set_env(REVIEW_AUTH_TOKEN_ENV, None);
        set_env(REVIEW_AGENT_TOKEN_ENV, None);
        config
    }

    #[rstest::rstest]
    #[case::unset(None)]
    #[case::blank(Some("  "))]
    #[serial_test::serial]
    fn should_name_the_missing_token_when_a_non_loopback_bind_has_none(
        #[case] auth_token: Option<&'static str>,
    ) {
        let error = review_config_from(Some("0.0.0.0:7878"), auth_token, None)
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
        let error = review_config_from(Some("localhost"), None, None)
            .err()
            .unwrap_or_else(|| panic!("a bind address without a port must fail"));

        assert_eq!(
            error.to_string(),
            "CRAWLER_REVIEW_BIND_ADDR must be host:port"
        );
    }

    /// Startup refuses an agent token that would let a request claim the agent without its own
    /// credential, and names the settings, never their values.
    #[rstest::rstest]
    #[case::equal_to_the_review_token(
        Some(REVIEW_TOKEN),
        "CRAWLER_REVIEW_AGENT_TOKEN must differ from CRAWLER_REVIEW_AUTH_TOKEN"
    )]
    #[case::without_a_review_token(
        None,
        "CRAWLER_REVIEW_AGENT_TOKEN is accepted only together with CRAWLER_REVIEW_AUTH_TOKEN"
    )]
    #[case::without_a_review_token_but_blank(
        Some(" "),
        "CRAWLER_REVIEW_AGENT_TOKEN is accepted only together with CRAWLER_REVIEW_AUTH_TOKEN"
    )]
    #[serial_test::serial]
    fn should_refuse_to_start_with_an_agent_token_that_is_not_its_own_credential(
        #[case] auth_token: Option<&'static str>,
        #[case] message: &str,
    ) {
        let error = review_config_from(None, auth_token, Some(REVIEW_TOKEN))
            .err()
            .unwrap_or_else(|| panic!("the agent token must be refused"));

        assert_eq!(error.to_string(), message);
        assert!(!error.to_string().contains(REVIEW_TOKEN));
    }

    #[test]
    #[serial_test::serial]
    fn should_read_distinct_review_and_agent_tokens_and_a_blank_agent_token_as_unset() {
        let both = review_config_from(None, Some(REVIEW_TOKEN), Some(AGENT_TOKEN))
            .unwrap_or_else(|error| panic!("distinct tokens: {error}"));
        let blank_agent = review_config_from(None, Some(REVIEW_TOKEN), Some("  "))
            .unwrap_or_else(|error| panic!("blank agent token: {error}"));

        assert_eq!(both.auth_token.as_deref(), Some(REVIEW_TOKEN));
        assert_eq!(both.agent_token.as_deref(), Some(AGENT_TOKEN));
        assert_eq!(blank_agent.agent_token, None);
    }

    fn access_with_agent_token() -> ReviewAccess {
        ReviewAccess::new(
            ReviewServerConfig {
                bind_addr: "127.0.0.1:7878".parse().unwrap(),
                auth_token: Some(REVIEW_TOKEN.to_string()),
                agent_token: Some(AGENT_TOKEN.to_string()),
                local_mutations_without_token: false,
            }
            .validate()
            .unwrap_or_else(|error| panic!("review config: {error}")),
        )
    }

    fn actor_of(access: &ReviewAccess, request: &str, token: Option<&str>) -> Option<ReviewActor> {
        let authorization = token
            .map(|token| format!("Authorization: Bearer {token}\r\n"))
            .unwrap_or_default();
        let raw = format!("{request} HTTP/1.1\r\nHost: 127.0.0.1:7878\r\n{authorization}\r\n");
        let request =
            parse_request(&raw).unwrap_or_else(|| panic!("request does not parse: {raw}"));
        access.authorize(&request)
    }

    /// The agent token acts as the agent on the review API and nowhere else; the review token
    /// acts as a human; any other bearer is refused.
    #[rstest::rstest]
    #[case::review_token_reads("GET /api/reviews", Some(REVIEW_TOKEN), Some(ReviewActor::Human))]
    #[case::agent_token_reads("GET /api/reviews", Some(AGENT_TOKEN), Some(ReviewActor::Agent))]
    #[case::review_token_edits(
        "POST /api/reviews/cr_1/candidate",
        Some(REVIEW_TOKEN),
        Some(ReviewActor::Human)
    )]
    #[case::agent_token_edits(
        "POST /api/reviews/cr_1/candidate",
        Some(AGENT_TOKEN),
        Some(ReviewActor::Agent)
    )]
    #[case::agent_token_approves(
        "POST /api/reviews/cr_1/approve",
        Some(AGENT_TOKEN),
        Some(ReviewActor::Agent)
    )]
    #[case::agent_token_removes_a_review_page(
        "DELETE /api/review-pages/crp_1",
        Some(AGENT_TOKEN),
        Some(ReviewActor::Agent)
    )]
    #[case::agent_token_opens_no_browser_session("POST /api/session", Some(AGENT_TOKEN), None)]
    #[case::agent_token_closes_no_browser_session(
        "POST /api/session/logout",
        Some(AGENT_TOKEN),
        None
    )]
    #[case::agent_token_registers_no_domain(
        "POST /api/listing-sources/ls_1/domains",
        Some(AGENT_TOKEN),
        None
    )]
    #[case::agent_token_removes_no_domain(
        "DELETE /api/listing-sources/ls_1/domains/cd_1",
        Some(AGENT_TOKEN),
        None
    )]
    #[case::review_token_opens_a_browser_session(
        "POST /api/session",
        Some(REVIEW_TOKEN),
        Some(ReviewActor::Human)
    )]
    #[case::agent_token_prefix_reads("GET /api/reviews", Some("agent-toke"), None)]
    #[case::longer_agent_token_edits(
        "POST /api/reviews/cr_1/candidate",
        Some("agent-token-2"),
        None
    )]
    #[case::no_token_reads("GET /api/reviews", None, None)]
    #[case::no_token_edits("POST /api/reviews/cr_1/candidate", None, None)]
    fn should_resolve_the_actor_of_each_credential(
        #[case] request: &str,
        #[case] token: Option<&str>,
        #[case] expected: Option<ReviewActor>,
    ) {
        assert_eq!(
            actor_of(&access_with_agent_token(), request, token),
            expected
        );
    }

    /// The tokenless local mode of the `dev` binary has no agent: a same-origin mutation acts as
    /// a human whatever bearer it carries, and nothing else mutates.
    #[test]
    fn should_never_resolve_the_agent_in_the_tokenless_local_mode() {
        let access = ReviewAccess::new(
            ReviewServerConfig {
                bind_addr: "127.0.0.1:7878".parse().unwrap(),
                auth_token: None,
                agent_token: None,
                local_mutations_without_token: false,
            }
            .allow_local_mutations_without_token(),
        );
        let same_origin = |token: Option<&str>| {
            let authorization = token
                .map(|token| format!("Authorization: Bearer {token}\r\n"))
                .unwrap_or_default();
            let raw = format!(
                "POST /api/reviews/cr_1/candidate HTTP/1.1\r\nHost: 127.0.0.1:7878\r\nOrigin: http://127.0.0.1:7878\r\n{authorization}\r\n"
            );
            access.authorize(&parse_request(&raw).unwrap_or_else(|| panic!("{raw}")))
        };

        assert_eq!(same_origin(None), Some(ReviewActor::Human));
        assert_eq!(same_origin(Some(AGENT_TOKEN)), Some(ReviewActor::Human));
        assert_eq!(
            actor_of(
                &access,
                "POST /api/reviews/cr_1/candidate",
                Some(AGENT_TOKEN)
            ),
            None
        );
    }

    #[test]
    fn should_compare_authentication_tokens_without_early_value_match() {
        assert!(constant_time_eq(b"token", b"token"));
        assert!(!constant_time_eq(b"token", b"taken"));
        assert!(!constant_time_eq(b"token", b"tokens"));
    }
}
