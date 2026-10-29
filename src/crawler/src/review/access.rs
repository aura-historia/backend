//! Who may use the review console and as which actor: the human review token, the browser
//! sessions that token opens, the author and reviewer agents' tokens of `agent` authoring mode
//! and, in the local `dev` binary only, same-origin mutations without a token. The operator
//! contract is in `docs/crawler/operations.md` (review service, review agents and review console
//! access).

use crate::review::agent_decisions::{AgentApprovals, AgentDecisionPolicy};
use crate::review::agent_tokens::ReviewAgentTokens;
use crate::review::http::{HttpResponse, ParsedRequest};
use crate::review::live_jobs::{DEFAULT_MAX_CONCURRENT_LIVE_JOBS, MAX_CONCURRENT_LIVE_JOBS_ENV};
use crate::review::model::ReviewActor;
use dashmap::DashMap;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;
use subtle::ConstantTimeEq;
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum ReviewServerConfigError {
    #[error("CRAWLER_REVIEW_BIND_ADDR must be host:port")]
    InvalidBindAddress(#[from] std::net::AddrParseError),
    #[error("CRAWLER_REVIEW_AUTH_TOKEN is required for non-loopback bind addresses")]
    AuthenticationRequiredForNonLoopback,
    #[error(
        "CRAWLER_AUTHORING_MODE=agent needs CRAWLER_REVIEW_AUTH_TOKEN, so a human can still reject, restore and override what the review agents do"
    )]
    ReviewAgentsWithoutHumanCredential,
    #[error("invalid {name} value {value:?}; expected a whole number of at least 1")]
    InvalidPositiveNumber { name: &'static str, value: String },
}

#[derive(Clone)]
pub struct ReviewServerConfig {
    pub bind_addr: SocketAddr,
    pub auth_token: Option<String>,
    /// The author and reviewer agents' tokens of this start, which `agent` authoring mode
    /// generates (see [`ReviewServerConfig::with_review_agents`]): a request with one acts as
    /// [`ReviewActor::AgentAuthor`] or [`ReviewActor::AgentReviewer`]. `None` in every other mode,
    /// where nothing acts as an agent.
    pub(super) agent_tokens: Option<ReviewAgentTokens>,
    /// What the agents may decide on their own; with the default they decide nothing.
    pub agent_decisions: AgentDecisionPolicy,
    /// How many live jobs (live matrix refreshes, candidate evaluations and page projections) the
    /// console runs at once, for all reviews and actors together.
    pub max_concurrent_live_jobs: NonZeroU32,
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
            agent_tokens: None,
            agent_decisions: AgentDecisionPolicy::from_env()?,
            max_concurrent_live_jobs: positive_number(MAX_CONCURRENT_LIVE_JOBS_ENV)?
                .unwrap_or(DEFAULT_MAX_CONCURRENT_LIVE_JOBS),
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

    /// This console opened to the author and reviewer agents with `tokens`, the reviewer agent
    /// deciding within the approval guard: the review console of `agent` authoring mode. The
    /// agents work next to a human credential for mutations, the review token or the `dev`
    /// binary's tokenless local console, so a human can always reject, restore and override what
    /// they do; without one it is refused. An unauthenticated request never acts as an agent.
    pub fn with_review_agents(
        self,
        tokens: ReviewAgentTokens,
    ) -> Result<Self, ReviewServerConfigError> {
        if self.auth_token.is_none() && !self.local_mutations_without_token {
            return Err(ReviewServerConfigError::ReviewAgentsWithoutHumanCredential);
        }
        Ok(Self {
            agent_tokens: Some(tokens),
            agent_decisions: AgentDecisionPolicy {
                approvals: AgentApprovals::Guarded,
                ..self.agent_decisions
            },
            ..self
        })
    }
}

/// The trimmed value of `name`, `None` when it is unset, empty or whitespace-only. A value that
/// is not valid Unicode is returned as `Err` to be refused like any other value not accepted.
fn setting(name: &str) -> Result<Option<String>, String> {
    match std::env::var(name) {
        Ok(raw) if raw.trim().is_empty() => Ok(None),
        Ok(raw) => Ok(Some(raw.trim().to_owned())),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(raw)) => Err(raw.to_string_lossy().into_owned()),
    }
}

/// A whole-number setting of at least 1, `None` when it is unset, empty or whitespace-only. A
/// value that is set but not accepted fails startup instead of silently changing a limit.
pub(super) fn positive_number(
    name: &'static str,
) -> Result<Option<NonZeroU32>, ReviewServerConfigError> {
    setting(name)
        .and_then(|raw| {
            raw.map(|raw| raw.parse::<NonZeroU32>().map_err(|_| raw))
                .transpose()
        })
        .map_err(|value| ReviewServerConfigError::InvalidPositiveNumber { name, value })
}

/// A token setting, where an empty or whitespace-only value counts as unset.
fn token_from_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|token| !token.trim().is_empty())
}

const REVIEW_SESSION_TTL: Duration = Duration::from_secs(8 * 60 * 60);
const REVIEW_SESSION_COOKIE: &str = "crawler_review_session";

/// The review console's access control: its configuration, the tokens a bearer may present and
/// the browser sessions its bearer token opened.
#[derive(Clone)]
pub(super) struct ReviewAccess {
    config: ReviewServerConfig,
    bearer_tokens: BearerTokens,
    sessions: Arc<DashMap<uuid::Uuid, std::time::Instant>>,
}

impl ReviewAccess {
    pub(super) fn new(config: ReviewServerConfig) -> Self {
        Self {
            bearer_tokens: BearerTokens::of(&config),
            config,
            sessions: Arc::new(DashMap::new()),
        }
    }

    pub(super) fn bind_addr(&self) -> SocketAddr {
        self.config.bind_addr
    }

    /// Whether the review agents can act on the console, which the startup log reports.
    pub(super) fn agent_access(&self) -> bool {
        self.config.agent_tokens.is_some()
    }

    /// What the review agents may decide, which the startup log reports.
    pub(super) fn agent_decisions(&self) -> &AgentDecisionPolicy {
        &self.config.agent_decisions
    }

    /// Authorizes a request and resolves who it acts as, or `None` when it is refused as
    /// unauthorized. An agent token reads the console API and makes review mutations as its
    /// role, never opens or closes a browser session and never administers domains; what each
    /// role may change is the repository's to refuse. The review token, a browser session (reads
    /// and logout only) and the `dev` binary's local same-origin mutations act as
    /// [`ReviewActor::Human`]. Without a configured review token the API is open to reads, which
    /// record nothing; only a generated agent token is ever resolved as an agent. An
    /// `Authorization` header that matches no token is refused on every API route: a supplied
    /// credential that fails is never downgraded to a session or to tokenless access.
    pub(super) fn authorize(&self, request: &ParsedRequest<'_>) -> Option<ReviewActor> {
        if !request.path.starts_with("/api/") {
            return Some(ReviewActor::Human);
        }

        let bearer = self.bearer(&request.headers);
        let session = || self.has_valid_session(&request.headers);
        let local_mutation = || {
            self.config.local_mutations_without_token
                && self.config.auth_token.is_none()
                && is_same_origin_loopback_request(&request.headers)
        };
        match (request.method.as_str(), request.path) {
            ("POST", "/api/session") => {
                (bearer == Bearer::ReviewToken).then_some(ReviewActor::Human)
            }
            ("POST", "/api/session/logout") => match bearer {
                Bearer::ReviewToken => Some(ReviewActor::Human),
                Bearer::Absent => session().then_some(ReviewActor::Human),
                Bearer::Agent(_) | Bearer::Invalid => None,
            },
            (method, path) if is_mutation_method(method) => match bearer {
                Bearer::ReviewToken => Some(ReviewActor::Human),
                Bearer::Agent(agent) => is_review_route(path).then_some(agent),
                Bearer::Absent => local_mutation().then_some(ReviewActor::Human),
                Bearer::Invalid => None,
            },
            _ => match bearer {
                Bearer::ReviewToken => Some(ReviewActor::Human),
                Bearer::Agent(agent) => Some(agent),
                Bearer::Absent => {
                    (session() || self.config.auth_token.is_none()).then_some(ReviewActor::Human)
                }
                Bearer::Invalid => None,
            },
        }
    }

    /// The token the request's `Authorization` header presents; a header that is not a bearer of
    /// one of the console's tokens is [`Bearer::Invalid`].
    fn bearer(&self, headers: &std::collections::HashMap<String, String>) -> Bearer {
        let Some(authorization) = headers.get("authorization") else {
            return Bearer::Absent;
        };
        let Some(presented) = authorization.strip_prefix("Bearer ") else {
            return Bearer::Invalid;
        };
        self.bearer_tokens.resolve(presented)
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

/// The credential a request's `Authorization` header presents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bearer {
    ReviewToken,
    /// The token of the review agent acting as this actor.
    Agent(ReviewActor),
    /// The request has no `Authorization` header.
    Absent,
    /// The header is not a bearer of a configured token.
    Invalid,
}

/// The tokens a bearer may present: the review token and, in `agent` authoring mode, the author
/// and reviewer agents' tokens, each kept as its digest.
#[derive(Clone, Copy)]
struct BearerTokens {
    review: Option<TokenDigest>,
    author: Option<TokenDigest>,
    reviewer: Option<TokenDigest>,
}

impl BearerTokens {
    fn of(config: &ReviewServerConfig) -> Self {
        let agents = config.agent_tokens.as_ref();
        Self {
            review: config.auth_token.as_deref().map(TokenDigest::of),
            author: agents.map(|tokens| TokenDigest::of(tokens.author())),
            reviewer: agents.map(|tokens| TokenDigest::of(tokens.reviewer())),
        }
    }

    /// Which token `presented` is. It is compared with every token, each on every request, so the
    /// timing tells none apart.
    fn resolve(&self, presented: &str) -> Bearer {
        let presented = TokenDigest::of(presented);
        let matches =
            |token: Option<TokenDigest>| token.is_some_and(|token| token.matches(presented));
        match (
            matches(self.review),
            matches(self.author),
            matches(self.reviewer),
        ) {
            (true, _, _) => Bearer::ReviewToken,
            (false, true, _) => Bearer::Agent(ReviewActor::AgentAuthor),
            (false, false, true) => Bearer::Agent(ReviewActor::AgentReviewer),
            (false, false, false) => Bearer::Invalid,
        }
    }
}

/// A token's SHA-256 digest. Comparing digests instead of the tokens takes the same time whatever
/// either token's length, so it tells a client nothing about a configured token's length; a
/// configured token is hashed once, when the console starts, and a request hashes only the value
/// it presents.
#[derive(Clone, Copy)]
struct TokenDigest([u8; 32]);

impl TokenDigest {
    fn of(token: &str) -> Self {
        Self(Sha256::digest(token.as_bytes()).into())
    }

    /// Whether both are the digest of the same token. `subtle` reads every byte whatever the
    /// first difference and keeps the optimizer from turning the comparison into an early exit.
    fn matches(self, other: Self) -> bool {
        self.0.as_slice().ct_eq(other.0.as_slice()).into()
    }
}

fn is_mutation_method(method: &str) -> bool {
    matches!(method, "POST" | "PUT" | "PATCH" | "DELETE")
}

/// The review routes the agent tokens may mutate: reviews and their pages, not domain
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::http::parse_request;

    const REVIEW_TOKEN: &str = "review-token";
    const AUTHOR_TOKEN: &str = "author-token";
    const REVIEWER_TOKEN: &str = "reviewer-token";

    fn config(bind_addr: &str, auth_token: Option<&str>) -> ReviewServerConfig {
        ReviewServerConfig {
            bind_addr: bind_addr.parse().unwrap(),
            auth_token: auth_token.map(str::to_owned),
            agent_tokens: None,
            agent_decisions: AgentDecisionPolicy::default(),
            local_mutations_without_token: false,
            max_concurrent_live_jobs: DEFAULT_MAX_CONCURRENT_LIVE_JOBS,
        }
    }

    fn agent_tokens() -> ReviewAgentTokens {
        ReviewAgentTokens::of(AUTHOR_TOKEN, REVIEWER_TOKEN)
    }

    #[test]
    fn should_keep_bearer_authorization_when_local_mode_has_token_or_public_bind() {
        let with_token =
            config("127.0.0.1:7878", Some(REVIEW_TOKEN)).allow_local_mutations_without_token();
        let public_bind = config("0.0.0.0:7878", None).allow_local_mutations_without_token();
        let tokenless_loopback =
            config("127.0.0.1:7878", None).allow_local_mutations_without_token();

        assert!(!with_token.local_mutations_without_token);
        assert!(!public_bind.local_mutations_without_token);
        assert!(tokenless_loopback.local_mutations_without_token);
    }

    #[test]
    fn should_require_authentication_for_non_loopback_review_bind() {
        let result = config("0.0.0.0:7878", None).validate();

        assert!(matches!(
            result,
            Err(ReviewServerConfigError::AuthenticationRequiredForNonLoopback)
        ));
    }

    #[test]
    fn should_allow_non_loopback_review_bind_with_authentication_token() {
        assert!(
            config("0.0.0.0:7878", Some(REVIEW_TOKEN))
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn should_allow_loopback_review_bind_without_authentication_token() {
        assert!(config("127.0.0.1:7878", None).validate().is_ok());
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

    /// The console's live-job limit defaults to four; a value that is set but not a whole number
    /// of at least 1 fails startup with the variable and the value.
    #[test]
    #[serial_test::serial]
    fn should_read_the_live_job_limit_and_refuse_a_value_that_is_not_accepted() {
        let read = |value: Option<&str>| {
            set_env(MAX_CONCURRENT_LIVE_JOBS_ENV, value);
            let config = review_config_from(None, None);
            set_env(MAX_CONCURRENT_LIVE_JOBS_ENV, None);
            config
        };
        let limit = |value: Option<&str>| {
            read(value)
                .map(|config| config.max_concurrent_live_jobs.get())
                .unwrap_or_else(|error| panic!("{value:?}: {error}"))
        };

        assert_eq!(limit(None), 4);
        assert_eq!(limit(Some(" ")), 4);
        assert_eq!(limit(Some(" 2 ")), 2);
        for refused in ["0", "-1", "two"] {
            assert_eq!(
                read(Some(refused)).err().map(|error| error.to_string()),
                Some(format!(
                    "invalid CRAWLER_REVIEW_MAX_CONCURRENT_LIVE_JOBS value \"{refused}\"; expected a whole number of at least 1"
                ))
            );
        }
    }

    /// No console reads agent tokens or turns agent decisions on from its settings: only `agent`
    /// authoring mode opens it to the agents it generated tokens for.
    #[test]
    #[serial_test::serial]
    fn should_open_the_console_read_from_its_settings_to_no_agent() {
        let config = review_config_from(None, Some(REVIEW_TOKEN))
            .unwrap_or_else(|error| panic!("review config: {error}"));

        assert_eq!(config.agent_tokens, None);
        assert_eq!(config.agent_decisions.approvals, AgentApprovals::Off);
    }

    /// The agents work next to a human credential for mutations, so a human can always decide
    /// over them: the review token, or the `dev` binary's tokenless local console, and never
    /// `server`'s tokenless loopback console, where nobody could mutate as a human.
    #[test]
    fn should_open_the_console_to_the_agents_only_next_to_a_human_credential() {
        let with_review_token = config("127.0.0.1:7878", Some(REVIEW_TOKEN))
            .with_review_agents(agent_tokens())
            .unwrap_or_else(|error| panic!("review token: {error}"));
        let local_console = config("127.0.0.1:7878", None)
            .allow_local_mutations_without_token()
            .with_review_agents(agent_tokens())
            .unwrap_or_else(|error| panic!("dev local console: {error}"));
        let refused = config("127.0.0.1:7878", None).with_review_agents(agent_tokens());

        for opened in [&with_review_token, &local_console] {
            assert_eq!(opened.agent_tokens, Some(agent_tokens()));
            assert_eq!(opened.agent_decisions.approvals, AgentApprovals::Guarded);
        }
        let error = refused
            .err()
            .unwrap_or_else(|| panic!("server's tokenless console must refuse the agents"));
        assert_eq!(
            error.to_string(),
            "CRAWLER_AUTHORING_MODE=agent needs CRAWLER_REVIEW_AUTH_TOKEN, so a human can still reject, restore and override what the review agents do"
        );
    }

    fn access_with_agents() -> ReviewAccess {
        ReviewAccess::new(
            config("127.0.0.1:7878", Some(REVIEW_TOKEN))
                .validate()
                .and_then(|config| config.with_review_agents(agent_tokens()))
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

    /// Each agent token acts as its role on the review API and nowhere else; the review token
    /// acts as a human; any other bearer is refused. What each role may change on a review is the
    /// repository's to refuse.
    #[rstest::rstest]
    #[case::review_token_reads("GET /api/reviews", Some(REVIEW_TOKEN), Some(ReviewActor::Human))]
    #[case::author_token_reads(
        "GET /api/reviews",
        Some(AUTHOR_TOKEN),
        Some(ReviewActor::AgentAuthor)
    )]
    #[case::reviewer_token_refreshes_the_matrix(
        "GET /api/reviews/cr_1/matrix?refresh=true",
        Some(REVIEWER_TOKEN),
        Some(ReviewActor::AgentReviewer)
    )]
    #[case::review_token_edits(
        "POST /api/reviews/cr_1/candidate",
        Some(REVIEW_TOKEN),
        Some(ReviewActor::Human)
    )]
    #[case::author_token_edits(
        "POST /api/reviews/cr_1/candidate",
        Some(AUTHOR_TOKEN),
        Some(ReviewActor::AgentAuthor)
    )]
    #[case::reviewer_token_approves(
        "POST /api/reviews/cr_1/approve",
        Some(REVIEWER_TOKEN),
        Some(ReviewActor::AgentReviewer)
    )]
    #[case::author_token_removes_a_review_page(
        "DELETE /api/review-pages/crp_1",
        Some(AUTHOR_TOKEN),
        Some(ReviewActor::AgentAuthor)
    )]
    #[case::author_token_opens_no_browser_session("POST /api/session", Some(AUTHOR_TOKEN), None)]
    #[case::reviewer_token_opens_no_browser_session(
        "POST /api/session",
        Some(REVIEWER_TOKEN),
        None
    )]
    #[case::reviewer_token_closes_no_browser_session(
        "POST /api/session/logout",
        Some(REVIEWER_TOKEN),
        None
    )]
    #[case::author_token_registers_no_domain(
        "POST /api/listing-sources/ls_1/domains",
        Some(AUTHOR_TOKEN),
        None
    )]
    #[case::reviewer_token_removes_no_domain(
        "DELETE /api/listing-sources/ls_1/domains/cd_1",
        Some(REVIEWER_TOKEN),
        None
    )]
    #[case::review_token_opens_a_browser_session(
        "POST /api/session",
        Some(REVIEW_TOKEN),
        Some(ReviewActor::Human)
    )]
    #[case::author_token_prefix_reads("GET /api/reviews", Some("author-toke"), None)]
    #[case::longer_reviewer_token_approves(
        "POST /api/reviews/cr_1/approve",
        Some("reviewer-token-2"),
        None
    )]
    #[case::no_token_reads("GET /api/reviews", None, None)]
    #[case::no_token_edits("POST /api/reviews/cr_1/candidate", None, None)]
    fn should_resolve_the_actor_of_each_credential(
        #[case] request: &str,
        #[case] token: Option<&str>,
        #[case] expected: Option<ReviewActor>,
    ) {
        assert_eq!(actor_of(&access_with_agents(), request, token), expected);
    }

    /// Without `agent` mode no request acts as an agent, whatever bearer it presents.
    #[test]
    fn should_resolve_no_agent_without_generated_agent_tokens() {
        let access = ReviewAccess::new(
            config("127.0.0.1:7878", Some(REVIEW_TOKEN))
                .validate()
                .unwrap_or_else(|error| panic!("review config: {error}")),
        );

        for token in [AUTHOR_TOKEN, REVIEWER_TOKEN] {
            assert_eq!(
                actor_of(&access, "POST /api/reviews/cr_1/approve", Some(token)),
                None
            );
            assert_eq!(actor_of(&access, "GET /api/reviews", Some(token)), None);
        }
    }

    /// The `dev` binary's tokenless local console in `agent` mode: a same-origin mutation without
    /// a bearer acts as a human, each agent acts only with its own token, and an unauthenticated
    /// request never acts as an agent.
    #[test]
    fn should_keep_the_tokenless_local_console_human_next_to_the_agents() {
        let access = ReviewAccess::new(
            config("127.0.0.1:7878", None)
                .allow_local_mutations_without_token()
                .with_review_agents(agent_tokens())
                .unwrap_or_else(|error| panic!("dev agent console: {error}")),
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
        assert_eq!(
            same_origin(Some(AUTHOR_TOKEN)),
            Some(ReviewActor::AgentAuthor)
        );
        assert_eq!(
            same_origin(Some(REVIEWER_TOKEN)),
            Some(ReviewActor::AgentReviewer)
        );
        assert_eq!(same_origin(Some("stale-author-token")), None);
        assert_eq!(
            actor_of(&access, "POST /api/reviews/cr_1/candidate", None),
            None,
            "a mutation without a bearer and from no same-origin page acts as nobody"
        );
        assert_eq!(
            actor_of(&access, "GET /api/reviews", None),
            Some(ReviewActor::Human)
        );
    }

    /// Sends `request` with exactly these `Authorization` and session cookie headers.
    fn actor_with_headers(
        access: &ReviewAccess,
        request: &str,
        authorization: Option<&str>,
        session_cookie: Option<&str>,
    ) -> Option<ReviewActor> {
        let authorization = authorization
            .map(|value| format!("Authorization: {value}\r\n"))
            .unwrap_or_default();
        let cookie = session_cookie
            .map(|cookie| format!("Cookie: {cookie}\r\n"))
            .unwrap_or_default();
        let raw =
            format!("{request} HTTP/1.1\r\nHost: 127.0.0.1:7878\r\n{authorization}{cookie}\r\n");
        access.authorize(&parse_request(&raw).unwrap_or_else(|| panic!("{raw}")))
    }

    /// The `name=value` pair of the session cookie a new browser session sets.
    fn new_session_cookie(access: &ReviewAccess) -> String {
        let response = access.create_session().to_string();
        response
            .lines()
            .find_map(|line| line.strip_prefix("Set-Cookie: "))
            .and_then(|cookie| cookie.split(';').next())
            .unwrap_or_else(|| panic!("session response without cookie: {response}"))
            .to_owned()
    }

    /// A browser session reads and logs out as a human, but a request that also presents an
    /// `Authorization` header matching no token is refused, not downgraded to the session; an
    /// agent token never closes a session either, and a session never mutates.
    #[rstest::rstest]
    #[case::session_reads("GET /api/reviews", None, Some(ReviewActor::Human))]
    #[case::session_logs_out("POST /api/session/logout", None, Some(ReviewActor::Human))]
    #[case::session_never_mutates("POST /api/reviews/cr_1/candidate", None, None)]
    #[case::wrong_bearer_with_session_reads("GET /api/reviews", Some("Bearer stale-token"), None)]
    #[case::wrong_bearer_with_session_logs_out(
        "POST /api/session/logout",
        Some("Bearer stale-token"),
        None
    )]
    #[case::another_scheme_with_session_reads(
        "GET /api/reviews",
        Some("Basic cmV2aWV3LXRva2Vu"),
        None
    )]
    #[case::author_token_with_session_logs_out(
        "POST /api/session/logout",
        Some("Bearer author-token"),
        None
    )]
    #[case::reviewer_token_with_session_reads(
        "GET /api/reviews",
        Some("Bearer reviewer-token"),
        Some(ReviewActor::AgentReviewer)
    )]
    fn should_never_downgrade_a_failed_bearer_to_the_browser_session(
        #[case] request: &str,
        #[case] authorization: Option<&str>,
        #[case] expected: Option<ReviewActor>,
    ) {
        let access = access_with_agents();
        let cookie = new_session_cookie(&access);

        assert_eq!(
            actor_with_headers(&access, request, authorization, Some(&cookie)),
            expected
        );
    }

    /// Without a review token the API is open to reads, but not to a request presenting a
    /// credential, which nothing can match.
    #[test]
    fn should_refuse_a_bearer_where_no_token_is_configured() {
        let access = ReviewAccess::new(
            config("127.0.0.1:7878", None)
                .validate()
                .unwrap_or_else(|error| panic!("review config: {error}")),
        );

        assert_eq!(
            actor_of(&access, "GET /api/reviews", None),
            Some(ReviewActor::Human)
        );
        assert_eq!(
            actor_of(&access, "GET /api/reviews", Some(AUTHOR_TOKEN)),
            None
        );
    }

    /// A presented token matches only the token it equals; one of another length is compared like
    /// any other value.
    #[rstest::rstest]
    #[case::equal("review-token", true)]
    #[case::different_of_equal_length("review-tokem", false)]
    #[case::shorter("review-toke", false)]
    #[case::longer("review-token-2", false)]
    #[case::empty("", false)]
    fn should_match_a_presented_token_only_when_it_equals_the_configured_one(
        #[case] presented: &str,
        #[case] expected: bool,
    ) {
        assert_eq!(
            TokenDigest::of(REVIEW_TOKEN).matches(TokenDigest::of(presented)),
            expected
        );
    }
}
