use aura_historia_mcp::{AnonymousGate, Registry, TransportConfig, router};
use platform_observability::{LoggingConfig, init_with_directives};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // SDK trace events include protocol messages; private arguments/results must
    // never enter operational logs, even if another component enables tracing.
    init_with_directives(LoggingConfig::default(), &["rmcp=off"]);
    let config = TransportConfig::from_env()?;
    let address = config.bind_addr;
    let app = router(config, Registry::default(), Arc::new(AnonymousGate))?;
    if std::env::var_os("AWS_LAMBDA_RUNTIME_API").is_some() {
        aura_historia_mcp::lambda::run(app).await?;
    } else {
        let listener = tokio::net::TcpListener::bind(address).await?;
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown())
            .await?;
    }
    Ok(())
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
