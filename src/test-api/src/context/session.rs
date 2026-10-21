//! Session ownership and bounded shutdown on an independent cleanup runtime.
use super::{EXCLUSIVE, Result, TestContext, failure, lease, resources};
use application::error::BoxError;
use futures_util::{FutureExt, StreamExt};
use std::future::Future;
use std::sync::{Arc, atomic::Ordering};
use std::time::Duration;

impl ExclusiveReservation {
    pub async fn scope<T>(&self, future: impl Future<Output = T>) -> T {
        EXCLUSIVE.scope((), future).await
    }
}

/// The non-cloneable owner of a reservation. Drop transfers cleanup and the reservation to an
/// independent thread, so cancellation of a test runtime cannot release a dirty slot early.
#[must_use]
pub struct TestSession {
    pub(super) context: TestContext,
    pub(super) reservation: Option<Arc<lease::Reservation>>,
}

/// For legacy fixtures and pristine cluster tests that do not use a scoped context.
/// Hold this guard for the complete operation. Ordinary leases share the same gate.
#[must_use]
pub struct ExclusiveReservation {
    pub(super) _reservation: lease::Reservation,
}

impl TestSession {
    pub fn context(&self) -> &TestContext {
        &self.context
    }
    pub async fn finish(mut self) -> Result<()> {
        let reservation = self.reservation.take().expect("session owns reservation");
        let receiver = start_cleanup(self.context.clone(), reservation, Duration::from_secs(90))?;
        receiver
            .await
            .map_err(|_| failure("test cleanup worker failed"))?
    }
}

impl Drop for TestSession {
    fn drop(&mut self) {
        if let Some(reservation) = self.reservation.take()
            && let Err(error) =
                start_cleanup(self.context.clone(), reservation, Duration::from_secs(90))
        {
            eprintln!("test cleanup could not start: {error}");
        }
    }
}

fn start_cleanup(
    context: TestContext,
    reservation: Arc<lease::Reservation>,
    deadline: Duration,
) -> Result<tokio::sync::oneshot::Receiver<Result<()>>> {
    context.0.closed.store(true, Ordering::Release);
    // Abort immediately, before a cancelled runtime can detach its worker tasks.
    for task in context.0.tasks.lock().expect("context task lock").iter() {
        task.abort();
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let mut sender = Some(sender);
    std::thread::Builder::new()
        .name("aura-test-cleanup".into())
        .spawn(move || {
            let _reservation = reservation;
            let started = std::time::Instant::now();
            let result = (|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                runtime.block_on(async {
                    let mut cleanup = Box::pin(clean_context(&context, deadline));
                    match tokio::time::timeout(deadline, &mut cleanup).await {
                        Ok(result) => result,
                        Err(error) => {
                            eprintln!("test cleanup deadline elapsed; reservation and state remain held until owned shutdown completes");
                            let _ = sender.take().expect("cleanup result sender").send(Err(error.into()));
                            // A timeout must not detach a join or let the next holder overtake
                            // an owned writer. Continue on this independent runtime.
                            cleanup.await?;
                            Err(failure("test cleanup exceeded its deadline; recovery metadata retained"))
                        }
                    }
                })?;
                // This thread owns the reservation through the final synchronous removal.
                // Cancellation of the test runtime cannot leave a delayed unlink behind.
                std::fs::remove_file(&context.0.path)?;
                Ok(())
            })();
            if let Err(error) = &result {
                eprintln!("test cleanup failed; next slot holder must recover: {error}");
            }
            let owner = context
                .0
                .manifest
                .lock()
                .expect("context metadata lock")
                .name
                .clone();
            crate::benchmark::record_timing("CONTEXT_CLEANUP", &owner, started.elapsed());
            if let Some(sender) = sender {
                let _ = sender.send(result);
            }
        })?;
    Ok(receiver)
}

async fn clean_context(context: &TestContext, deadline: Duration) -> Result<()> {
    let mut first = shutdown_owned_work(context, deadline).await.err();
    #[cfg(feature = "postgres")]
    {
        let mut close = Box::pin(context.close_postgres_pools());
        if let Err(error) =
            tokio::time::timeout(deadline.min(Duration::from_secs(10)), &mut close).await
        {
            first.get_or_insert(error.into());
            close.await;
        }
    }
    #[cfg(feature = "aura-historia-api")]
    context
        .0
        .api_state
        .lock()
        .expect("API test state lock")
        .clear();
    let _persistence = context.0.persistence.clone().lock_owned().await;
    let metadata = context
        .0
        .manifest
        .lock()
        .expect("context metadata lock")
        .clone();
    if let Err(error) = resources::clean(&metadata).await {
        first.get_or_insert(error);
    }
    first.map_or(Ok(()), Err)
}

async fn shutdown_owned_work(context: &TestContext, deadline: Duration) -> Result<()> {
    let mut first: Option<BoxError> = None;
    let mut pending = futures_util::stream::FuturesUnordered::<super::Cleanup>::new();
    let tasks = std::mem::take(&mut *context.0.tasks.lock().expect("context task lock"));
    for mut task in tasks {
        let result =
            match tokio::time::timeout(deadline.min(Duration::from_secs(5)), &mut task).await {
                Ok(result) => task_result(result),
                Err(error) => {
                    pending.push(Box::pin(async move { task_result(task.await) }));
                    Err(error.into())
                }
            };
        if let Err(error) = result {
            first.get_or_insert(error);
        }
    }
    let cleanup = std::mem::take(&mut *context.0.cleanup.lock().expect("context cleanup lock"));
    for shutdown in cleanup.into_iter().rev() {
        let mut shutdown = std::panic::AssertUnwindSafe(shutdown).catch_unwind();
        let result = match tokio::time::timeout(
            deadline.min(Duration::from_secs(10)),
            &mut shutdown,
        )
        .await
        {
            Ok(result) => shutdown_result(result),
            Err(error) => {
                pending.push(Box::pin(async move { shutdown_result(shutdown.await) }));
                Err(error.into())
            }
        };
        if let Err(error) = result {
            first.get_or_insert(error);
        }
    }
    // Attempt every shutdown before awaiting delayed joins together. Keep API state,
    // pools and the reservation alive until all of them actually complete.
    while let Some(result) = pending.next().await {
        if let Err(error) = result {
            first.get_or_insert(error);
        }
    }
    first.map_or(Ok(()), Err)
}

fn task_result(result: std::result::Result<(), tokio::task::JoinError>) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.is_cancelled() => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn shutdown_result(result: std::thread::Result<Result<()>>) -> Result<()> {
    result.unwrap_or_else(|_| Err(failure("test-owned server or worker shutdown panicked")))
}

#[cfg(all(test, feature = "aura-historia-api"))]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[tokio::test]
    async fn should_retain_reservation_and_api_state_until_timed_out_shutdown_really_joins() {
        let mut session =
            TestContext::acquire_explicit("delayed shutdown", super::super::Isolation::Isolated)
                .await
                .unwrap();
        let context = session.context().clone();
        let state = Arc::new(AtomicBool::new(false));
        let weak = Arc::downgrade(&state);
        context.insert_api_state(state.clone()).unwrap();
        drop(state);
        let (release, wait) = std::sync::mpsc::channel();
        let thread_weak = weak.clone();
        let worker = std::thread::spawn(move || {
            wait.recv().unwrap();
            assert!(
                thread_weak.upgrade().is_some(),
                "API state must outlive its worker"
            );
        });
        let joined = Arc::new(AtomicBool::new(false));
        let worker_joined = joined.clone();
        let continued = Arc::new(AtomicBool::new(false));
        let cleanup_continued = continued.clone();
        context
            .on_shutdown(async move {
                cleanup_continued.store(true, Ordering::Release);
                Ok(())
            })
            .unwrap();
        context
            .on_shutdown(async move {
                tokio::task::spawn_blocking(move || worker.join())
                    .await?
                    .map_err(|_| failure("delayed worker panicked"))?;
                worker_joined.store(true, Ordering::Release);
                Ok(())
            })
            .unwrap();
        let receiver = start_cleanup(
            context.clone(),
            session.reservation.take().unwrap(),
            Duration::from_millis(50),
        )
        .unwrap();
        let error = tokio::time::timeout(Duration::from_secs(2), receiver)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("deadline"));
        assert!(weak.upgrade().is_some());
        assert!(!joined.load(Ordering::Acquire));
        assert!(tokio::fs::try_exists(&context.0.path).await.unwrap());
        let next = TestContext::reserve(
            "after delayed shutdown",
            super::super::Isolation::Isolated,
            lease::Runner {
                invocation: uuid::Uuid::new_v4().simple().to_string(),
                slot: Some(context.slot()),
            },
        );
        tokio::pin!(next);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut next)
                .await
                .is_err(),
            "a caller timeout cannot release the slot while its worker is still alive"
        );
        assert!(
            continued.load(Ordering::Acquire),
            "a delayed join must not skip other shutdowns"
        );
        assert!(weak.upgrade().is_some());
        release.send(()).unwrap();
        let next = tokio::time::timeout(Duration::from_secs(5), next)
            .await
            .unwrap()
            .unwrap();
        assert!(joined.load(Ordering::Acquire));
        assert!(
            weak.upgrade().is_none(),
            "API state is released after the join"
        );
        next.finish().await.unwrap();
    }
}
