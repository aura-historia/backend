//! How `review-mcp` is started: the one agent role it serves, from `--role`, the one review it may
//! work, from `--review`, and from its environment the review console it calls and the token file
//! it reads its role's token from.
//! Tokens are never settings: they come from the token file only. The review agent runner reaches
//! the console the same way and reads the same two settings through [`ReviewConsoleAccess`].

use crate::CrawlerReviewId;
use crate::review::agent_tokens::REVIEW_AGENT_TOKEN_FILE;
use std::fmt;
use std::path::PathBuf;
use url::Url;

/// The review console `review-mcp` calls unless `CRAWLER_REVIEW_CONSOLE_URL` names another.
pub const DEFAULT_REVIEW_CONSOLE_URL: &str = "http://127.0.0.1:7878";
pub const REVIEW_CONSOLE_URL_ENV: &str = "CRAWLER_REVIEW_CONSOLE_URL";
pub const REVIEW_AGENT_TOKEN_FILE_ENV: &str = "CRAWLER_REVIEW_AGENT_TOKEN_FILE";

/// The review agent a `review-mcp` process serves: the author agent stages candidates, the
/// reviewer agent decides them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRole {
    Author,
    Reviewer,
}

impl AgentRole {
    /// The role's name in `--role` and its key in the token file.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Author => "author",
            Self::Reviewer => "reviewer",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        [Self::Author, Self::Reviewer]
            .into_iter()
            .find(|role| role.as_str() == value)
    }
}

impl fmt::Display for AgentRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The reviews one `review-mcp` process works on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewScope {
    /// Every review of the console, for an MCP client a person drives.
    AnyReview,
    /// Only this review. The runner starts each agent process for one review, so page content
    /// the agent reads cannot turn it on any other review.
    Only(CrawlerReviewId),
}

impl ReviewScope {
    /// Whether a tool call on `review_id` stays within the scope.
    pub fn admits(self, review_id: CrawlerReviewId) -> bool {
        match self {
            Self::AnyReview => true,
            Self::Only(scoped) => scoped == review_id,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReviewMcpSettingsError {
    #[error("usage: review-mcp --role author|reviewer [--review <review id>]; {0}")]
    Usage(String),
    /// The value itself is not repeated: a refused one may carry credentials or a token.
    #[error(
        "invalid {REVIEW_CONSOLE_URL_ENV}: {reason}; expected the review console's base URL, such as {DEFAULT_REVIEW_CONSOLE_URL}"
    )]
    InvalidConsoleUrl { reason: &'static str },
}

/// The settings of one `review-mcp` process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewMcpSettings {
    pub role: AgentRole,
    pub scope: ReviewScope,
    /// The console's base URL, without a trailing slash.
    pub console_url: Url,
    pub token_file: PathBuf,
}

impl ReviewMcpSettings {
    /// Reads the settings from the command-line `args` (without the program name), the
    /// environment through `env`, and, for the default token file, the `state_directory`, the
    /// crawler's working directory. `--role` is required, `--review` optional, and nothing else is
    /// accepted, so no credential can be passed as an argument. An empty or whitespace-only
    /// setting counts as unset.
    pub fn from_args_and_env(
        args: impl IntoIterator<Item = String>,
        env: impl Fn(&str) -> Option<String>,
        state_directory: impl FnOnce() -> PathBuf,
    ) -> Result<Self, ReviewMcpSettingsError> {
        let (role, scope) = command_line(args)?;
        let ReviewConsoleAccess {
            console_url,
            token_file,
        } = ReviewConsoleAccess::from_env(env, state_directory)?;
        Ok(Self {
            role,
            scope,
            console_url,
            token_file,
        })
    }
}

/// How a client of `agent` authoring mode reaches the review console: the console's base URL and
/// the token file the crawler writes the agents' tokens to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewConsoleAccess {
    /// The console's base URL, without a trailing slash.
    pub console_url: Url,
    pub token_file: PathBuf,
}

impl ReviewConsoleAccess {
    /// Reads `CRAWLER_REVIEW_CONSOLE_URL` and `CRAWLER_REVIEW_AGENT_TOKEN_FILE` through `env`; the
    /// token file defaults to the crawler's in the `state_directory`, the crawler's working
    /// directory. An empty or whitespace-only setting counts as unset.
    pub fn from_env(
        env: impl Fn(&str) -> Option<String>,
        state_directory: impl FnOnce() -> PathBuf,
    ) -> Result<Self, ReviewMcpSettingsError> {
        let setting = |name: &str| env(name).filter(|value| !value.trim().is_empty());
        let console_url = console_url(
            setting(REVIEW_CONSOLE_URL_ENV)
                .as_deref()
                .unwrap_or(DEFAULT_REVIEW_CONSOLE_URL),
        )?;
        let token_file = setting(REVIEW_AGENT_TOKEN_FILE_ENV).map_or_else(
            || state_directory().join(REVIEW_AGENT_TOKEN_FILE),
            |path| PathBuf::from(path.trim()),
        );
        Ok(Self {
            console_url,
            token_file,
        })
    }
}

/// The role from `--role`, required, and the scope from `--review`, optional; each at most once,
/// as `--flag value` or `--flag=value`.
fn command_line(
    args: impl IntoIterator<Item = String>,
) -> Result<(AgentRole, ReviewScope), ReviewMcpSettingsError> {
    let usage = |reason: String| ReviewMcpSettingsError::Usage(reason);
    let mut args = args.into_iter();
    let mut role = None;
    let mut review = None;
    while let Some(arg) = args.next() {
        let Some((flag, inline)) = ["--role", "--review"].into_iter().find_map(|flag| {
            arg.strip_prefix(flag)
                .filter(|rest| rest.is_empty() || rest.starts_with('='))
                .map(|rest| (flag, rest))
        }) else {
            return Err(usage(format!("unexpected argument {arg:?}")));
        };
        let value = match inline.strip_prefix('=') {
            Some(value) => value.to_owned(),
            None => args
                .next()
                .ok_or_else(|| usage(format!("{flag} needs a value")))?,
        };
        let given_twice = || usage(format!("{flag} is given more than once"));
        if flag == "--role" {
            let parsed =
                AgentRole::parse(&value).ok_or_else(|| usage(format!("unknown role {value:?}")))?;
            if role.replace(parsed).is_some() {
                return Err(given_twice());
            }
        } else {
            let parsed = value
                .parse::<CrawlerReviewId>()
                .map_err(|_| usage(format!("{value:?} is not a review id")))?;
            if review.replace(parsed).is_some() {
                return Err(given_twice());
            }
        }
    }
    let role = role.ok_or_else(|| usage("--role is required".to_owned()))?;
    Ok((
        role,
        review.map_or(ReviewScope::AnyReview, ReviewScope::Only),
    ))
}

/// The console's base URL. The bearer token travels with every request, so plain http is
/// accepted for a loopback host only, and a URL that carries credentials, a query or a fragment
/// is refused.
fn console_url(value: &str) -> Result<Url, ReviewMcpSettingsError> {
    let invalid = |reason| ReviewMcpSettingsError::InvalidConsoleUrl { reason };
    let url = Url::parse(value.trim()).map_err(|_| invalid("not a URL"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid("credentials are not accepted in the URL"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(invalid("a query or fragment is not accepted"));
    }
    let loopback = match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => return Err(invalid("the URL names no host")),
    };
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        "http" => return Err(invalid("plain http is accepted for a loopback host only")),
        _ => return Err(invalid("the scheme is neither http nor https")),
    }
    let mut url = url;
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(&path);
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn settings(
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Result<ReviewMcpSettings, ReviewMcpSettingsError> {
        let env: HashMap<String, String> = env
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        ReviewMcpSettings::from_args_and_env(
            args.iter().map(|arg| (*arg).to_owned()),
            |name| env.get(name).cloned(),
            || PathBuf::from("/var/lib/crawler"),
        )
    }

    #[test]
    fn should_serve_the_role_named_on_the_command_line_with_the_default_console_and_token_file() {
        for (args, role) in [
            (&["--role", "author"][..], AgentRole::Author),
            (&["--role=reviewer"][..], AgentRole::Reviewer),
        ] {
            let read = settings(args, &[]).unwrap_or_else(|error| panic!("{error}"));

            assert_eq!(read.role, role);
            assert_eq!(read.scope, ReviewScope::AnyReview);
            assert_eq!(read.console_url.as_str(), "http://127.0.0.1:7878/");
            assert_eq!(
                read.token_file,
                PathBuf::from("/var/lib/crawler/review-agent-tokens.json")
            );
        }
    }

    #[test]
    fn should_read_the_console_url_and_the_token_file_from_the_environment() {
        let read = settings(
            &["--role", "reviewer"],
            &[
                (REVIEW_CONSOLE_URL_ENV, " https://review.example/console/ "),
                (REVIEW_AGENT_TOKEN_FILE_ENV, "/run/crawler/tokens.json"),
            ],
        )
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(read.console_url.as_str(), "https://review.example/console");
        assert_eq!(read.token_file, PathBuf::from("/run/crawler/tokens.json"));
        let blank = settings(
            &["--role", "author"],
            &[
                (REVIEW_CONSOLE_URL_ENV, " "),
                (REVIEW_AGENT_TOKEN_FILE_ENV, ""),
            ],
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(blank.console_url.as_str(), "http://127.0.0.1:7878/");
        assert_eq!(
            blank.token_file,
            PathBuf::from("/var/lib/crawler/review-agent-tokens.json")
        );
    }

    #[test]
    fn should_scope_the_process_to_the_review_named_on_the_command_line() {
        let review = CrawlerReviewId::new();
        let other = CrawlerReviewId::new();
        for args in [
            vec![
                "--role".to_owned(),
                "reviewer".to_owned(),
                "--review".to_owned(),
                review.to_string(),
            ],
            vec![format!("--review={review}"), "--role=reviewer".to_owned()],
        ] {
            let read = ReviewMcpSettings::from_args_and_env(
                args,
                |_| None,
                || PathBuf::from("/var/lib/crawler"),
            )
            .unwrap_or_else(|error| panic!("{error}"));

            assert_eq!(read.scope, ReviewScope::Only(review));
            assert!(read.scope.admits(review));
            assert!(!read.scope.admits(other));
        }
        assert!(ReviewScope::AnyReview.admits(other));
    }

    #[test]
    fn should_refuse_any_argument_but_one_known_role_and_at_most_one_review() {
        let review = CrawlerReviewId::new().to_string();
        let page = crate::CrawlerReviewPageId::new().to_string();
        for args in [
            &[][..],
            &["--role"][..],
            &["--role", "admin"][..],
            &["--role", "author", "--role", "reviewer"][..],
            &["--role", "author", "--token", "secret"][..],
            &["author"][..],
            &["--review", &review][..],
            &["--role", "author", "--review"][..],
            &["--role", "author", "--review", &page][..],
            &["--role", "author", "--review", &review, "--review", &review][..],
            &["--role", "author", "--reviewer", &review][..],
        ] {
            assert!(
                matches!(settings(args, &[]), Err(ReviewMcpSettingsError::Usage(_))),
                "{args:?}"
            );
        }
    }

    #[test]
    fn should_refuse_a_console_url_that_would_send_the_token_in_the_clear_or_elsewhere() {
        for url in [
            "http://review.example:7878",
            "https://user:secret@review.example",
            "https://review.example/?token=1",
            "ftp://127.0.0.1",
            "127.0.0.1:7878",
        ] {
            let refused = settings(&["--role", "author"], &[(REVIEW_CONSOLE_URL_ENV, url)]);
            assert!(
                matches!(
                    refused,
                    Err(ReviewMcpSettingsError::InvalidConsoleUrl { .. })
                ),
                "{url}"
            );
            let shown = refused
                .err()
                .map(|error| error.to_string())
                .unwrap_or_default();
            assert!(
                !shown.contains("secret") && !shown.contains("token="),
                "{shown}"
            );
        }
        for url in [
            "http://localhost:7878",
            "http://[::1]:7878",
            "http://127.0.0.2:80",
        ] {
            assert!(
                settings(&["--role", "author"], &[(REVIEW_CONSOLE_URL_ENV, url)]).is_ok(),
                "{url}"
            );
        }
    }
}
