//! The termination signal of a host process.

use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
#[error("failed to register the termination signal handlers")]
pub struct ShutdownError(#[source] std::io::Error);

/// SIGTERM (container stop) or SIGINT (interactive stop). Handlers are registered when this is
/// created, before any slow startup, so an early signal is not lost.
pub struct Shutdown {
    terminate: Signal,
    interrupt: Signal,
}

impl Shutdown {
    pub fn register() -> Result<Self, ShutdownError> {
        Ok(Self {
            terminate: signal(SignalKind::terminate()).map_err(ShutdownError)?,
            interrupt: signal(SignalKind::interrupt()).map_err(ShutdownError)?,
        })
    }

    /// Resolves on the first signal.
    pub async fn received(mut self) {
        let signal = tokio::select! {
            _ = self.terminate.recv() => "SIGTERM",
            _ = self.interrupt.recv() => "SIGINT",
        };
        tracing::info!(event = "process.shutdown.requested", signal);
    }

    /// A token cancelled on the first signal, for tasks that stop on their own schedule.
    pub fn into_token(self) -> CancellationToken {
        let token = CancellationToken::new();
        let cancel = token.clone();
        tokio::spawn(async move {
            self.received().await;
            cancel.cancel();
        });
        token
    }
}
