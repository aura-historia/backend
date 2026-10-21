//! Job-owned test infrastructure. The JSON contract is shared by every runner.
mod contract;
pub use contract::{ManagedEnvironment, managed_environment};

#[cfg(feature = "supervisor")]
mod cli;
#[cfg(feature = "supervisor")]
mod docker;
#[cfg(feature = "supervisor")]
mod process;
#[cfg(feature = "supervisor")]
mod services;
#[cfg(feature = "supervisor")]
mod supervisor;

#[cfg(feature = "supervisor")]
pub use cli::run_cli;

use application::error::BoxError;
use std::fmt;

type Result<T> = std::result::Result<T, BoxError>;

#[derive(Debug)]
struct OperationFailed {
    operation: &'static str,
    source: BoxError,
}

impl fmt::Display for OperationFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.operation)
    }
}

impl std::error::Error for OperationFailed {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

fn invalid(message: &'static str) -> BoxError {
    std::io::Error::other(message).into()
}

fn context(operation: &'static str, source: impl Into<BoxError>) -> BoxError {
    Box::new(OperationFailed {
        operation,
        source: source.into(),
    })
}
