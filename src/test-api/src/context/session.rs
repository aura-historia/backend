//! Session ownership and bounded shutdown on an independent cleanup runtime.
use super::{EXCLUSIVE, Result, TestContext, failure, lease, resources};
use application::error::BoxError;
use futures_util::FutureExt;
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
        let receiver = start_cleanup(self.context.clone(), reservation)?;
        receiver
            .await
            .map_err(|_| failure("test cleanup worker failed"))?
    }
}

impl Drop for TestSession {
    fn drop(&mut self) {
        if let Some(reservation) = self.reservation.take()
            && let Err(error) = start_cleanup(self.context.clone(), reservation)
        {
            eprintln!("test cleanup could not start: {error}");
        }
    }
}

fn start_cleanup(
    context: TestContext,
    reservation: Arc<lease::Reservation>,
) -> Result<tokio::sync::oneshot::Receiver<Result<()>>> {
    context.0.closed.store(true, Ordering::Release);
    // Abort immediately, before a cancelled runtime can detach its worker tasks.
    for task in context.0.tasks.lock().expect("context task lock").iter() {
        task.abort();
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
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
                    tokio::time::timeout(Duration::from_secs(90), clean_context(&context)).await?
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
            let _ = sender.send(result);
        })?;
    Ok(receiver)
}

async fn clean_context(context: &TestContext) -> Result<()> {
    let mut first: Option<BoxError> = None;
    let tasks = std::mem::take(&mut *context.0.tasks.lock().expect("context task lock"));
    for task in tasks {
        let result = match tokio::time::timeout(Duration::from_secs(5), task).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) if error.is_cancelled() => Ok(()),
            Ok(Err(error)) => Err(error.into()),
            Err(error) => Err(error.into()),
        };
        if let Err(error) = result {
            first.get_or_insert(error);
        }
    }
    let cleanup = std::mem::take(&mut *context.0.cleanup.lock().expect("context cleanup lock"));
    for shutdown in cleanup.into_iter().rev() {
        let result = match tokio::time::timeout(
            Duration::from_secs(10),
            std::panic::AssertUnwindSafe(shutdown).catch_unwind(),
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(failure("test-owned server or worker shutdown panicked")),
            Err(error) => Err(error.into()),
        };
        if let Err(error) = result {
            first.get_or_insert(error);
        }
    }
    #[cfg(feature = "postgres")]
    if let Err(error) =
        tokio::time::timeout(Duration::from_secs(10), context.close_postgres_pools()).await
    {
        first.get_or_insert(error.into());
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
    if let Some(error) = first {
        return Err(error);
    }
    Ok(())
}
