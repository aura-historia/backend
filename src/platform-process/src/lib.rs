//! Startup and shutdown of the long-running processes on the host: configuration from the
//! environment and from files, the PostgreSQL pool, the termination signal, bounded panic and
//! failure diagnostics, and Google credentials from a file. The [README](../README.md) owns the
//! configuration contract.

pub mod config;
mod failure_diagnostic;
mod google;
mod panic;
pub mod postgres;
mod rate_limit;
mod shutdown;

pub use failure_diagnostic::{
    Dependency, FailureDiagnostic, INVALID_PERSISTED_STATE, UNCLASSIFIED_FAILURE,
};
pub use google::{GoogleCredentialsError, google_application_default_credentials};
pub use panic::install_bounded_panic_hook;
pub use rate_limit::{InvalidRateLimit, RateLimit, TokenBucket};
pub use shutdown::{Shutdown, ShutdownError};
