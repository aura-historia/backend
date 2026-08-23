mod config;
pub mod health;
pub mod jobs;
pub mod scheduled_job;
pub mod scheduler;
pub mod wiring;

use std::future::Future;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::error;

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
    let listener = match TcpListener::bind(config.health_bind_addr()).await {
        Ok(listener) => listener,
        Err(health_bind) => {
            let shutdown_result = scheduler.shutdown(config.shutdown_grace()).await;
            if let Err(scheduler_shutdown) = shutdown_result {
                error!(error = %scheduler_shutdown, "cron.scheduler.shutdown_failed_after_health_bind_failure");
                return Err(CronRuntimeError::HealthBindAndSchedulerShutdown {
                    health_bind,
                    scheduler_shutdown,
                });
            }
            return Err(CronRuntimeError::HealthBind(health_bind));
        }
    };
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
                health.set_ready(false);
                let health_error = match result {
                    Ok(Ok(())) => CronRuntimeError::HealthServerExited,
                    Ok(Err(error)) => error,
                    Err(error) => CronRuntimeError::HealthServerTask(error),
                };
                if let Err(shutdown_error) = scheduler.shutdown(config.shutdown_grace()).await {
                    return Err(CronRuntimeError::SchedulerShutdown(shutdown_error));
                }
                return Err(health_error);
            }
            () = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                if !scheduler.is_alive() {
                    health.set_ready(false);
                    server.abort();
                    let _ = server.await;
                    if let Err(shutdown_error) = scheduler.shutdown(config.shutdown_grace()).await {
                        return Err(CronRuntimeError::SchedulerShutdown(shutdown_error));
                    }
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
    #[error("failed to bind cron health listener ({health_bind}) and drain cron executions")]
    HealthBindAndSchedulerShutdown {
        health_bind: std::io::Error,
        #[source]
        scheduler_shutdown: CronSchedulerShutdownError,
    },
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
