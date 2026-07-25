use aura_historia_worker::{WorkerConfig, WorkerConfigError, WorkerRunError, run_until_shutdown};

#[tokio::main]
async fn main() -> Result<(), MainError> {
    common::logging::init_logging();
    let config = WorkerConfig::from_env()?;
    run_until_shutdown(config, shutdown_signal()).await?;
    Ok(())
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to listen for shutdown signal");
    }
}

#[derive(thiserror::Error, Debug)]
enum MainError {
    #[error(transparent)]
    Config(#[from] WorkerConfigError),
    #[error(transparent)]
    Run(#[from] WorkerRunError),
}
