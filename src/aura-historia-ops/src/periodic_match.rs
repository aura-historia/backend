//! Periodic saved-search matching. SIGTERM or SIGINT cancels the run; the termination handlers
//! are registered before the matching wiring connects to its providers.

use crate::OpsError;
use platform_process::Shutdown;
use serde_json::{Value, json};

const COMMAND: &str = "periodic-match";

pub(crate) async fn run() -> Result<Value, OpsError> {
    let shutdown = Shutdown::register().map_err(|error| OpsError::failed(COMMAND, error))?;
    search_filter_periodic_match::run(shutdown.received())
        .await
        .map_err(|error| OpsError::failed(COMMAND, error))?;
    Ok(json!({ "outcome": "success" }))
}
