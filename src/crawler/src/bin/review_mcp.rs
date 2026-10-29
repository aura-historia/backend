//! `review-mcp`: the review console's tools for one review agent of `agent` authoring mode, served
//! as an MCP server over stdio. An MCP client such as the Claude Code CLI starts one process per
//! agent role, scoped with `--review` to the one review an agent process works:
//!
//! ```text
//! review-mcp --role author|reviewer [--review <review id>]
//! ```
//!
//! | Variable | Purpose | Default |
//! |---|---|---|
//! | `CRAWLER_REVIEW_CONSOLE_URL` | The review console's base URL; plain http only on a loopback host | `http://127.0.0.1:7878` |
//! | `CRAWLER_REVIEW_AGENT_TOKEN_FILE` | The token file the crawler writes in `agent` mode | `review-agent-tokens.json` in the working directory, the crawler's state directory |
//! | `LOG_LEVEL` | Log level of the JSON log on stderr | `info` |
//!
//! The role's token is read from the token file at startup and again when the console answers
//! `401`; it is never a setting or an argument. stdout carries the MCP session only. The contract
//! is in `docs/crawler/operations.md#review-mcp-server`.

use crawler::review_mcp::{Polling, ReviewMcpSettings, serve};
use std::process::ExitCode;
use tracing::error;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_owned());
    tracing_subscriber::fmt()
        .json()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::new(level))
        .init();

    let settings = match ReviewMcpSettings::from_args_and_env(
        std::env::args().skip(1),
        |name| std::env::var(name).ok(),
        crawler::runtime::state_directory,
    ) {
        Ok(settings) => settings,
        Err(settings_error) => {
            error!(error = %settings_error, "Invalid review MCP server settings");
            return ExitCode::from(2);
        }
    };
    let (stdin, stdout) = rmcp::transport::stdio();
    match serve(settings, Polling::default(), stdin, stdout).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(serve_error) => {
            error!(error = %serve_error, source = ?std::error::Error::source(&serve_error), "Review MCP server stopped");
            ExitCode::FAILURE
        }
    }
}
