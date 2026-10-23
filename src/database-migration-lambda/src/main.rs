mod bootstrap;

use bootstrap::{MigrationConfig, initialize_database};
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_lambda_bootstrap::{log_cold_start, log_invocation_start, logging_config_from_env};
use platform_observability::init;
use serde::Serialize;
use serde_json::Value;
use std::time::Instant;
use tracing::info;

#[derive(Serialize)]
struct InitializationResult {
    status: &'static str,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let config = MigrationConfig::from_env()?;
    log_cold_start("database-migration-lambda", initialization_started_at);

    run(service_fn(move |event: LambdaEvent<Value>| {
        let config = config.clone();
        async move {
            log_invocation_start("database-migration-lambda", &event.context);
            initialize_database(&config)
                .await
                .map_err(|error| -> Error { Box::new(error) })?;
            info!(
                event = "database.initialization.completed",
                "Database initialization completed"
            );
            Ok::<_, Error>(InitializationResult { status: "ready" })
        }
    }))
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_return_only_safe_initialization_result() {
        let response = match serde_json::to_string(&InitializationResult { status: "ready" }) {
            Ok(response) => response,
            Err(error) => panic!("failed to serialize initialization result: {error}"),
        };

        assert_eq!(response, r#"{"status":"ready"}"#);
        assert!(!response.contains("password"));
        assert!(!response.contains("secret"));
    }
}
