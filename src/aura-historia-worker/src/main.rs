use aura_historia_worker::{compose_handlers, config::WorkerConfig, run};
use platform_observability::{LoggingConfig, init};
use platform_process::{
    Shutdown,
    config::{find_config_error, load_file_backed_environment},
    install_bounded_panic_hook,
    postgres::pool_config_from_env,
};
use std::process::ExitCode;

const COMPONENT: &str = "aura-historia-worker";

fn main() -> ExitCode {
    // Job handlers can panic on provider or persisted data; never print the payload.
    install_bounded_panic_hook(COMPONENT);
    if let Err(error) = load_file_backed_environment() {
        eprintln!("{COMPONENT}: {error}");
        return ExitCode::FAILURE;
    }
    init(LoggingConfig::from_env());
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            tracing::error!(event = "worker.startup.failed", stage = "async_runtime");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(async {
        match start().await {
            Ok(()) => ExitCode::SUCCESS,
            Err(stage) => {
                tracing::error!(event = "worker.startup.failed", stage);
                ExitCode::FAILURE
            }
        }
    })
}

/// Startup failures name only their stage; errors can carry configuration values.
async fn start() -> Result<(), &'static str> {
    let shutdown = Shutdown::register().map_err(|_| "signal_handlers")?;
    let config = WorkerConfig::from_env().map_err(|error| {
        tracing::error!(event = "worker.config.invalid", error = %error);
        "configuration"
    })?;
    let pool = pool_config_from_env(config.pool_size())
        .map_err(|_| "postgres_configuration")?
        .connect()
        .await
        .map_err(|_| "postgres_pool")?;
    let handlers = compose_handlers(&config, &pool).await.map_err(|error| {
        // Only a configuration error is named: other causes can carry provider text.
        tracing::error!(
            event = "worker.composition.failed",
            job_scope = %error.scope,
            config_error = find_config_error(&error).map(tracing::field::display),
        );
        "composition"
    })?;
    tracing::info!(event = "worker.started", scopes = handlers.len());
    run(pool, config, handlers, shutdown.into_token())
        .await
        .map_err(|_| "task_stopped")?;
    tracing::info!(event = "worker.stopped");
    Ok(())
}
