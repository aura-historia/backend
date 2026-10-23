//! Result-bearing test tasks whose actual join remains owned by the context.
use crate::TestContext;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tokio::{sync::oneshot, task::AbortHandle};

struct Worker<T> {
    join: Option<tokio::task::JoinHandle<T>>,
    result: Option<oneshot::Sender<Result<T, tokio::task::JoinError>>>,
}

struct Shutdown<T> {
    abort: AbortHandle,
    waiter: tokio::task::JoinHandle<()>,
    worker: Arc<tokio::sync::Mutex<Worker<T>>>,
}

async fn finish_worker<T>(worker: &tokio::sync::Mutex<Worker<T>>) {
    let mut worker = worker.lock().await;
    if let Some(join) = worker.join.as_mut() {
        // Cancellation of the waiter leaves this actual join available to context shutdown.
        let result = join.await;
        worker.join.take();
        if let Some(sender) = worker.result.take() {
            let _ = sender.send(result);
        }
    }
}

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
            let handles: Option<Shutdown<T>> =
                shutdown.lock().expect("owned task shutdown lock").take();
            if let Some(handles) = handles {
                handles.abort.abort();
                let result = handles.waiter.await;
                // A runtime may cancel the waiter while its worker is still being polled.
                // Recover the actual worker join before reporting any waiter failure.
                finish_worker(&handles.worker).await;
                if let Err(error) = result
                    && !error.is_cancelled()
                {
                    return Err(error.into());
                }
            }
            Ok(())
        })
        .expect("register owned task before spawning");
    let (sender, result) = oneshot::channel();
    let join = tokio::spawn(async move { context.scope(future).await });
    let abort = join.abort_handle();
    let worker = Arc::new(tokio::sync::Mutex::new(Worker {
        join: Some(join),
        result: Some(sender),
    }));
    let waiting = worker.clone();
    let waiter = tokio::spawn(async move {
        finish_worker(&waiting).await;
    });
    *initialize = Some(Shutdown {
        abort: abort.clone(),
        waiter,
        worker,
    });
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
    #[tokio::test]
    async fn should_retain_the_lease_until_a_worker_joins_after_its_runtime_cancels_the_waiter() {
        struct Finished(Arc<AtomicBool>);
        impl Drop for Finished {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let session =
            TestContext::acquire_explicit("owned task runtime shutdown", Isolation::Exclusive)
                .await
                .unwrap();
        let context = session.context().clone();
        let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let blocked = release.clone();
        let finished = Arc::new(AtomicBool::new(false));
        let marker = finished.clone();
        let caller = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            let (started, running) = std::sync::mpsc::channel();
            runtime.block_on(context.scope(async move {
                drop(spawn_owned(async move {
                    let _finished = Finished(marker);
                    started.send(()).unwrap();
                    let (lock, wake) = &*blocked;
                    drop(
                        wake.wait_while(lock.lock().unwrap(), |released| !*released)
                            .unwrap(),
                    );
                }));
            }));
            running
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            runtime.shutdown_background();
        });
        caller.join().unwrap();
        let cleanup = tokio::spawn(session.finish());
        let next = tokio::spawn(TestContext::acquire_explicit(
            "after caller shutdown",
            Isolation::Isolated,
        ));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let held =
            !cleanup.is_finished() && !next.is_finished() && !finished.load(Ordering::Acquire);
        *release.0.lock().unwrap() = true;
        release.1.notify_all();
        cleanup.await.unwrap().unwrap();
        let next = next.await.unwrap().unwrap();
        assert!(
            held,
            "cleanup and the slot must wait for the actual worker join"
        );
        assert!(finished.load(Ordering::Acquire));
        next.finish().await.unwrap();
    }
}
