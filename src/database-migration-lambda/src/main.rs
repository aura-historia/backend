mod bootstrap;
mod operations;

use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use operations::OperationConfig;
use platform_lambda_bootstrap::{
    log_cold_start, log_invocation_start, logging_config_from_env, required_config_from_env,
};
use platform_observability::init;
use serde_json::{Value, json};
use std::time::Instant;

#[derive(Clone)]
enum DatabaseOperation {
    Bootstrap(bootstrap::MigrationConfig),
    Inspect(OperationConfig),
    Migrate(OperationConfig),
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let started_at = Instant::now();
    init(logging_config_from_env());
    // The deployment identity fixes the operation and secret access. Payloads cannot select bootstrap.
    let operation = match required_config_from_env("DATABASE_OPERATION")?.as_str() {
        "BOOTSTRAP" => DatabaseOperation::Bootstrap(bootstrap::MigrationConfig::from_env()?),
        "INSPECT" => {
            DatabaseOperation::Inspect(OperationConfig::from_env("POSTGRES_INSPECTION_SECRET_ARN")?)
        }
        "MIGRATE" => {
            DatabaseOperation::Migrate(OperationConfig::from_env("POSTGRES_MIGRATION_SECRET_ARN")?)
        }
        _ => return Err("invalid DATABASE_OPERATION".into()),
    };
    log_cold_start("database-migration-lambda", started_at);
    run(service_fn(move |event: LambdaEvent<Value>| {
        let operation = operation.clone();
        async move {
            log_invocation_start("database-migration-lambda", &event.context);
            let response = match operation {
                DatabaseOperation::Bootstrap(config) => {
                    bootstrap::initialize_database(&config).await?;
                    json!({"status":"BOOTSTRAPPED"})
                }
                DatabaseOperation::Inspect(config) => {
                    serde_json::to_value(operations::inspect(&config).await?)?
                }
                DatabaseOperation::Migrate(config) => serde_json::to_value(
                    operations::migrate(&config, serde_json::from_value(event.payload)?).await?,
                )?,
            };
            Ok::<_, Error>(response)
        }
    }))
    .await
}
