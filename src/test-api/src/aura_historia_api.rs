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
        server.get_or_init(|| self.spawn_scoped(context)).await;
    }

    async fn spawn_scoped(&'static self, context: crate::TestContext) -> String {
        let factory = self.app_factory;
        let thread_context = context.clone();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let thread = std::thread::Builder::new()
            .name("aura-scoped-api".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("scoped API runtime");
                runtime.block_on(thread_context.scope(async {
                    let app = factory().await;
                    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                        .await
                        .expect("scoped API listener");
                    ready_tx
                        .send(format!(
                            "http://{}",
                            listener.local_addr().expect("API address")
                        ))
                        .expect("API test still waiting");
                    tokio::select! {
                        result = axum::serve(listener,app) => result.expect("scoped API server"),
                        _ = stop_rx => {},
                    }
                    #[cfg(feature = "postgres")]
                    thread_context.close_postgres_pools().await;
                }));
            })
            .expect("scoped API thread");
        context
            .on_shutdown(async move {
                let _ = stop_tx.send(());
                tokio::task::spawn_blocking(move || thread.join())
                    .await?
                    .map_err(|_| std::io::Error::other("scoped API thread panicked"))?;
                Ok(())
            })
            .expect("register scoped API shutdown");
        ready_rx.await.expect("scoped API startup")
    }
}
