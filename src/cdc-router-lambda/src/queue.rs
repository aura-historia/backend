//! Router-only Standard SQS publisher. Consumers and receipt handling stay outside this crate.
use std::time::{Duration, Instant};

use aws_sdk_sqs::{Client, config::Region, types::QueueAttributeName};
use aws_smithy_types::{retry::RetryConfig, timeout::TimeoutConfig};

use crate::queue_config::validate_attributes;
pub use crate::queue_config::{CdcRouterQueueConfig, QueueError, SqsQueueConfig};

const API_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct SqsQueue {
    config: SqsQueueConfig,
    client: Client,
}

impl SqsQueue {
    pub async fn new(client: Client, config: SqsQueueConfig) -> Result<Self, QueueError> {
        let client = Client::from_conf(
            client
                .config()
                .to_builder()
                .region(Region::new(config.region().to_owned()))
                .endpoint_url(config.endpoint_url())
                .endpoint_resolver(aws_sdk_sqs::config::endpoint::DefaultResolver::new())
                .use_fips(false)
                .use_dual_stack(false)
                .retry_config(RetryConfig::standard().with_max_attempts(2))
                .timeout_config(
                    TimeoutConfig::builder()
                        .connect_timeout(Duration::from_secs(3))
                        .read_timeout(Duration::from_secs(25))
                        .operation_attempt_timeout(Duration::from_secs(30))
                        .operation_timeout(Duration::from_secs(55))
                        .build(),
                )
                .build(),
        );
        let queue = Self { config, client };
        queue.validate_attributes(false).await?;
        queue.validate_attributes(true).await?;
        Ok(queue)
    }

    pub fn config(&self) -> &SqsQueueConfig {
        &self.config
    }

    async fn validate_attributes(&self, dlq: bool) -> Result<(), QueueError> {
        let url = if dlq {
            self.config.dlq_url()
        } else {
            self.config.queue_url().clone()
        };
        let response = tokio::time::timeout(
            API_TIMEOUT,
            self.client
                .get_queue_attributes()
                .queue_url(url.as_str())
                .attribute_names(QueueAttributeName::All)
                .send(),
        )
        .await
        .map_err(|_| QueueError::Timeout)?
        .map_err(|_| QueueError::Unavailable)?;
        let attributes = response.attributes.ok_or(QueueError::InvalidResponse)?;
        validate_attributes(&self.config, &attributes, dlq)
    }

    pub async fn publish(&self, body: &str) -> Result<(), QueueError> {
        struct Observation<'a> {
            scope: &'a str,
            bytes: usize,
            started: Instant,
            outcome: &'static str,
        }
        impl Drop for Observation<'_> {
            fn drop(&mut self) {
                tracing::info!(
                    scope = self.scope,
                    encoded_bytes = self.bytes,
                    publication_duration_ms = self.started.elapsed().as_secs_f64() * 1000.0,
                    outcome = self.outcome,
                    "worker SQS publication finished"
                );
            }
        }
        let mut observation = Observation {
            scope: self.config.scope().as_str(),
            bytes: body.len(),
            started: Instant::now(),
            outcome: "cancelled_acceptance_unknown",
        };
        if body.len() > aura_historia_jobs::wire::MAX_JOB_BYTES {
            observation.outcome = "rejected_size";
            return Err(QueueError::MessageTooLarge);
        }
        // A timed-out or missing SQS confirmation is never an acknowledgment.
        let result = match tokio::time::timeout(
            API_TIMEOUT,
            self.client
                .send_message()
                .queue_url(self.config.queue_url().as_str())
                .message_body(body)
                .send(),
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                observation.outcome = "failed_acceptance_unknown";
                return Err(QueueError::Unavailable);
            }
            Err(_) => {
                observation.outcome = "timeout_acceptance_unknown";
                return Err(QueueError::Timeout);
            }
        };
        if !result.message_id().is_some_and(|id| !id.is_empty()) {
            observation.outcome = "invalid_response_acceptance_unknown";
            return Err(QueueError::InvalidResponse);
        }
        observation.outcome = "published";
        Ok(())
    }
}
