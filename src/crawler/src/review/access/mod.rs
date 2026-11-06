//! Who may use the review console and as which actor: the human review token, the browser
//! sessions that token opens, the author and reviewer agents' tokens of `agent` authoring mode
//! and, in the local `dev` binary only, same-origin mutations without a token. The operator
//! contract is in `docs/crawler/operations.md` (review service, review agents and review console
//! access).

mod bearer_token;
mod browser_session;
mod config;

pub(super) use config::positive_number;
pub use config::{ReviewServerConfig, ReviewServerConfigError};

use crate::review::http::{HttpResponse, ParsedRequest};
use crate::review::model::ReviewActor;
use bearer_token::{Bearer, BearerTokens};
use browser_session::BrowserSessions;
use std::net::SocketAddr;
use url::Url;

/// The review console's access control: its configuration, the tokens a bearer may present and
/// the browser sessions its bearer token opened.
#[derive(Clone)]
pub(super) struct ReviewAccess {
    config: ReviewServerConfig,
    bearer_tokens: BearerTokens,
    sessions: BrowserSessions,
}

impl ReviewAccess {
    pub(super) fn new(config: ReviewServerConfig) -> Self {
        Self {
            bearer_tokens: BearerTokens::of(&config),
            config,
            sessions: BrowserSessions::default(),
        }
    }

    pub(super) fn bind_addr(&self) -> SocketAddr {
        self.config.bind_addr
    }

    /// Whether the review agents can act on the console, which the startup log reports.
    pub(super) fn agent_access(&self) -> bool {
        self.config.agent_tokens.is_some()
    }

    /// Authorizes a request and resolves who it acts as, or `None` when it is refused as
    /// unauthorized. An agent token reads the console API and makes review mutations as its role,
    /// never opens or closes a browser session, never mutates ListingSource routes and never
    /// fetches a live preview; what each role may change is the repository's to refuse. The review
    /// token, a browser session (reads and logout only) and the `dev` binary's local same-origin
    /// mutations act as [`ReviewActor::Human`]. Without a configured review token the API is open
    /// to reads, which record nothing; only a generated agent token is ever resolved as an agent.
    /// An `Authorization` header that matches no token is refused on every API route: a supplied
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
            (_, path) => match bearer {
                Bearer::ReviewToken => Some(ReviewActor::Human),
                Bearer::Agent(agent) => (!is_live_preview_route(path)).then_some(agent),
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
        self.sessions.is_valid(headers)
    }

    pub(super) fn create_session(&self) -> HttpResponse {
        self.sessions.open(&self.config)
    }

    pub(super) fn clear_session(&self, request: &ParsedRequest<'_>) -> HttpResponse {
        self.sessions.close(&self.config, request)
    }
}

fn is_mutation_method(method: &str) -> bool {
    matches!(method, "POST" | "PUT" | "PATCH" | "DELETE")
}

/// The review routes the agent tokens may mutate: reviews and their pages, not ListingSource
/// routes or sessions.
fn is_review_route(path: &str) -> bool {
    path.starts_with("/api/reviews/") || path.starts_with("/api/review-pages/")
}

/// The live previews, of any URL (`/api/live-inspect`, `/api/live-html`) and of a review page
/// (`/api/review-pages/{id}/inspect`, `/html`), which only the browser console uses. They fetch a
/// page outside the console's cap on live jobs and without asking whether its review waits for a
/// human; the agents read a page as a review's page projection, a live job, instead.
fn is_live_preview_route(path: &str) -> bool {
    matches!(path, "/api/live-inspect" | "/api/live-html")
        || (path.starts_with("/api/review-pages/")
            && (path.ends_with("/inspect") || path.ends_with("/html")))
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
    use super::config::tests::{AUTHOR_TOKEN, REVIEW_TOKEN, REVIEWER_TOKEN, agent_tokens, config};
    use super::*;
    use crate::review::http::parse_request;

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
    #[case::review_token_confirms_a_mass_removal(
        "POST /api/listing-sources/ls_1/mass-removal-confirmation",
        Some(REVIEW_TOKEN),
        Some(ReviewActor::Human)
    )]
    #[case::author_token_confirms_no_mass_removal(
        "POST /api/listing-sources/ls_1/mass-removal-confirmation",
        Some(AUTHOR_TOKEN),
        None
    )]
    #[case::reviewer_token_revokes_no_mass_removal_confirmation(
        "DELETE /api/listing-sources/ls_1/mass-removal-confirmation",
        Some(REVIEWER_TOKEN),
        None
    )]
    #[case::author_token_opens_no_missing_value_page_review(
        "POST /api/listing-sources/ls_1/missing-value-pages/review",
        Some(AUTHOR_TOKEN),
        None
    )]
    #[case::reviewer_token_opens_no_missing_value_page_review(
        "POST /api/listing-sources/ls_1/missing-value-pages/review",
        Some(REVIEWER_TOKEN),
        None
    )]
    #[case::author_token_fetches_no_live_preview(
        "GET /api/live-inspect?url=https%3A%2F%2Fshop.example%2Fitem&review=cr_1",
        Some(AUTHOR_TOKEN),
        None
    )]
    #[case::reviewer_token_fetches_no_live_html(
        "GET /api/live-html?url=https%3A%2F%2Fshop.example%2Fitem&review=cr_1",
        Some(REVIEWER_TOKEN),
        None
    )]
    #[case::review_token_fetches_a_live_preview(
        "GET /api/live-inspect?url=https%3A%2F%2Fshop.example%2Fitem",
        Some(REVIEW_TOKEN),
        Some(ReviewActor::Human)
    )]
    #[case::author_token_previews_no_review_page(
        "GET /api/review-pages/crp_1/inspect",
        Some(AUTHOR_TOKEN),
        None
    )]
    #[case::reviewer_token_fetches_no_review_page_html(
        "GET /api/review-pages/crp_1/html",
        Some(REVIEWER_TOKEN),
        None
    )]
    #[case::review_token_previews_a_review_page(
        "GET /api/review-pages/crp_1/inspect",
        Some(REVIEW_TOKEN),
        Some(ReviewActor::Human)
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
}
