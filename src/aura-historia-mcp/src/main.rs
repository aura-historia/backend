use aura_historia_mcp::{McpDependencies, TransportConfig, mcp_router};
use aura_historia_runtime::{ProductListingConfig, Runtime};
use platform_observability::{LoggingConfig, init_with_directives};
use platform_process::{Shutdown, config::load_file_backed_environment};

type MainError = application::error::BoxError;

fn main() -> Result<(), MainError> {
    load_file_backed_environment()?;
    // SDK trace events include protocol messages and must never enter operational logs.
    init_with_directives(LoggingConfig::from_env(), &["rmcp=off"]);
    let transport = TransportConfig::from_env()?;
    let listing_config = ProductListingConfig::from_env()?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            // Registered before startup, so a stop request during composition is not lost.
            let shutdown = Shutdown::register()?;
            let address = transport.bind_addr;
            let runtime = Runtime::from_env().await?;
            let dependencies = McpDependencies::from_env(&listing_config)?;
            let app = mcp_router(transport, &runtime, dependencies)?;
            let listener = tokio::net::TcpListener::bind(address).await?;
            tracing::info!(bind_addr = %address, "aura-historia-mcp listening");
            // Stops accepting connections, then drains in-flight requests.
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown.received())
                .await?;
            tracing::info!("aura-historia-mcp drained in-flight requests and stopped");
            Ok(())
        })
}
