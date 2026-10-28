//! The review agent's bearer token, read from the token file the crawler writes at every start in
//! `agent` authoring mode ([`crate::review::agent_tokens`]). `review-mcp` reads its role's token at
//! startup and again when the console answers `401`, because a crawler restart rotates the tokens.
//! A token is never logged, printed or returned by a tool.

use super::settings::AgentRole;
use serde::Deserialize;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// The longest token file `review-mcp` reads; the crawler writes about 150 bytes.
const MAX_TOKEN_FILE_BYTES: u64 = 4 * 1024;

/// One review agent's bearer token.
#[derive(Clone, PartialEq, Eq)]
pub struct BearerToken(String);

impl BearerToken {
    /// The value of the `Authorization` header that presents this token.
    pub(super) fn authorization(&self) -> String {
        format!("Bearer {}", self.0)
    }
}

impl fmt::Debug for BearerToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BearerToken(..)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TokenFileError {
    #[error(
        "cannot read the review agents' token file {}; the crawler writes it at every start with CRAWLER_AUTHORING_MODE=agent",
        path.display()
    )]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the review agents' token file {} is larger than a token file", path.display())]
    TooLarge { path: PathBuf },
    #[error("the review agents' token file {} is not the crawler's token file", path.display())]
    Malformed {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("the review agents' token file {} holds no valid {role} token", path.display())]
    InvalidToken { path: PathBuf, role: AgentRole },
}

/// The token file's content: each role's token by the role's name.
#[derive(Deserialize)]
struct TokenFile {
    author: String,
    reviewer: String,
}

/// Where one role's token is read from.
#[derive(Debug, Clone)]
pub struct RoleTokenFile {
    path: PathBuf,
    role: AgentRole,
}

impl RoleTokenFile {
    pub fn new(path: PathBuf, role: AgentRole) -> Self {
        Self { path, role }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the role's current token. A token is a non-empty run of ASCII letters and digits,
    /// as the crawler generates it, so it can never break the request header it is sent in.
    pub fn read(&self) -> Result<BearerToken, TokenFileError> {
        let path = || self.path.clone();
        let file = std::fs::File::open(&self.path).map_err(|source| TokenFileError::Read {
            path: path(),
            source,
        })?;
        let mut contents = Vec::new();
        io::Read::read_to_end(
            &mut io::Read::take(file, MAX_TOKEN_FILE_BYTES + 1),
            &mut contents,
        )
        .map_err(|source| TokenFileError::Read {
            path: path(),
            source,
        })?;
        if contents.len() as u64 > MAX_TOKEN_FILE_BYTES {
            return Err(TokenFileError::TooLarge { path: path() });
        }
        let tokens: TokenFile =
            serde_json::from_slice(&contents).map_err(|source| TokenFileError::Malformed {
                path: path(),
                source,
            })?;
        let token = match self.role {
            AgentRole::Author => tokens.author,
            AgentRole::Reviewer => tokens.reviewer,
        };
        if token.is_empty() || !token.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            return Err(TokenFileError::InvalidToken {
                path: path(),
                role: self.role,
            });
        }
        Ok(BearerToken(token))
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::review::agent_tokens::REVIEW_AGENT_TOKEN_FILE;

    /// A directory of its own for one test holding a token file, removed when the test ends.
    pub(in crate::review_mcp) struct TokenDirectory(PathBuf);

    impl TokenDirectory {
        pub(in crate::review_mcp) fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "crawler-review-mcp-tokens-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir(&path).unwrap_or_else(|error| panic!("token directory: {error}"));
            Self(path)
        }

        pub(in crate::review_mcp) fn path(&self) -> PathBuf {
            self.0.join(REVIEW_AGENT_TOKEN_FILE)
        }

        pub(in crate::review_mcp) fn write(&self, contents: &str) {
            std::fs::write(self.path(), contents)
                .unwrap_or_else(|error| panic!("write token file: {error}"));
        }

        pub(in crate::review_mcp) fn write_tokens(&self, author: &str, reviewer: &str) {
            self.write(&serde_json::json!({ "author": author, "reviewer": reviewer }).to_string());
        }
    }

    impl Drop for TokenDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn should_read_only_the_token_of_its_role() {
        let directory = TokenDirectory::new();
        directory.write_tokens("authortoken1", "reviewertoken2");

        let author = RoleTokenFile::new(directory.path(), AgentRole::Author).read();
        let reviewer = RoleTokenFile::new(directory.path(), AgentRole::Reviewer).read();

        assert_eq!(
            author.map(|token| token.authorization()).ok(),
            Some("Bearer authortoken1".to_owned())
        );
        assert_eq!(
            reviewer.map(|token| token.authorization()).ok(),
            Some("Bearer reviewertoken2".to_owned())
        );
    }

    #[test]
    fn should_read_the_rotated_token_on_the_next_read() {
        let directory = TokenDirectory::new();
        let file = RoleTokenFile::new(directory.path(), AgentRole::Author);
        directory.write_tokens("before", "reviewer");
        let before = file.read().unwrap_or_else(|error| panic!("{error}"));

        directory.write_tokens("after", "reviewer");
        let after = file.read().unwrap_or_else(|error| panic!("{error}"));

        assert_ne!(before, after);
        assert_eq!(after.authorization(), "Bearer after");
    }

    #[test]
    fn should_refuse_a_missing_malformed_or_oversized_file_and_an_invalid_token() {
        let directory = TokenDirectory::new();
        let file = RoleTokenFile::new(directory.path(), AgentRole::Reviewer);
        assert!(matches!(file.read(), Err(TokenFileError::Read { .. })));

        directory.write("{\"author\": \"a\"}");
        assert!(matches!(file.read(), Err(TokenFileError::Malformed { .. })));

        directory.write(&format!(
            "{{\"author\": \"a\", \"reviewer\": \"{}\"}}",
            "b".repeat(5000)
        ));
        assert!(matches!(file.read(), Err(TokenFileError::TooLarge { .. })));

        for token in ["", "with space", "line\r\nX-Injected: 1"] {
            directory.write_tokens("author", token);
            assert!(
                matches!(file.read(), Err(TokenFileError::InvalidToken { .. })),
                "{token:?}"
            );
        }
    }

    #[test]
    fn should_never_show_the_token_in_debug_output_or_errors() {
        let directory = TokenDirectory::new();
        directory.write_tokens("secretauthor", "secret reviewer");

        let token = RoleTokenFile::new(directory.path(), AgentRole::Author)
            .read()
            .unwrap_or_else(|error| panic!("{error}"));
        let refusal = RoleTokenFile::new(directory.path(), AgentRole::Reviewer)
            .read()
            .err()
            .unwrap_or_else(|| panic!("a token with a space was read"));

        assert_eq!(format!("{token:?}"), "BearerToken(..)");
        let shown = format!("{refusal} {refusal:?}");
        assert!(!shown.contains("secret"), "{shown}");
    }
}
