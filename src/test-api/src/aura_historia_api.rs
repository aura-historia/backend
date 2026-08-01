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

    pub fn base_url(&self) -> &str {
        match self.base_url.get() {
            Some(base_url) => base_url,
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
