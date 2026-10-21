use crate::IntegrationTestService;
use async_trait::async_trait;
use axum::Router;
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

pub type AuraHistoriaApiAppFactory = fn() -> Pin<Box<dyn Future<Output = Router> + Send>>;

pub struct AuraHistoriaApi {
    app_factory: AuraHistoriaApiAppFactory,
    base_url: OnceLock<String>,
}

impl AuraHistoriaApi {
    pub const fn new(app_factory: AuraHistoriaApiAppFactory) -> Self {
        Self {
            app_factory,
            base_url: OnceLock::new(),
        }
    }

    pub fn base_url(&self) -> String {
        if let Some(context) = crate::TestContext::current() {
            return context
                .server_url(self as *const Self as usize)
                .expect("context API server should be running");
        }
        match self.base_url.get() {
            Some(base_url) => base_url.clone(),
            None => panic!("aura-historia-api test server should be running"),
        }
    }
}

#[async_trait]
impl IntegrationTestService for &'static AuraHistoriaApi {
    fn service_names(&self) -> &'static [&'static str] {
        &[]
    }

    async fn set_up(&self) {
        if let Some(context) = crate::TestContext::current() {
            self.start_scoped(context).await;
            return;
        }
        if self.base_url.get().is_some() {
            return;
        }

        let factory = self.app_factory;
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        match std::thread::Builder::new()
            .name("aura-historia-api-test".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _send_result = ready_tx.send(Err(format!(
                            "failed to build aura-historia-api test runtime: {error}"
                        )));
                        return;
                    }
                };

                runtime.block_on(async move {
                    let app = factory().await;
                    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
                        Ok(listener) => listener,
                        Err(error) => {
                            let _send_result = ready_tx.send(Err(format!(
                                "failed to bind aura-historia-api test listener: {error}"
                            )));
                            return;
                        }
                    };
                    let addr = match listener.local_addr() {
                        Ok(addr) => addr,
                        Err(error) => {
                            let _send_result = ready_tx.send(Err(format!(
                                "failed to read aura-historia-api test listener address: {error}"
                            )));
                            return;
                        }
                    };
                    let _send_result = ready_tx.send(Ok(format!("http://{addr}")));
                    if let Err(error) = axum::serve(listener, app).await {
                        panic!("aura-historia-api test server failed: {error}");
                    }
                });
            }) {
            Ok(_handle) => {}
            Err(error) => panic!("failed to spawn aura-historia-api test thread: {error}"),
        }

        let base_url = match ready_rx.recv() {
            Ok(Ok(base_url)) => base_url,
            Ok(Err(error)) => panic!("{error}"),
            Err(error) => panic!("failed to receive aura-historia-api test base URL: {error}"),
        };
        let _set_result = self.base_url.set(base_url);
    }
}

impl AuraHistoriaApi {
    async fn start_scoped(&'static self, context: crate::TestContext) {
        let key = self as *const Self as usize;
        let server = context
            .server_initialization(key)
            .expect("active API context");
        let (ready, waiting) = tokio::sync::oneshot::channel();
        let setup_context = context.clone();
        // The initializer belongs to the context, so a cancelled setup waiter cannot
        // reset the OnceCell and start a second factory while the first still runs.
        context
            .spawn(async move {
                server
                    .get_or_init(|| self.spawn_scoped(setup_context))
                    .await;
                let _ = ready.send(());
            })
            .expect("register scoped API initializer");
        waiting.await.expect("scoped API initializer");
    }

    async fn spawn_scoped(&'static self, context: crate::TestContext) -> String {
        let factory = self.app_factory;
        let thread_context = context.clone();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let (thread_tx, thread_rx) = tokio::sync::oneshot::channel::<std::thread::JoinHandle<()>>();
        // Register ownership before starting the thread, including if teardown races setup.
        context
            .on_shutdown(async move {
                let _ = stop_tx.send(());
                let thread = thread_rx.await?;
                tokio::task::spawn_blocking(move || thread.join())
                    .await?
                    .map_err(|_| std::io::Error::other("scoped API thread panicked"))?;
                Ok(())
            })
            .expect("register scoped API shutdown");
        let thread = std::thread::Builder::new()
            .name("aura-scoped-api".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("scoped API runtime");
                runtime.block_on(thread_context.scope(async {
                    tokio::select! {
                        _ = async {
                            let app = factory().await;
                            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                                .await
                                .expect("scoped API listener");
                            if ready_tx.send(format!(
                                "http://{}",
                                listener.local_addr().expect("API address")
                            )).is_err() {
                                return;
                            }
                            axum::serve(listener, app).await.expect("scoped API server");
                        } => {},
                        _ = stop_rx => {},
                    }
                    #[cfg(feature = "postgres")]
                    thread_context.close_postgres_pools().await;
                }));
            })
            .expect("scoped API thread");
        if let Err(thread) = thread_tx.send(thread) {
            tokio::task::spawn_blocking(move || thread.join())
                .await
                .expect("join abandoned API startup")
                .expect("abandoned API thread");
            panic!("scoped API cleanup ended before startup");
        }
        ready_rx.await.expect("scoped API startup")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    struct FactoryState {
        started: AtomicUsize,
        cancelled: AtomicBool,
    }

    struct FactoryGuard(Arc<FactoryState>);
    impl Drop for FactoryGuard {
        fn drop(&mut self) {
            self.0.cancelled.store(true, Ordering::Release);
        }
    }

    fn pending_app() -> Pin<Box<dyn Future<Output = Router> + Send>> {
        Box::pin(async {
            let state = crate::TestContext::current()
                .unwrap()
                .api_state::<Arc<FactoryState>>()
                .unwrap();
            state.started.fetch_add(1, Ordering::AcqRel);
            let _guard = FactoryGuard(state);
            std::future::pending().await
        })
    }
    static PENDING_API: AuraHistoriaApi = AuraHistoriaApi::new(pending_app);

    #[tokio::test]
    async fn should_cancel_and_join_a_scoped_api_factory_that_is_still_starting() {
        let session =
            crate::TestContext::acquire_explicit("pending API factory", crate::Isolation::Isolated)
                .await
                .unwrap();
        let state = Arc::new(FactoryState {
            started: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
        });
        session.context().insert_api_state(state.clone()).unwrap();
        let first_waiter = session
            .context()
            .spawn(async { (&PENDING_API).set_up().await })
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while state.started.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        first_waiter.abort();
        session
            .context()
            .spawn(async { (&PENDING_API).set_up().await })
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert_eq!(
            1,
            state.started.load(Ordering::Acquire),
            "cancelling a setup waiter must not start another factory"
        );
        tokio::time::timeout(std::time::Duration::from_secs(5), session.finish())
            .await
            .unwrap()
            .unwrap();
        assert!(
            state.cancelled.load(Ordering::Acquire),
            "shutdown must cancel startup before joining the API thread"
        );
        assert_eq!(
            1,
            Arc::strong_count(&state),
            "joined thread and context must release API state"
        );
    }
}
