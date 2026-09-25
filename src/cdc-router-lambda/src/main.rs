use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use aws_sdk_sqs::{Client, config::Region};
use cdc_router_lambda::{
    cdc::{CdcFanout, WorkerQueueRegistry},
    kinesis,
    queue::{CdcRouterQueueConfig, SqsQueue},
};
use futures_util::future::try_join_all;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use platform_observability::{LogLevel, LoggingConfig, init};
use tracing::info;

const COMPOSITION_TIMEOUT: Duration = Duration::from_secs(15);

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(LoggingConfig::new(
        std::env::var("LOG_LEVEL")
            .ok()
            .as_deref()
            .and_then(LogLevel::parse)
            .unwrap_or_default(),
    ));

    let config = CdcRouterQueueConfig::from_env()
        .map_err(|_| Error::from("invalid CDC router queue configuration"))?;
    let fanout = Arc::new(compose_fanout(config).await?);

    info!(
        event = "lambda.initialized",
        component = "cdc-router-lambda",
        cold_start_duration_ms = initialization_started_at.elapsed().as_millis(),
    );
    run(service_fn(
        move |event: LambdaEvent<aws_lambda_events::kinesis::KinesisEvent>| {
            let fanout = Arc::clone(&fanout);
            async move {
                info!(
                    event = "lambda.invocation.started",
                    component = "cdc-router-lambda",
                    request_id = %event.context.request_id,
                    remaining_budget_ms = kinesis::invocation_budget(&event.context).as_millis(),
                );
                kinesis::handler(event, fanout.as_ref()).await
            }
        },
    ))
    .await
}

/// Initializes all ten validated destinations before Lambda begins checkpointing the stream.
/// The router intentionally has no PostgreSQL, VPC, or Secrets Manager composition.
async fn compose_fanout(config: CdcRouterQueueConfig) -> Result<CdcFanout, Error> {
    let region = Region::new(config.region().to_owned());
    let sdk = tokio::time::timeout(
        COMPOSITION_TIMEOUT,
        aws_config::defaults(aws_config::BehaviorVersion::v2026_01_12())
            .region(region)
            .load(),
    )
    .await
    .map_err(|_| Error::from("CDC router AWS SDK initialization timed out"))?;
    let client = Client::new(&sdk);
    let queues = tokio::time::timeout(
        COMPOSITION_TIMEOUT,
        try_join_all(
            config
                .into_queues()
                .into_iter()
                .map(|config| SqsQueue::new(client.clone(), config)),
        ),
    )
    .await
    .map_err(|_| Error::from("CDC router queue validation timed out"))?
    .map_err(|_| Error::from("CDC router queue validation failed"))?;
    let registry = WorkerQueueRegistry::with_all_sqs_queues(queues)
        .map_err(|_| Error::from("CDC router destination registry is invalid"))?;
    Ok(CdcFanout::new(registry))
}
