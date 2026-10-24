//! Per-test resource ownership on shared, job-owned infrastructure.
mod lease;
mod names;
#[cfg(feature = "postgres")]
mod postgres;
mod resources;
mod session;
pub use session::{ExclusiveReservation, TestSession};

use application::error::BoxError;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

pub type Result<T> = std::result::Result<T, BoxError>;
type Cleanup = Pin<Box<dyn Future<Output = Result<()>> + Send>>;

fn failure(message: &'static str) -> BoxError {
    std::io::Error::other(message).into()
}

/// Exclusive reservations exclude every ordinary lease, across processes and runner invocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    Isolated,
    Exclusive,
}

/// A resource scope, explicitly cloned into spawned tasks or OS threads.
/// Cloning does not extend the test session; use after session teardown fails closed.
#[derive(Clone)]
pub struct TestContext(Arc<State>);

struct State {
    manifest: Mutex<Manifest>,
    path: std::path::PathBuf,
    persistence: Arc<tokio::sync::Mutex<()>>,
    reservation: std::sync::Weak<lease::Reservation>,
    closed: AtomicBool,
    cleanup: Mutex<Vec<Cleanup>>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    #[cfg(feature = "postgres")]
    pools: Mutex<Vec<sqlx::PgPool>>,
    #[cfg(feature = "postgres")]
    baseline: Mutex<Option<String>>,
    #[cfg(feature = "postgres")]
    postgres: Mutex<Option<sqlx::postgres::PgConnectOptions>>,
    #[cfg(feature = "aura-historia-api")]
    servers: Mutex<std::collections::HashMap<usize, Arc<tokio::sync::OnceCell<String>>>>,
    #[cfg(feature = "aura-historia-api")]
    api_state: Mutex<axum::http::Extensions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    run: String,
    invocation: String,
    test: String,
    name: String,
    slot: u16,
    resources: Vec<Resource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum Kind {
    Database,
    Index,
    Queue,
    Bucket,
    Sender,
    ConfigurationSet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Resource {
    kind: Kind,
    name: String,
}

tokio::task_local! { static CURRENT: TestContext; static EXCLUSIVE: (); }

#[cfg(feature = "ses")]
pub(crate) fn exclusive_active() -> bool {
    EXCLUSIVE.try_with(|_| ()).is_ok()
}

impl TestContext {
    pub async fn acquire(name: &str, isolation: Isolation) -> Result<TestSession> {
        let allocation = lease::Allocation::current().await?;
        Self::reserve(
            name,
            isolation,
            lease::Runner::environment(allocation.slots)?,
        )
        .await
    }

    /// A bounded non-Nextest path, including a child batch that must not inherit its parent's slot.
    pub async fn acquire_explicit(name: &str, isolation: Isolation) -> Result<TestSession> {
        Self::reserve(
            name,
            isolation,
            lease::Runner::parse(None, None, None, None, 1)?,
        )
        .await
    }

    async fn reserve(
        name: &str,
        isolation: Isolation,
        runner: lease::Runner,
    ) -> Result<TestSession> {
        if name.is_empty() || name.len() > 4096 {
            return Err(failure("test name must be nonempty and bounded"));
        }
        let allocation = lease::Allocation::current().await?;
        let reservation = allocation.reserve(runner, isolation).await?;
        let run = allocation.run.clone();
        let (reservation, path, previous) = tokio::task::spawn_blocking(move || -> Result<_> {
            let path = reservation
                .directory
                .join(format!("slot-{}.json", reservation.slot));
            let previous = if path.try_exists()? {
                let file = lease::lock_file(&path)?;
                let previous: Manifest = serde_json::from_reader(file)?;
                previous.validate(&run, reservation.slot)?;
                Some(previous)
            } else {
                None
            };
            Ok((reservation, path, previous))
        })
        .await??;
        if let Some(previous) = previous {
            resources::clean(&previous).await?;
        }
        let manifest = Manifest {
            version: 1,
            run: allocation.run.clone(),
            invocation: reservation.invocation.clone(),
            test: uuid::Uuid::new_v4().simple().to_string(),
            name: name.to_owned(),
            slot: reservation.slot,
            resources: vec![],
        };
        tokio::task::spawn_blocking(move || -> Result<_> {
            manifest.write(&path)?;
            let reservation = Arc::new(reservation);
            let context = Self(Arc::new(State {
                manifest: Mutex::new(manifest),
                path,
                persistence: Arc::new(tokio::sync::Mutex::new(())),
                reservation: Arc::downgrade(&reservation),
                closed: AtomicBool::new(false),
                cleanup: Mutex::new(vec![]),
                tasks: Mutex::new(vec![]),
                #[cfg(feature = "aura-historia-api")]
                servers: Mutex::new(std::collections::HashMap::new()),
                #[cfg(feature = "aura-historia-api")]
                api_state: Mutex::new(axum::http::Extensions::new()),
                #[cfg(feature = "postgres")]
                pools: Mutex::new(vec![]),
                #[cfg(feature = "postgres")]
                baseline: Mutex::new(None),
                #[cfg(feature = "postgres")]
                postgres: Mutex::new(None),
            }));
            Ok(TestSession {
                context,
                reservation: Some(reservation),
            })
        })
        .await?
    }

    pub fn current() -> Option<Self> {
        CURRENT.try_with(Clone::clone).ok()
    }
    pub async fn scope<T>(&self, future: impl Future<Output = T>) -> T {
        CURRENT.scope(self.clone(), future).await
    }
    pub fn run_id(&self) -> String {
        self.0
            .manifest
            .lock()
            .expect("context metadata lock")
            .run
            .clone()
    }
    pub fn test_id(&self) -> String {
        self.0
            .manifest
            .lock()
            .expect("context metadata lock")
            .test
            .clone()
    }
    pub fn slot(&self) -> u16 {
        self.0.manifest.lock().expect("context metadata lock").slot
    }

    fn active(&self) -> Result<()> {
        if self.0.closed.load(Ordering::Acquire) {
            Err(failure("test context is already closing"))
        } else {
            Ok(())
        }
    }

    fn name(&self, label: &str, maximum: usize, suffix: &str) -> Result<String> {
        self.active()?;
        let metadata = self.0.manifest.lock().expect("context metadata lock");
        names::resource(&metadata.run, &metadata.test, label, maximum, suffix)
    }

    fn own(&self, kind: Kind, name: String) -> Result<String> {
        self.active()?;
        let mut metadata = self.0.manifest.lock().expect("context metadata lock");
        self.active()?;
        let resource = Resource {
            kind,
            name: name.clone(),
        };
        if !metadata.resources.contains(&resource) {
            metadata.resources.push(resource);
        }
        Ok(name)
    }

    /// Register and durably record a resource before issuing its create request.
    pub async fn queue_name(&self, label: &str, fifo: bool) -> Result<String> {
        let name = self.resolve_queue_name(label, fifo)?;
        self.persist_resources().await?;
        Ok(name)
    }
    pub(crate) fn resolve_queue_name(&self, label: &str, fifo: bool) -> Result<String> {
        self.own(
            Kind::Queue,
            self.name(label, 80, if fifo { ".fifo" } else { "" })?,
        )
    }
    pub async fn bucket_name(&self, label: &str) -> Result<String> {
        let name = self.own(Kind::Bucket, self.name(label, 63, "")?)?;
        self.persist_resources().await?;
        Ok(name)
    }
    pub async fn configuration_set_name(&self, label: &str) -> Result<String> {
        let name = self.own(Kind::ConfigurationSet, self.name(label, 64, "")?)?;
        self.persist_resources().await?;
        Ok(name)
    }

    pub(crate) async fn persist_resources(&self) -> Result<()> {
        self.active()?;
        let guard = self.0.persistence.clone().lock_owned().await;
        self.active()?;
        let metadata = self
            .0
            .manifest
            .lock()
            .expect("context metadata lock")
            .clone();
        let path = self.0.path.clone();
        let reservation = self
            .0
            .reservation
            .upgrade()
            .ok_or_else(|| failure("test reservation has ended"))?;
        tokio::task::spawn_blocking(move || {
            // The owned guard survives cancellation of the awaiting test and prevents
            // cleanup or a following generation from overtaking this atomic publication.
            let _guard = guard;
            let _reservation = reservation;
            metadata.write(&path)
        })
        .await?
    }

    /// Durably register a labeled SES sender before verifying or using its identity.
    pub async fn register_sender(&self, label: &str) -> Result<String> {
        let email = self.own(
            Kind::Sender,
            format!("{}@example.test", self.name(label, 64, "")?),
        )?;
        self.persist_resources().await?;
        Ok(email)
    }

    pub fn sender_email(&self) -> Result<String> {
        self.own(
            Kind::Sender,
            format!("{}@example.test", self.name("sender", 64, "")?),
        )
    }
    #[cfg(feature = "sqs")]
    pub(crate) fn register_queue(&self, name: &str) -> Result<()> {
        let prefix = format!("a{}-{}-", self.run_id(), &self.test_id()[..16]);
        if !name.starts_with(&prefix) {
            return Err(failure("queue must be named by the active context"));
        }
        self.own(Kind::Queue, name.to_owned())?;
        Ok(())
    }
    pub fn product_listings_index(&self) -> Result<String> {
        self.own(Kind::Index, self.name("product-listings", 255, "")?)
    }

    /// Register shutdown that stops and joins an externally created server/worker before cleanup.
    pub fn on_shutdown(
        &self,
        shutdown: impl Future<Output = Result<()>> + Send + 'static,
    ) -> Result<()> {
        self.active()?;
        let mut cleanup = self.0.cleanup.lock().expect("context cleanup lock");
        self.active()?;
        cleanup.push(Box::pin(shutdown));
        Ok(())
    }

    /// Spawn a context-bound task. Teardown aborts and joins it even after panic/cancellation.
    pub fn spawn(
        &self,
        future: impl Future<Output = ()> + Send + 'static,
    ) -> Result<tokio::task::AbortHandle> {
        self.active()?;
        let mut tasks = self.0.tasks.lock().expect("context task lock");
        self.active()?;
        let context = self.clone();
        let task = tokio::spawn(async move { context.scope(future).await });
        let abort = task.abort_handle();
        tasks.push(task);
        Ok(abort)
    }

    /// Initialize typed API fake state once, before declaring the first scoped server.
    #[cfg(feature = "aura-historia-api")]
    pub fn insert_api_state<T: Clone + Send + Sync + 'static>(&self, state: T) -> Result<()> {
        let servers = self.0.servers.lock().expect("context server lock");
        self.active()?;
        if !servers.is_empty() {
            return Err(failure("insert API test state before starting its server"));
        }
        let mut values = self.0.api_state.lock().expect("API test state lock");
        self.active()?;
        if values.get::<T>().is_some() {
            return Err(failure(
                "API test state of this type is already initialized",
            ));
        }
        values.insert(state);
        Ok(())
    }

    #[cfg(feature = "aura-historia-api")]
    pub fn api_state<T: Clone + Send + Sync + 'static>(&self) -> Result<T> {
        self.active()?;
        self.0
            .api_state
            .lock()
            .expect("API test state lock")
            .get::<T>()
            .cloned()
            .ok_or_else(|| {
                failure("requested API test state is missing; insert it before server setup")
            })
    }

    #[cfg(feature = "aura-historia-api")]
    pub(crate) fn server_url(&self, key: usize) -> Option<String> {
        self.active().ok()?;
        self.0
            .servers
            .lock()
            .expect("context server lock")
            .get(&key)
            .and_then(|server| server.get())
            .cloned()
    }

    #[cfg(feature = "aura-historia-api")]
    pub(crate) fn server_initialization(
        &self,
        key: usize,
    ) -> Result<Arc<tokio::sync::OnceCell<String>>> {
        let mut servers = self.0.servers.lock().expect("context server lock");
        self.active()?;
        Ok(servers
            .entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new()))
            .clone())
    }
}

impl Manifest {
    fn validate(&self, run: &str, slot: u16) -> Result<()> {
        if self.version != 1
            || self.run != run
            || self.slot != slot
            || uuid::Uuid::parse_str(&self.invocation).is_err()
            || self.test.len() != 32
            || !self.test.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(failure("slot recovery metadata has invalid ownership"));
        }
        let prefix = format!("a{}-{}-", self.run, &self.test[..16]);
        if self.resources.iter().any(|resource| {
            let maximum = match resource.kind {
                Kind::Database => 63,
                Kind::Index => 255,
                Kind::Queue => 80,
                Kind::Bucket => 63,
                Kind::Sender => 77,
                Kind::ConfigurationSet => 64,
            };
            !resource.name.starts_with(&prefix)
                || resource.name.len() > maximum
                || !resource.name.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'-' | b'.' | b'@')
                })
                || (resource.kind == Kind::Sender && !resource.name.ends_with("@example.test"))
                || (resource.kind != Kind::Sender && resource.name.contains('@'))
        }) {
            return Err(failure("slot recovery contains an unowned resource"));
        }
        Ok(())
    }
    fn write(&self, path: &Path) -> Result<()> {
        use std::io::Write;
        let temporary = path.with_extension("json.tmp");
        let mut file = lease::lock_file(&temporary)?;
        file.set_len(0)?;
        file.write_all(&serde_json::to_vec(self)?)?;
        file.sync_data()?;
        std::fs::rename(temporary, path)?;
        Ok(())
    }
}

/// A real cross-process exclusion gate for raw global mutation and legacy fixture callers.
pub async fn reserve_exclusive() -> Result<ExclusiveReservation> {
    let allocation = lease::Allocation::current().await?;
    let runner = lease::Runner::environment(allocation.slots)?;
    Ok(ExclusiveReservation {
        _reservation: allocation.reserve(runner, Isolation::Exclusive).await?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[tokio::test]
    async fn should_keep_context_explicit_across_tasks_and_cleanup_before_reuse() {
        let session = TestContext::acquire_explicit("cancellation", Isolation::Isolated)
            .await
            .unwrap();
        let context = session.context().clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let finished = stopped.clone();
        context
            .on_shutdown(async move {
                finished.store(true, Ordering::Release);
                Ok(())
            })
            .unwrap();
        let (started, ready) = tokio::sync::oneshot::channel();
        let expected = context.test_id();
        context
            .spawn(async move {
                assert_eq!(expected, TestContext::current().unwrap().test_id());
                let _ = started.send(());
                std::future::pending::<()>().await;
            })
            .unwrap();
        ready.await.unwrap();
        drop(session);
        let next = TestContext::reserve(
            "after cancellation",
            Isolation::Isolated,
            lease::Runner {
                invocation: uuid::Uuid::new_v4().simple().to_string(),
                slot: Some(context.slot()),
            },
        )
        .await
        .unwrap();
        assert!(stopped.load(Ordering::Acquire));
        assert!(
            context.product_listings_index().is_err(),
            "expired contexts cannot register more resources"
        );
        next.finish().await.unwrap();
    }

    #[tokio::test]
    async fn should_report_cleanup_failure_and_recover_the_slot_before_reuse() {
        let session = TestContext::acquire_explicit("cleanup failure", Isolation::Isolated)
            .await
            .unwrap();
        let slot = session.context().slot();
        let stopped = Arc::new(AtomicBool::new(false));
        let finished = stopped.clone();
        session
            .context()
            .on_shutdown(async move {
                finished.store(true, Ordering::Release);
                Ok(())
            })
            .unwrap();
        session
            .context()
            .on_shutdown(async { Err(failure("cleanup canary")) })
            .unwrap();
        let error = session.finish().await.unwrap_err();
        assert!(error.to_string().contains("cleanup canary"));
        assert!(
            stopped.load(Ordering::Acquire),
            "one cleanup failure must not skip other shutdowns"
        );
        let next = TestContext::reserve(
            "cleanup recovery",
            Isolation::Isolated,
            lease::Runner {
                invocation: uuid::Uuid::new_v4().simple().to_string(),
                slot: Some(slot),
            },
        )
        .await
        .unwrap();
        next.finish().await.unwrap();
    }

    #[tokio::test]
    async fn should_bound_non_nextest_execution_through_the_same_reservation() {
        let running = Arc::new(AtomicUsize::new(0));
        let mut tasks = vec![];
        for _ in 0..4 {
            let running = running.clone();
            tasks.push(tokio::spawn(async move {
                let session = TestContext::acquire_explicit("bounded", Isolation::Exclusive)
                    .await
                    .unwrap();
                assert_eq!(0, running.fetch_add(1, Ordering::SeqCst));
                tokio::task::yield_now().await;
                assert_eq!(1, running.fetch_sub(1, Ordering::SeqCst));
                session.finish().await.unwrap();
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
    }

    #[test]
    fn should_reject_recovery_metadata_that_belongs_to_another_run_or_slot() {
        let manifest = Manifest {
            version: 1,
            run: "0".repeat(32),
            invocation: uuid::Uuid::new_v4().simple().to_string(),
            test: "1".repeat(32),
            name: "ownership".into(),
            slot: 0,
            resources: vec![Resource {
                kind: Kind::Queue,
                name: "foreign-queue".into(),
            }],
        };
        assert!(manifest.validate(&manifest.run, 0).is_err());
        assert!(manifest.validate(&"2".repeat(32), 0).is_err());
        assert!(manifest.validate(&manifest.run, 1).is_err());
    }

    #[cfg(feature = "aura-historia-api")]
    #[tokio::test]
    async fn should_own_typed_api_state_and_share_the_same_arc_across_threads() {
        let session = TestContext::acquire_explicit("API state", Isolation::Isolated)
            .await
            .unwrap();
        let context = session.context().clone();
        assert!(context.api_state::<Arc<AtomicUsize>>().is_err());
        let state = Arc::new(AtomicUsize::new(17));
        context.insert_api_state(state.clone()).unwrap();
        assert!(context.insert_api_state(state.clone()).is_err());
        let thread_context = context.clone();
        let threaded =
            std::thread::spawn(move || thread_context.api_state::<Arc<AtomicUsize>>().unwrap())
                .join()
                .unwrap();
        assert!(Arc::ptr_eq(&state, &threaded));
        assert_eq!(17, threaded.load(Ordering::SeqCst));
        let weak = Arc::downgrade(&state);
        drop(threaded);
        drop(state);
        session.finish().await.unwrap();
        assert!(
            weak.upgrade().is_none(),
            "context must release API fake state after server shutdown"
        );
        assert!(context.api_state::<Arc<AtomicUsize>>().is_err());
        let next = TestContext::acquire_explicit("another API state", Isolation::Isolated)
            .await
            .unwrap();
        assert!(next.context().api_state::<Arc<AtomicUsize>>().is_err());
        next.finish().await.unwrap();
    }

    #[tokio::test]
    async fn should_continue_shutdown_and_recover_when_a_cleanup_future_panics() {
        let session = TestContext::acquire_explicit("panicking cleanup", Isolation::Isolated)
            .await
            .unwrap();
        let slot = session.context().slot();
        let stopped = Arc::new(AtomicBool::new(false));
        let finished = stopped.clone();
        session
            .context()
            .on_shutdown(async move {
                finished.store(true, Ordering::Release);
                Ok(())
            })
            .unwrap();
        session
            .context()
            .on_shutdown(async {
                panic!("cleanup future canary");
            })
            .unwrap();
        assert!(
            session
                .finish()
                .await
                .unwrap_err()
                .to_string()
                .contains("shutdown panicked")
        );
        assert!(stopped.load(Ordering::Acquire));
        let next = TestContext::reserve(
            "after cleanup panic",
            Isolation::Isolated,
            lease::Runner {
                invocation: uuid::Uuid::new_v4().simple().to_string(),
                slot: Some(slot),
            },
        )
        .await
        .unwrap();
        next.finish().await.unwrap();
    }

    #[tokio::test]
    async fn should_complete_concurrent_metadata_writes_before_releasing_the_slot() {
        let session = TestContext::acquire_explicit("concurrent metadata", Isolation::Isolated)
            .await
            .unwrap();
        let context = session.context().clone();
        let path = context.0.path.clone();
        for label in [
            "",
            "UPPERCASE",
            "sender@example.test",
            "a-label-that-is-too-long",
        ] {
            assert!(context.register_sender(label).await.is_err());
        }
        let mut writers = Vec::new();
        for _ in 0..8 {
            let context = context.clone();
            writers.push(tokio::spawn(
                async move { context.persist_resources().await },
            ));
        }
        session.finish().await.unwrap();
        for writer in writers {
            // Teardown may close the context before a queued writer acquires the mutex.
            let _ = writer.await.unwrap();
        }
        assert!(!tokio::fs::try_exists(path).await.unwrap());
        assert!(context.persist_resources().await.is_err());
    }
}
