//! The review console's settings: where it listens, the human review token, the agents' tokens
//! of `agent` authoring mode and the cap on live jobs, read from the environment and validated
//! before the console starts.

use crate::review::agent_tokens::ReviewAgentTokens;
use crate::review::live_jobs::{DEFAULT_MAX_CONCURRENT_LIVE_JOBS, MAX_CONCURRENT_LIVE_JOBS_ENV};
#[cfg(doc)]
use crate::review::model::ReviewActor;
use std::net::SocketAddr;
use std::num::NonZeroU32;

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
    pub(in crate::review) agent_tokens: Option<ReviewAgentTokens>,
    /// How many live jobs (live matrix refreshes, candidate evaluations and page projections) the
    /// console runs at once, for all reviews and actors together.
    pub max_concurrent_live_jobs: NonZeroU32,
    /// Lets same-origin browser requests mutate review state without a token on a
    /// loopback bind. Only the local `dev` binary opts in through
    /// [`ReviewServerConfig::allow_local_mutations_without_token`], which keeps it
    /// limited to a loopback bind without a token.
    pub(in crate::review) local_mutations_without_token: bool,
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

    /// This console opened to the author and reviewer agents with `tokens`: the review console of
    /// `agent` authoring mode, where the reviewer agent decides within the approval guard. The
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
pub(in crate::review) fn positive_number(
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

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(in crate::review::access) const REVIEW_TOKEN: &str = "review-token";
    pub(in crate::review::access) const AUTHOR_TOKEN: &str = "author-token";
    pub(in crate::review::access) const REVIEWER_TOKEN: &str = "reviewer-token";

    pub(in crate::review::access) fn config(
        bind_addr: &str,
        auth_token: Option<&str>,
    ) -> ReviewServerConfig {
        ReviewServerConfig {
            bind_addr: bind_addr.parse().unwrap(),
            auth_token: auth_token.map(str::to_owned),
            agent_tokens: None,
            local_mutations_without_token: false,
            max_concurrent_live_jobs: DEFAULT_MAX_CONCURRENT_LIVE_JOBS,
        }
    }

    pub(in crate::review::access) fn agent_tokens() -> ReviewAgentTokens {
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

    /// No console reads agent tokens from its settings: only `agent` authoring mode opens it to the
    /// agents it generated tokens for.
    #[test]
    #[serial_test::serial]
    fn should_open_the_console_read_from_its_settings_to_no_agent() {
        let config = review_config_from(None, Some(REVIEW_TOKEN))
            .unwrap_or_else(|error| panic!("review config: {error}"));

        assert_eq!(config.agent_tokens, None);
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
        }
        let error = refused
            .err()
            .unwrap_or_else(|| panic!("server's tokenless console must refuse the agents"));
        assert_eq!(
            error.to_string(),
            "CRAWLER_AUTHORING_MODE=agent needs CRAWLER_REVIEW_AUTH_TOKEN, so a human can still reject, restore and override what the review agents do"
        );
    }
}
