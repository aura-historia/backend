//! Routes `LISTEN/NOTIFY` wake-ups to the consumer of each scope.

use platform_postgres_queue::{JobWakeups, ScopeName, Wakeup};
use sqlx::PgPool;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const LISTEN_INTERVAL: Duration = Duration::from_secs(30);
const RECONNECT_DELAY: Duration = Duration::from_secs(1);

/// One notification handle per scope. A notification that arrives while its consumer is busy is
/// kept, so the consumer claims again as soon as it is free.
#[derive(Clone, Default)]
pub struct ScopeWakeups {
    scopes: HashMap<ScopeName, Arc<Notify>>,
}

impl ScopeWakeups {
    /// The handle a scope's consumer waits on.
    pub fn register(&mut self, scope: &ScopeName) -> Arc<Notify> {
        Arc::clone(self.scopes.entry(scope.clone()).or_default())
    }

    /// Wakes the consumers a wake-up concerns. A poll wakes none, since consumers poll on their
    /// own; a reconnected listener may have missed any scope's notification.
    fn route(&self, wakeup: Wakeup) {
        match wakeup {
            Wakeup::Enqueued(scope) => self.wake(&scope),
            Wakeup::Poll => {}
            Wakeup::Reconnected => self.wake_all(),
        }
    }

    fn wake(&self, scope: &ScopeName) {
        if let Some(notify) = self.scopes.get(scope) {
            notify.notify_one();
        }
    }

    fn wake_all(&self) {
        self.scopes.values().for_each(|notify| notify.notify_one());
    }

    /// Listens on every registered scope until `shutdown`. A lost listener reconnects and wakes
    /// every consumer, whether it reconnected on its own or had to be replaced, since
    /// notifications may have been missed meanwhile; consumers also poll on their own, so a
    /// missed notification only delays a job.
    pub async fn listen(self, pool: PgPool, shutdown: CancellationToken) {
        while !shutdown.is_cancelled() {
            match JobWakeups::listen(&pool, self.scopes.keys()).await {
                Ok(mut wakeups) => loop {
                    let wakeup = tokio::select! {
                        () = shutdown.cancelled() => return,
                        wakeup = wakeups.wait(LISTEN_INTERVAL) => wakeup,
                    };
                    match wakeup {
                        Ok(wakeup) => self.route(wakeup),
                        Err(_) => {
                            tracing::warn!(
                                event = "worker.wakeups.disconnected",
                                "Job wake-up listener disconnected; reconnecting"
                            );
                            break;
                        }
                    }
                },
                Err(_) => tracing::warn!(
                    event = "worker.wakeups.unavailable",
                    "Job wake-up listener could not connect; consumers keep polling"
                ),
            }
            self.wake_all();
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(RECONNECT_DELAY) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_woken(notify: &Notify) -> bool {
        // Enabling consumes a stored permit and reports it.
        std::pin::pin!(notify.notified()).enable()
    }

    fn scopes() -> (ScopeWakeups, Arc<Notify>, Arc<Notify>) {
        let mut wakeups = ScopeWakeups::default();
        let first = wakeups.register(&ScopeName::new("first").unwrap());
        let second = wakeups.register(&ScopeName::new("second").unwrap());
        (wakeups, first, second)
    }

    #[test]
    fn should_wake_only_the_enqueued_scope() {
        let (wakeups, first, second) = scopes();

        wakeups.route(Wakeup::Enqueued(ScopeName::new("first").unwrap()));

        assert_eq!((true, false), (is_woken(&first), is_woken(&second)));
    }

    #[test]
    fn should_wake_every_scope_when_the_listener_reconnected() {
        let (wakeups, first, second) = scopes();

        wakeups.route(Wakeup::Reconnected);

        assert_eq!((true, true), (is_woken(&first), is_woken(&second)));
    }

    #[test]
    fn should_leave_polling_to_the_consumers() {
        let (wakeups, first, second) = scopes();

        wakeups.route(Wakeup::Poll);

        assert_eq!((false, false), (is_woken(&first), is_woken(&second)));
    }
}
