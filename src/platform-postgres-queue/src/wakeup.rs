//! Wake-ups for idle workers: `LISTEN/NOTIFY` per scope, with polling as the fallback.

use crate::{error::JobQueueError, scope::ScopeName};
use sqlx::{PgPool, postgres::PgListener};
use std::time::Duration;

/// Why a waiting worker should claim again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wakeup {
    /// A transaction that enqueued jobs of this scope committed.
    Enqueued(ScopeName),
    /// The poll interval elapsed, or a notification named no listened scope. Retries, expired
    /// leases and unblocked FIFO groups become claimable without a notification.
    Poll,
    /// The listener lost its connection and reconnected, so it may have missed notifications
    /// of any scope. Claim from every scope.
    Reconnected,
}

/// A dedicated listening connection for a worker's scopes.
pub struct JobWakeups {
    listener: PgListener,
}

impl JobWakeups {
    pub async fn listen<'a>(
        pool: &PgPool,
        scopes: impl IntoIterator<Item = &'a ScopeName>,
    ) -> Result<Self, JobQueueError> {
        let mut listener = PgListener::connect_with(pool).await?;
        let channels: Vec<String> = scopes.into_iter().map(ScopeName::notify_channel).collect();
        listener
            .listen_all(channels.iter().map(String::as_str))
            .await?;
        Ok(Self { listener })
    }

    /// Waits for the next notification, at most `poll_interval`. Notifications that arrived
    /// while the worker was busy return at once.
    pub async fn wait(&mut self, poll_interval: Duration) -> Result<Wakeup, JobQueueError> {
        match tokio::time::timeout(poll_interval, self.listener.try_recv()).await {
            Err(_elapsed) => Ok(Wakeup::Poll),
            Ok(Ok(None)) => Ok(Wakeup::Reconnected),
            Ok(Ok(Some(notification))) => {
                Ok(ScopeName::from_notify_channel(notification.channel())
                    .map_or(Wakeup::Poll, Wakeup::Enqueued))
            }
            Ok(Err(error)) => Err(error.into()),
        }
    }
}
