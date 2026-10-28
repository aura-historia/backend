//! `review-mcp`: the review console's tools for one review agent of `agent` authoring mode, as an
//! MCP server over stdio. It is a thin client of the console API: every tool shapes one or a few
//! console requests with the role's bearer token and maps the answer, and the console decides
//! everything, so the server has no database access and no review policy of its own. A role's
//! server lists and runs only that role's tools, refuses any URL off the review's ListingSource
//! before a request is sent, and answers within a size bound. No answer carries a token, cookie or
//! header. The operator contract is in `docs/crawler/operations.md` (review MCP server).

mod bounds;
mod console;
mod errors;
mod server;
mod settings;
mod token_file;
mod tools;
mod views;
mod wire;

pub use console::{ConsoleClient, ConsoleClientError};
pub use server::ReviewMcpServer;
pub use settings::{
    AgentRole, DEFAULT_REVIEW_CONSOLE_URL, REVIEW_AGENT_TOKEN_FILE_ENV, REVIEW_CONSOLE_URL_ENV,
    ReviewMcpSettings, ReviewMcpSettingsError,
};
pub use token_file::{RoleTokenFile, TokenFileError};
pub use tools::{Polling, ToolContext, ToolSchemaError, tools_for};

use rmcp::ServiceExt;
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::info;

#[derive(Debug, thiserror::Error)]
pub enum ReviewMcpError {
    #[error(transparent)]
    Client(#[from] ConsoleClientError),
    #[error(transparent)]
    Tools(#[from] ToolSchemaError),
    #[error("the MCP session could not start")]
    Initialize(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("the MCP session ended abnormally")]
    Session(#[source] tokio::task::JoinError),
}

/// Serves the role's tools over the MCP session on `reader` and `writer`, such as the process's
/// stdin and stdout, until the client closes it. The role's token is read before the session
/// starts, so a process without a readable token file fails at once.
pub async fn serve<R, W>(
    settings: ReviewMcpSettings,
    polling: Polling,
    reader: R,
    writer: W,
) -> Result<(), ReviewMcpError>
where
    R: AsyncRead + Send + Unpin + 'static,
    W: AsyncWrite + Send + Unpin + 'static,
{
    let ReviewMcpSettings {
        role,
        console_url,
        token_file,
    } = settings;
    let console = ConsoleClient::new(&console_url, RoleTokenFile::new(token_file.clone(), role))?;
    let tools = tools_for(role)?;
    info!(
        role = role.as_str(),
        console_url = %console_url,
        token_file = %token_file.display(),
        tools = tools.len(),
        "Review MCP server serving its role's tools"
    );
    let server = ReviewMcpServer::new(role, tools, ToolContext::new(console, polling));
    let session = server
        .serve((reader, writer))
        .await
        .map_err(|error| ReviewMcpError::Initialize(Box::new(error)))?;
    let reason = session.waiting().await.map_err(ReviewMcpError::Session)?;
    info!(reason = ?reason, "Review MCP session ended");
    Ok(())
}
