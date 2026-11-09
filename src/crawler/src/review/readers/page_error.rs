//! How a paginated console read fails.

/// Why a page of the console's lists could not be read.
#[derive(Debug, thiserror::Error)]
pub enum PageReadError {
    /// The row `after` names does not exist, so the page cannot start after it. A reader starts
    /// again from the first page instead of taking an empty page for the end of the list.
    #[error("after names no such row: {0}")]
    UnknownCursor(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}
