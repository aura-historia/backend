//! Result-bearing test tasks whose actual join remains owned by the context.
use crate::TestContext;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tokio::{sync::oneshot, task::AbortHandle};

/// A worker's original Tokio join result, with context-owned shutdown if the body stops waiting.
pub struct OwnedTask<T> {
    result: oneshot::Receiver<Result<T, tokio::task::JoinError>>,
    abort: AbortHandle,
}

impl<T> OwnedTask<T> {
    pub fn abort(&self) {
        self.abort.abort();
    }

    pub fn is_finished(&self) -> bool {
        self.abort.is_finished()
    }
}

impl<T> Future for OwnedTask<T> {
    type Output = Result<T, tokio::task::JoinError>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.result).poll(context).map(|result| {
            result.expect("context-owned task waiter closed without returning its worker result")
        })
    }
}

/// Spawn inside the current context while retaining `JoinError`, `abort`, and `is_finished`.
/// Shutdown ownership is registered before either task exists; the initialization lock prevents
/// concurrent context shutdown from overtaking publication of their handles.
pub fn spawn_owned<T: Send + 'static>(
    future: impl Future<Output = T> + Send + 'static,
) -> OwnedTask<T> {
    let context = TestContext::current().expect("spawn_owned requires an active test context");
    let ownership = Arc::new(Mutex::new(None));
    let mut initialize = ownership.lock().expect("owned task initialization lock");
    let shutdown = ownership.clone();
    context
        .on_shutdown(async move {
            let handles: Option<(AbortHandle, tokio::task::JoinHandle<()>)> =
                shutdown.lock().expect("owned task shutdown lock").take();
            if let Some((abort, waiter)) = handles {
                abort.abort();
                // The waiter owns and joins the worker, including its panic/cancellation result.
                waiter.await?;
            }
            Ok(())
        })
        .expect("register owned task before spawning");
    let (sender, result) = oneshot::channel();
    let worker = tokio::spawn(async move { context.scope(future).await });
    let abort = worker.abort_handle();
    let waiter = tokio::spawn(async move {
        let _ = sender.send(worker.await);
    });
    *initialize = Some((abort.clone(), waiter));
    drop(initialize);
    OwnedTask { result, abort }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Isolation;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn should_preserve_worker_results_panics_and_cancellation() {
        let session = TestContext::acquire_explicit("owned task results", Isolation::Isolated)
            .await
            .unwrap();
        session
            .context()
            .scope(async {
                assert_eq!(42, spawn_owned(async { 42 }).await.unwrap());
                assert!(
                    spawn_owned(async { panic!("worker panic") })
                        .await
                        .unwrap_err()
                        .is_panic()
                );
                let mut pending = spawn_owned(std::future::pending::<()>());
                assert!(
                    tokio::time::timeout(std::time::Duration::from_millis(10), &mut pending)
                        .await
                        .is_err()
                );
                assert!(!pending.is_finished());
                pending.abort();
                assert!(pending.await.unwrap_err().is_cancelled());
            })
            .await;
        session.finish().await.unwrap();
    }

    #[tokio::test]
    async fn should_join_a_worker_after_its_result_handle_is_dropped() {
        struct Finished(Arc<AtomicBool>);
        impl Drop for Finished {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let session =
            TestContext::acquire_explicit("owned task dropped result", Isolation::Isolated)
                .await
                .unwrap();
        let finished = Arc::new(AtomicBool::new(false));
        let marker = finished.clone();
        let (started, running) = oneshot::channel();
        session
            .context()
            .scope(async move {
                drop(spawn_owned(async move {
                    let _finished = Finished(marker);
                    started.send(()).unwrap();
                    std::future::pending::<()>().await;
                }));
                running.await.unwrap();
            })
            .await;
        session.finish().await.unwrap();
        assert!(finished.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn should_not_start_work_when_shutdown_registration_is_rejected() {
        let session =
            TestContext::acquire_explicit("owned task closed context", Isolation::Isolated)
                .await
                .unwrap();
        let context = session.context().clone();
        session.finish().await.unwrap();
        let started = Arc::new(AtomicBool::new(false));
        let marker = started.clone();
        context
            .scope(async move {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    spawn_owned(async move {
                        marker.store(true, Ordering::Release);
                    })
                }));
                assert!(result.is_err());
            })
            .await;
        assert!(!started.load(Ordering::Acquire));
    }
}
