mod config;
pub mod health;
pub mod jobs;
pub mod scheduled_job;
pub mod scheduler;
pub mod wiring;

pub use config::{
    CRON_ENABLED_JOBS_ENV, CRON_HEALTH_BIND_ADDR_ENV, CRON_SHUTDOWN_GRACE_SECONDS_ENV,
    CronRuntimeConfig, CronRuntimeConfigError,
};
pub use scheduler::{CronSchedulerShutdownError, CronSchedulerTaskExit, JobRegistration};
use std::future::Future;
use std::sync::Arc;
use tokio::net::TcpListener;

pub async fn run_until_shutdown<S>(
    config: CronRuntimeConfig,
    registrations: Vec<JobRegistration>,
    shutdown: S,
) -> Result<(), CronRuntimeError>
where
    S: Future<Output = ()>,
{
    let listener = TcpListener::bind(config.health_bind_addr())
        .await
        .map_err(CronRuntimeError::HealthBind)?;
    let health = Arc::new(health::RuntimeHealth::default());
    let server_health = Arc::clone(&health);
    let mut server = tokio::spawn(async move {
        axum::serve(listener, health::router(server_health))
            .await
            .map_err(CronRuntimeError::HealthServer)
    });
    let mut scheduler = match scheduler::CronScheduler::start(registrations).await {
        Ok(scheduler) => scheduler,
        Err(error) => {
            server.abort();
            let _ = server.await;
            return Err(CronRuntimeError::SchedulerStart(error));
        }
    };
    health.set_ready(true);
    tokio::pin!(shutdown);

    tokio::select! {
        () = &mut shutdown => {
            health.set_ready(false);
            let result = scheduler.shutdown(config.shutdown_grace()).await;
            server.abort();
            let _ = server.await;
            result.map_err(CronRuntimeError::SchedulerShutdown)
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
            Err(health_error)
        }
        scheduler_exit = scheduler.wait_for_exit() => {
            health.set_ready(false);
            server.abort();
            let _ = server.await;
            if let Err(shutdown_error) = scheduler.shutdown(config.shutdown_grace()).await {
                return Err(CronRuntimeError::SchedulerShutdown(shutdown_error));
            }
            Err(CronRuntimeError::SchedulerTask(scheduler_exit))
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
    #[error("cron scheduler stopped unexpectedly")]
    SchedulerTask(#[source] CronSchedulerTaskExit),
    #[error("failed to drain cron executions")]
    SchedulerShutdown(#[source] CronSchedulerShutdownError),
}
