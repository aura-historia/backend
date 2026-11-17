//! The outbox dispatcher loop: routes captured inserts into the job queue as they commit.

use delivery_outbox::{Dispatch, OutboxDispatcher, OutboxWakeups};
use sqlx::PgPool;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const FAILURE_BACKOFF: Duration = Duration::from_secs(1);

/// Dispatches until `shutdown`. A full batch dispatches again at once; otherwise the loop waits
/// for a capture notification or the poll interval. Only one dispatch transaction runs at a time
/// across all workers, so a busy lock simply waits for the next round. A failed dispatch rolls
/// back and is retried with the same effect.
pub async fn run(
    pool: PgPool,
    dispatcher: OutboxDispatcher,
    batch_size: usize,
    poll_interval: Duration,
    shutdown: CancellationToken,
) {
    let mut wakeups = None;
    while !shutdown.is_cancelled() {
        let more = match dispatcher.dispatch_batch().await {
            Ok(Dispatch::Dispatched(report)) => {
                if report.claimed > 0 {
                    tracing::debug!(
                        event = "worker.dispatch.completed",
                        claimed = report.claimed,
                        enqueued = report.enqueued,
                        deduplicated = report.deduplicated,
                        custody = report.custody,
                    );
                }
                report.claimed >= batch_size
            }
            Ok(Dispatch::Busy) => false,
            Err(_) => {
                tracing::warn!(
                    event = "worker.dispatch.failed",
                    "Outbox dispatch failed and rolled back; retrying"
                );
                tokio::select! {
                    () = shutdown.cancelled() => return,
                    () = tokio::time::sleep(FAILURE_BACKOFF) => continue,
                }
            }
        };
        if more {
            continue;
        }
        if wakeups.is_none() {
            wakeups = OutboxWakeups::listen(&pool).await.ok();
        }
        let waited = match wakeups.as_mut() {
            Some(listener) => tokio::select! {
                () = shutdown.cancelled() => return,
                waited = listener.wait(poll_interval) => waited.is_ok(),
            },
            None => tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(poll_interval) => true,
            },
        };
        if !waited {
            // Reconnect on the next round; dispatching first covers missed notifications.
            wakeups = None;
        }
    }
}
