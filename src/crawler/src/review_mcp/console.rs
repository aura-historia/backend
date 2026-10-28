//! The review console as `review-mcp` calls it: JSON requests with the role's bearer token, and
//! the console's answers, accepted (`2xx`) or refused (`4xx`, with the console's own `error`,
//! `reason`, `failed_checks` and the other keys it answers), each read up to a bound. The client
//! follows no redirect and uses no proxy, so the token only ever reaches the configured console.

use super::token_file::{BearerToken, RoleTokenFile, TokenFileError};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{info, warn};
use url::Url;

/// The longest console answer `review-mcp` reads; the review queue with its candidates is the
/// largest one.
const MAX_CONSOLE_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The console answers within its own 30-second request deadline.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

/// An accepted console answer: its status and JSON body.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsoleAnswer {
    pub status: u16,
    pub body: Value,
}

impl ConsoleAnswer {
    /// The body as `T`, or [`ConsoleError::UnexpectedAnswer`] when it is not one.
    pub fn decode<T: DeserializeOwned>(self) -> Result<T, ConsoleError> {
        serde_json::from_value(self.body).map_err(ConsoleError::UnexpectedAnswer)
    }
}

/// What the console said when it refused a request, as its JSON body carries it.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsoleRefusal {
    pub status: u16,
    pub body: Value,
}

#[derive(Debug, thiserror::Error)]
pub enum ConsoleError {
    #[error("the review console refused the request with status {}", .0.status)]
    Refused(ConsoleRefusal),
    #[error(
        "the review console refused this agent's token, also after the token file was read again"
    )]
    Unauthorized,
    #[error("the agent's token cannot be read")]
    TokenFile(#[source] TokenFileError),
    #[error("the review console cannot be reached")]
    Unreachable(#[source] reqwest::Error),
    #[error("the review console's answer is larger than {MAX_CONSOLE_RESPONSE_BYTES} bytes")]
    AnswerTooLarge,
    #[error("the review console's answer is not the expected JSON")]
    UnexpectedAnswer(#[source] serde_json::Error),
}

/// A client of one review console, acting with one role's token.
pub struct ConsoleClient {
    base_url: String,
    http: reqwest::Client,
    token_file: RoleTokenFile,
    token: RwLock<BearerToken>,
}

impl ConsoleClient {
    /// A client of the console at `console_url` acting with the role's token, which it reads from
    /// `token_file` now: a process without a readable token fails at startup.
    pub fn new(console_url: &Url, token_file: RoleTokenFile) -> Result<Self, ConsoleClientError> {
        let token = token_file.read().map_err(ConsoleClientError::TokenFile)?;
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(ConsoleClientError::Http)?;
        Ok(Self {
            base_url: console_url.as_str().trim_end_matches('/').to_owned(),
            http,
            token_file,
            token: RwLock::new(token),
        })
    }

    pub async fn get(&self, path: &str) -> Result<ConsoleAnswer, ConsoleError> {
        self.request(Method::GET, path, None).await
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<ConsoleAnswer, ConsoleError> {
        self.request(Method::POST, path, Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> Result<ConsoleAnswer, ConsoleError> {
        self.request(Method::DELETE, path, None).await
    }

    /// Sends a request with the current token. The console refuses an unknown token before it
    /// reads the request, so on a `401` the token file is read again and, when the crawler has
    /// rotated the token since, the request is sent once more with the new one.
    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<ConsoleAnswer, ConsoleError> {
        let token = self.token.read().await.clone();
        match self.send(&method, path, body, &token).await? {
            Sent::Answered(answer) => Ok(answer),
            Sent::Unauthorized => {
                let rotated = self.token_file.read().map_err(ConsoleError::TokenFile)?;
                if rotated == token {
                    warn!(path, "The review console refused the agent's current token");
                    return Err(ConsoleError::Unauthorized);
                }
                info!(
                    token_file = %self.token_file.path().display(),
                    "Read the agent's rotated token from the token file"
                );
                *self.token.write().await = rotated.clone();
                match self.send(&method, path, body, &rotated).await? {
                    Sent::Answered(answer) => Ok(answer),
                    Sent::Unauthorized => Err(ConsoleError::Unauthorized),
                }
            }
        }
    }

    async fn send(
        &self,
        method: &Method,
        path: &str,
        body: Option<&Value>,
        token: &BearerToken,
    ) -> Result<Sent, ConsoleError> {
        let mut request = self
            .http
            .request(method.clone(), format!("{}{path}", self.base_url))
            .header(reqwest::header::AUTHORIZATION, token.authorization());
        if let Some(body) = body {
            request = request.json(body);
        }
        let mut response = request.send().await.map_err(ConsoleError::Unreachable)?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED {
            return Ok(Sent::Unauthorized);
        }
        let bytes = read_bounded(&mut response).await?;
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).map_err(ConsoleError::UnexpectedAnswer)?
        };
        if status.is_success() {
            Ok(Sent::Answered(ConsoleAnswer {
                status: status.as_u16(),
                body,
            }))
        } else {
            Err(ConsoleError::Refused(ConsoleRefusal {
                status: status.as_u16(),
                body,
            }))
        }
    }
}

enum Sent {
    Answered(ConsoleAnswer),
    Unauthorized,
}

async fn read_bounded(response: &mut reqwest::Response) -> Result<Vec<u8>, ConsoleError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(ConsoleError::Unreachable)? {
        if bytes.len() + chunk.len() > MAX_CONSOLE_RESPONSE_BYTES {
            return Err(ConsoleError::AnswerTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[derive(Debug, thiserror::Error)]
pub enum ConsoleClientError {
    #[error(transparent)]
    TokenFile(TokenFileError),
    #[error("cannot build the review console client")]
    Http(#[source] reqwest::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_mcp::settings::AgentRole;
    use crate::review_mcp::token_file::tests::TokenDirectory;
    use serde_json::json;
    use std::sync::{Arc, Mutex, PoisonError};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A console that accepts one token at a time and answers each request with `status` and
    /// `body`, recording the bearer of every request.
    struct FakeConsole {
        url: Url,
        accepted: Arc<Mutex<String>>,
        bearers: Arc<Mutex<Vec<String>>>,
    }

    impl FakeConsole {
        async fn start(accepted: &str, status: u16, body: Value) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .unwrap_or_else(|error| panic!("bind: {error}"));
            let url = Url::parse(&format!(
                "http://{}",
                listener
                    .local_addr()
                    .unwrap_or_else(|error| panic!("{error}"))
            ))
            .unwrap_or_else(|error| panic!("{error}"));
            let accepted = Arc::new(Mutex::new(accepted.to_owned()));
            let bearers = Arc::new(Mutex::new(Vec::new()));
            let (token, seen) = (Arc::clone(&accepted), Arc::clone(&bearers));
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    let mut request = Vec::new();
                    let mut buffer = [0_u8; 4096];
                    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                        match stream.read(&mut buffer).await {
                            Ok(0) | Err(_) => break,
                            Ok(read) => request.extend_from_slice(&buffer[..read]),
                        }
                    }
                    let request = String::from_utf8_lossy(&request).to_string();
                    let bearer = request
                        .lines()
                        .find_map(|line| line.strip_prefix("authorization: Bearer "))
                        .unwrap_or_default()
                        .to_owned();
                    seen.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(bearer.clone());
                    let current = token.lock().unwrap_or_else(PoisonError::into_inner).clone();
                    let (status, body) = if bearer == current {
                        (status, body.to_string())
                    } else {
                        (401, json!({ "error": "unauthorized" }).to_string())
                    };
                    let response = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                }
            });
            Self {
                url,
                accepted,
                bearers,
            }
        }

        fn rotate(&self, token: &str) {
            *self.accepted.lock().unwrap_or_else(PoisonError::into_inner) = token.to_owned();
        }

        fn bearers(&self) -> Vec<String> {
            self.bearers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    fn client(console: &FakeConsole, directory: &TokenDirectory) -> ConsoleClient {
        ConsoleClient::new(
            &console.url,
            RoleTokenFile::new(directory.path(), AgentRole::Author),
        )
        .unwrap_or_else(|error| panic!("{error}"))
    }

    #[tokio::test]
    async fn should_read_the_rotated_token_once_the_console_refuses_the_old_one() {
        let directory = TokenDirectory::new();
        directory.write_tokens("first", "reviewer");
        let console = FakeConsole::start("first", 200, json!({ "ok": true })).await;
        let client = client(&console, &directory);
        assert!(client.get("/api/reviews").await.is_ok());

        console.rotate("second");
        directory.write_tokens("second", "reviewer");
        let answer = client.get("/api/reviews").await;
        let after = client.get("/api/reviews").await;

        assert_eq!(
            answer.ok().map(|answer| answer.body),
            Some(json!({ "ok": true }))
        );
        assert!(after.is_ok());
        assert_eq!(console.bearers(), ["first", "first", "second", "second"]);
    }

    #[tokio::test]
    async fn should_refuse_without_retrying_when_the_token_file_holds_the_refused_token() {
        let directory = TokenDirectory::new();
        directory.write_tokens("stale", "reviewer");
        let console = FakeConsole::start("rotated", 200, json!({ "ok": true })).await;
        let client = client(&console, &directory);

        let refused = client.get("/api/reviews").await;
        std::fs::remove_file(directory.path()).unwrap_or_else(|error| panic!("{error}"));
        let without_file = client.get("/api/reviews").await;

        assert!(matches!(refused, Err(ConsoleError::Unauthorized)));
        assert!(matches!(without_file, Err(ConsoleError::TokenFile(_))));
        assert_eq!(console.bearers(), ["stale", "stale"]);
    }

    #[tokio::test]
    async fn should_answer_a_refusal_with_the_console_body() {
        let directory = TokenDirectory::new();
        directory.write_tokens("token", "reviewer");
        let body = json!({ "error": "guard", "failed_checks": [{ "check": "TOO_FEW_LABELS" }] });
        let console = FakeConsole::start("token", 409, body.clone()).await;

        let refused = client(&console, &directory)
            .post(
                "/api/reviews/x/approve",
                &json!({ "expected_candidate_version": 1 }),
            )
            .await;

        match refused {
            Err(ConsoleError::Refused(refusal)) => {
                assert_eq!((refusal.status, refusal.body), (409, body));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn should_fail_at_startup_without_a_readable_token() {
        let directory = TokenDirectory::new();
        let console_url =
            Url::parse("http://127.0.0.1:7878").unwrap_or_else(|error| panic!("{error}"));

        let started = ConsoleClient::new(
            &console_url,
            RoleTokenFile::new(directory.path(), AgentRole::Reviewer),
        );

        assert!(matches!(started, Err(ConsoleClientError::TokenFile(_))));
    }
}
