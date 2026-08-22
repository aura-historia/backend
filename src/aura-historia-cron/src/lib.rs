mod config;
pub mod health;
pub mod jobs;
pub mod scheduled_job;
pub mod scheduler;
pub mod wiring;

use std::future::Future;
use std::sync::Arc;
use tokio::net::TcpListener;

pub use config::{
    CRON_ENABLED_JOBS_ENV, CRON_HEALTH_BIND_ADDR_ENV, CRON_SHUTDOWN_GRACE_SECONDS_ENV,
    CronRuntimeConfig, CronRuntimeConfigError,
};
pub use scheduler::{CronSchedulerShutdownError, JobRegistration};

pub async fn run_until_shutdown<S>(
    config: CronRuntimeConfig,
    registrations: Vec<JobRegistration>,
    shutdown: S,
) -> Result<(), CronRuntimeError>
where
    S: Future<Output = ()>,
{
    let scheduler = scheduler::CronScheduler::start(registrations)
        .await
        .map_err(CronRuntimeError::SchedulerStart)?;
    let listener = TcpListener::bind(config.health_bind_addr())
        .await
        .map_err(CronRuntimeError::HealthBind)?;
    let health = Arc::new(health::RuntimeHealth::default());
    health.set_ready(true);
    let server_health = Arc::clone(&health);
    let mut server = tokio::spawn(async move {
        axum::serve(listener, health::router(server_health))
            .await
            .map_err(CronRuntimeError::HealthServer)
    });
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            () = &mut shutdown => {
                health.set_ready(false);
                let result = scheduler.shutdown(config.shutdown_grace()).await;
                server.abort();
                let _ = server.await;
                return result.map_err(CronRuntimeError::SchedulerShutdown);
            }
            result = &mut server => {
                return match result {
                    Ok(Ok(())) => Err(CronRuntimeError::HealthServerExited),
                    Ok(Err(error)) => Err(error),
                    Err(error) => Err(CronRuntimeError::HealthServerTask(error)),
                };
            }
            () = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                if !scheduler.is_alive() {
                    health.set_ready(false);
                    server.abort();
                    let _ = server.await;
                    return Err(CronRuntimeError::SchedulerExited);
                }
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CronRuntimeError {
    #[error("failed to start cron scheduler")]
    SchedulerStart(#[source] scheduler::CronSchedulerStartError),
    #[error("failed to bind cron health listener")]
    HealthBind(#[source] std::io::Error),
    #[error("cron health server failed")]
    HealthServer(#[source] std::io::Error),
    #[error("cron health server exited unexpectedly")]
    HealthServerExited,
    #[error("cron health server task failed")]
    HealthServerTask(#[source] tokio::task::JoinError),
    #[error("cron scheduler exited unexpectedly")]
    SchedulerExited,
    #[error("failed to drain cron executions")]
    SchedulerShutdown(#[source] CronSchedulerShutdownError),
}
