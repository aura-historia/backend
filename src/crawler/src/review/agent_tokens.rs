//! The bearer tokens of the review agents in `agent` authoring mode: one for the author agent and
//! one for the reviewer agent, generated from the operating system's CSPRNG at every start and
//! handed to the review-console MCP server through a token file only the crawler's user can read.
//! Nobody configures them, a restart rotates them, and they are never logged. The operator contract
//! is in `docs/crawler/operations.md` (review agents).

use crate::authoring::CrawlerAuthoringMode;
use crate::review::access::{ReviewServerConfig, ReviewServerConfigError};
use serde::Serialize;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tracing::info;

/// The token file's name in the crawler's state directory, its working directory.
pub const REVIEW_AGENT_TOKEN_FILE: &str = "review-agent-tokens.json";

/// Random bytes behind each token: 256 bits, hex-encoded.
const TOKEN_BYTES: usize = 32;

/// The author agent's and the reviewer agent's bearer tokens of this start.
#[derive(Clone, PartialEq, Eq)]
pub struct ReviewAgentTokens {
    author: String,
    reviewer: String,
}

impl fmt::Debug for ReviewAgentTokens {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReviewAgentTokens { .. }")
    }
}

impl ReviewAgentTokens {
    /// Fresh tokens for both agents from the operating system's CSPRNG.
    pub fn generate() -> Result<Self, getrandom::Error> {
        Ok(Self {
            author: random_token()?,
            reviewer: random_token()?,
        })
    }

    /// Tokens of the given values, for tests that send them as a client would.
    #[cfg(test)]
    pub(crate) fn of(author: &str, reviewer: &str) -> Self {
        Self {
            author: author.to_owned(),
            reviewer: reviewer.to_owned(),
        }
    }

    pub(crate) fn author(&self) -> &str {
        &self.author
    }

    pub(crate) fn reviewer(&self) -> &str {
        &self.reviewer
    }
}

fn random_token() -> Result<String, getrandom::Error> {
    let mut bytes = [0_u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The token file's content: each role's token by the role's name.
#[derive(Serialize)]
struct TokenFile<'a> {
    author: &'a str,
    reviewer: &'a str,
}

#[derive(Debug, thiserror::Error)]
pub enum ReviewAgentProvisioningError {
    #[error("could not generate the review agents' tokens")]
    Random(#[source] getrandom::Error),
    #[error(transparent)]
    Config(#[from] ReviewServerConfigError),
    #[error("could not write the review agents' token file {}", path.display())]
    WriteTokenFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(
        "could not remove the review agents' token file {} that an earlier start in agent authoring mode left",
        path.display()
    )]
    RemoveTokenFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Prepares the review console's agent access for `authoring`. In `agent` mode it generates fresh
/// author and reviewer tokens, opens the console to them and writes them to the token file in
/// `state_directory`, replacing the tokens of an earlier start. In every other mode no agent acts:
/// it writes nothing and removes a token file an earlier start left.
pub fn provision_review_agents(
    config: ReviewServerConfig,
    authoring: CrawlerAuthoringMode,
    state_directory: &Path,
) -> Result<ReviewServerConfig, ReviewAgentProvisioningError> {
    let path = state_directory.join(REVIEW_AGENT_TOKEN_FILE);
    if !authoring.has_review_agents() {
        if remove_token_file(&path).map_err(|source| {
            ReviewAgentProvisioningError::RemoveTokenFile {
                path: path.clone(),
                source,
            }
        })? {
            info!(
                path = %path.display(),
                authoring_mode = authoring.as_str(),
                "Removed the review agents' token file: only agent authoring mode has review agents"
            );
        }
        return Ok(config);
    }
    let tokens = ReviewAgentTokens::generate().map_err(ReviewAgentProvisioningError::Random)?;
    let config = config.with_review_agents(tokens.clone())?;
    write_token_file(&path, &tokens).map_err(|source| {
        ReviewAgentProvisioningError::WriteTokenFile {
            path: path.clone(),
            source,
        }
    })?;
    info!(
        path = %path.display(),
        "Generated the review agents' tokens of this start and wrote them to their token file"
    );
    Ok(config)
}

/// Writes `tokens` to `path` as a file only its owner can read and write. The file is written
/// under a new temporary name in the same directory and renamed over `path`, so a reader sees the
/// old or the new tokens, never a partial file, and never a file another user could read.
fn write_token_file(path: &Path, tokens: &ReviewAgentTokens) -> io::Result<()> {
    let contents = serde_json::to_vec(&TokenFile {
        author: &tokens.author,
        reviewer: &tokens.reviewer,
    })?;
    let temporary = path.with_file_name(format!(
        ".{REVIEW_AGENT_TOKEN_FILE}.{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    let written = new_owner_only_file(&temporary)
        .and_then(|mut file| {
            file.write_all(&contents)?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&temporary, path));
    if written.is_err() {
        // The temporary file may not exist; the write's own error is the one to report.
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// Creates a file that must not exist yet, readable and writable by its owner only (mode 0600 on
/// Unix; elsewhere it keeps the access its directory grants).
fn new_owner_only_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Removes the token file, returning whether one was there.
fn remove_token_file(path: &Path) -> io::Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::agent_decisions::{AgentApprovals, AgentDecisionPolicy};
    use crate::review::live_jobs::DEFAULT_MAX_CONCURRENT_LIVE_JOBS;

    fn console_config(auth_token: Option<&str>) -> ReviewServerConfig {
        ReviewServerConfig {
            bind_addr: "127.0.0.1:7878".parse().unwrap(),
            auth_token: auth_token.map(str::to_owned),
            agent_tokens: None,
            agent_decisions: AgentDecisionPolicy::default(),
            local_mutations_without_token: false,
            max_concurrent_live_jobs: DEFAULT_MAX_CONCURRENT_LIVE_JOBS,
        }
    }

    /// A directory of its own for one test, removed when the test ends.
    struct StateDirectory(PathBuf);

    impl StateDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "crawler-review-agent-tokens-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir(&path).unwrap_or_else(|error| panic!("state directory: {error}"));
            Self(path)
        }

        fn token_file(&self) -> PathBuf {
            self.0.join(REVIEW_AGENT_TOKEN_FILE)
        }

        fn entries(&self) -> Vec<String> {
            let mut entries: Vec<String> = std::fs::read_dir(&self.0)
                .unwrap_or_else(|error| panic!("read state directory: {error}"))
                .map(|entry| {
                    entry
                        .unwrap_or_else(|error| panic!("state directory entry: {error}"))
                        .file_name()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            entries.sort();
            entries
        }
    }

    impl Drop for StateDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn written_tokens(path: &Path) -> serde_json::Value {
        let contents =
            std::fs::read(path).unwrap_or_else(|error| panic!("read token file: {error}"));
        serde_json::from_slice(&contents).unwrap_or_else(|error| panic!("token file: {error}"))
    }

    fn is_token(value: &serde_json::Value) -> bool {
        value.as_str().is_some_and(|token| {
            token.len() == 2 * TOKEN_BYTES && token.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    }

    #[test]
    fn should_generate_distinct_random_tokens_and_never_show_them_in_debug_output() {
        let first = ReviewAgentTokens::generate().unwrap_or_else(|error| panic!("{error}"));
        let second = ReviewAgentTokens::generate().unwrap_or_else(|error| panic!("{error}"));

        for tokens in [&first, &second] {
            assert!(is_token(&serde_json::Value::from(tokens.author())));
            assert!(is_token(&serde_json::Value::from(tokens.reviewer())));
            assert_ne!(tokens.author(), tokens.reviewer());
            let debug = format!("{tokens:?}");
            assert!(!debug.contains(tokens.author()), "{debug}");
            assert!(!debug.contains(tokens.reviewer()), "{debug}");
        }
        assert_ne!(first, second);
    }

    /// In `agent` mode each start writes fresh tokens, owner-only, to the token file and opens the
    /// console to them, with the reviewer's decisions guarded; no temporary file is left behind.
    #[test]
    fn should_write_fresh_tokens_of_both_roles_to_an_owner_only_file_at_every_agent_mode_start() {
        let state = StateDirectory::new();

        let first = provision_review_agents(
            console_config(Some("review-token")),
            CrawlerAuthoringMode::Agent,
            &state.0,
        )
        .unwrap_or_else(|error| panic!("first start: {error}"));
        let first_file = written_tokens(&state.token_file());
        let second = provision_review_agents(
            console_config(Some("review-token")),
            CrawlerAuthoringMode::Agent,
            &state.0,
        )
        .unwrap_or_else(|error| panic!("second start: {error}"));
        let second_file = written_tokens(&state.token_file());

        let tokens = second
            .agent_tokens
            .as_ref()
            .unwrap_or_else(|| panic!("agent mode opens the console to the agents"));
        assert_eq!(
            second_file,
            serde_json::json!({ "author": tokens.author(), "reviewer": tokens.reviewer() })
        );
        assert!(is_token(&second_file["author"]) && is_token(&second_file["reviewer"]));
        assert_ne!(first_file, second_file, "a restart rotates the tokens");
        assert_ne!(first.agent_tokens, second.agent_tokens);
        assert_eq!(second.agent_decisions.approvals, AgentApprovals::Guarded);
        assert_eq!(state.entries(), vec![REVIEW_AGENT_TOKEN_FILE.to_owned()]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(state.token_file())
                .unwrap_or_else(|error| panic!("token file metadata: {error}"))
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    /// Outside `agent` mode no agent acts: nothing is written, a token file an earlier start left
    /// is removed, and the agents' decisions stay off.
    #[rstest::rstest]
    #[case::manual(CrawlerAuthoringMode::Manual)]
    #[case::review(CrawlerAuthoringMode::Review)]
    #[case::auto(CrawlerAuthoringMode::Auto)]
    fn should_remove_the_token_file_and_open_the_console_to_no_agent_outside_agent_mode(
        #[case] authoring: CrawlerAuthoringMode,
    ) {
        let state = StateDirectory::new();
        provision_review_agents(
            console_config(Some("review-token")),
            CrawlerAuthoringMode::Agent,
            &state.0,
        )
        .unwrap_or_else(|error| panic!("agent mode start: {error}"));

        let config =
            provision_review_agents(console_config(Some("review-token")), authoring, &state.0)
                .unwrap_or_else(|error| panic!("{authoring:?} start: {error}"));
        let again = provision_review_agents(console_config(None), authoring, &state.0)
            .unwrap_or_else(|error| panic!("{authoring:?} start without a file: {error}"));

        assert!(state.entries().is_empty(), "{:?}", state.entries());
        for config in [config, again] {
            assert_eq!(config.agent_tokens, None);
            assert_eq!(config.agent_decisions, AgentDecisionPolicy::default());
        }
    }

    /// `agent` mode refuses to start without a human credential for mutations, and then writes no
    /// token file.
    #[test]
    fn should_refuse_agent_mode_without_a_human_credential_and_write_nothing() {
        let state = StateDirectory::new();

        let refused =
            provision_review_agents(console_config(None), CrawlerAuthoringMode::Agent, &state.0);

        match refused {
            Err(ReviewAgentProvisioningError::Config(
                ReviewServerConfigError::ReviewAgentsWithoutHumanCredential,
            )) => {}
            Err(other) => panic!("unexpected refusal: {other}"),
            Ok(_) => panic!("agent mode without a human credential must fail startup"),
        }
        assert!(state.entries().is_empty(), "{:?}", state.entries());
    }

    #[test]
    fn should_name_the_token_file_when_it_cannot_be_written() {
        let state = StateDirectory::new();
        let missing = state.0.join("missing");

        let refused = provision_review_agents(
            console_config(Some("review-token")),
            CrawlerAuthoringMode::Agent,
            &missing,
        )
        .err()
        .unwrap_or_else(|| panic!("a missing state directory must fail startup"));

        assert_eq!(
            refused.to_string(),
            format!(
                "could not write the review agents' token file {}",
                missing.join(REVIEW_AGENT_TOKEN_FILE).display()
            )
        );
    }
}
