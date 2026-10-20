use aura_historia_mcp::{
    TransportConfig,
    public::{RotatingPublicRouter, public_router},
};
use aura_historia_runtime::{ProductListingConfig, RotatingRuntime, Runtime};
use platform_lambda_bootstrap::materialize_google_application_credentials_from_env;
use platform_observability::{LoggingConfig, init_with_directives};
use std::sync::Arc;

type MainError = application::error::BoxError;

fn main() -> Result<(), MainError> {
    let is_lambda = std::env::var_os("AWS_LAMBDA_RUNTIME_API").is_some();
    if is_lambda {
        // Injected ADC must be prepared before worker threads can read the environment.
        materialize_google_application_credentials_from_env()?;
    }
    // SDK trace events include protocol messages and must never enter operational logs.
    init_with_directives(LoggingConfig::default(), &["rmcp=off"]);
    let transport = TransportConfig::from_env()?;
    let listing_config = ProductListingConfig::from_env()?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            if is_lambda {
                let app = Arc::new(RotatingPublicRouter::new(
                    RotatingRuntime::from_env().await?,
                    transport,
                    listing_config,
                ));
                aura_historia_mcp::lambda::run(move || {
                    let app = Arc::clone(&app);
                    async move { app.current().await }
                })
                .await?;
            } else {
                let address = transport.bind_addr;
                let runtime = Runtime::from_env().await?;
                let app = public_router(transport, &listing_config, &runtime)?;
                let listener = tokio::net::TcpListener::bind(address).await?;
                axum::serve(listener, app)
                    .with_graceful_shutdown(shutdown())
                    .await?;
            }
            Ok(())
        })
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        if let Ok(mut terminate) = terminate {
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
