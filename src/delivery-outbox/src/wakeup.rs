//! Wake-ups for an idle dispatcher: `LISTEN/NOTIFY` on capture, with polling as the fallback.

use crate::error::OutboxError;
use sqlx::{PgPool, postgres::PgListener};
use std::time::Duration;

/// The channel the outbox notifies when a capturing transaction commits.
pub const OUTBOX_CHANNEL: &str = "delivery_outbox";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutboxWakeup {
    /// A transaction that captured snapshots committed.
    Captured,
    /// The poll interval elapsed, or the listener reconnected and may have missed a
    /// notification. Dispatch anyway: redriven snapshots arrive without a notification.
    Poll,
}

/// A dedicated listening connection for the dispatcher.
pub struct OutboxWakeups {
    listener: PgListener,
}

impl OutboxWakeups {
    pub async fn listen(pool: &PgPool) -> Result<Self, OutboxError> {
        let mut listener = PgListener::connect_with(pool).await?;
        listener.listen(OUTBOX_CHANNEL).await?;
        Ok(Self { listener })
    }

    /// Waits for the next notification, at most `poll_interval`. Notifications that arrived
    /// while the dispatcher was busy return at once.
    pub async fn wait(&mut self, poll_interval: Duration) -> Result<OutboxWakeup, OutboxError> {
        match tokio::time::timeout(poll_interval, self.listener.try_recv()).await {
            Err(_) | Ok(Ok(None)) => Ok(OutboxWakeup::Poll),
            Ok(Ok(Some(_))) => Ok(OutboxWakeup::Captured),
            Ok(Err(error)) => Err(error.into()),
        }
    }
}
