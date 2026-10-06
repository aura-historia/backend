//! Router-only SQS publisher. Consumers and receipt handling stay outside this crate.
use std::time::{Duration, Instant};

use aura_historia_jobs::{PreparedJob, WorkerQueueType};
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

    fn send_message_request(
        &self,
        job: &PreparedJob,
    ) -> Result<aws_sdk_sqs::operation::send_message::builders::SendMessageFluentBuilder, QueueError>
    {
        if job.scope() != self.config.scope() {
            return Err(QueueError::PreparedJob);
        }
        let request = self
            .client
            .send_message()
            .queue_url(self.config.queue_url().as_str())
            .message_body(job.body());
        match (self.config.queue_type(), job.fifo_message_attributes()) {
            (WorkerQueueType::Standard, None) => Ok(request),
            (WorkerQueueType::Fifo, Some(attributes)) => Ok(request
                .message_group_id(attributes.message_group_id())
                .message_deduplication_id(attributes.message_deduplication_id())),
            _ => Err(QueueError::PreparedJob),
        }
    }

    pub async fn publish(&self, job: &PreparedJob) -> Result<(), QueueError> {
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
            bytes: job.body().len(),
            started: Instant::now(),
            outcome: "cancelled_acceptance_unknown",
        };
        if job.body().len() > aura_historia_jobs::wire::MAX_JOB_BYTES {
            observation.outcome = "rejected_size";
            return Err(QueueError::MessageTooLarge);
        }
        let request = match self.send_message_request(job) {
            Ok(request) => request,
            Err(error) => {
                observation.outcome = "rejected_job_contract";
                return Err(error);
            }
        };
        // A timed-out or missing SQS confirmation is never an acknowledgment.
        let result = match tokio::time::timeout(API_TIMEOUT, request.send()).await {
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

#[cfg(test)]
mod tests {
    use super::*;
    use aura_historia_jobs::{WorkerScope, jobs::SearchFilterOperation, wire};

    fn queue(scope: WorkerScope) -> SqsQueue {
        let fifo = if scope.queue_type() == WorkerQueueType::Fifo {
            ".fifo"
        } else {
            ""
        };
        let config = SqsQueueConfig::new(
            scope,
            format!(
                "https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-{}-prod{fifo}",
                scope.as_str()
            )
            .parse()
            .unwrap(),
            "eu-central-1".into(),
            "prod".into(),
            None,
        )
        .unwrap();
        let client = Client::from_conf(
            aws_sdk_sqs::Config::builder()
                .behavior_version(aws_sdk_sqs::config::BehaviorVersion::v2026_01_12())
                .region(Region::new("eu-central-1"))
                .build(),
        );
        SqsQueue { config, client }
    }

    fn prepared_job(scope: WorkerScope) -> PreparedJob {
        let body = match scope {
            WorkerScope::MarketingConsentSync => {
                r#"{"schema_version":2,"scope":"marketing-consent-sync","idempotency_key":"marketing-consent:mci_01h455vb4pex5vy7enb1p677vn","ordering_key":"marketing-email:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","job_type":"MARKETING_CONSENT_SYNC_INTENT_CREATED","payload":{"marketing_consent_sync_intent_id":"mci_01h455vb4pex5vy7enb1p677vn"}}"#
            }
            WorkerScope::NotificationDelivery => {
                r#"{"schema_version":2,"scope":"notification-delivery","idempotency_key":"notification-delivery:nd_01h455vb4pex5vy7enb1p677vn","ordering_key":"notification-delivery:nd_01h455vb4pex5vy7enb1p677vn","job_type":"NOTIFICATION_DELIVERY_CREATED","payload":{"notification_delivery_id":"nd_01h455vb4pex5vy7enb1p677vn"}}"#
            }
            _ => panic!("test scope is not configured"),
        };
        let job = wire::decode::<SearchFilterOperation>(body, scope).unwrap();
        wire::prepare(&job).unwrap()
    }

    #[test]
    fn publisher_sets_both_fifo_fields_and_leaves_standard_fields_unset() {
        let consent_request = queue(WorkerScope::MarketingConsentSync)
            .send_message_request(&prepared_job(WorkerScope::MarketingConsentSync))
            .unwrap();
        assert_eq!(
            Some(
                "marketing-email:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            consent_request.get_message_group_id().as_deref()
        );
        assert_eq!(
            Some("marketing-consent:mci_01h455vb4pex5vy7enb1p677vn"),
            consent_request.get_message_deduplication_id().as_deref()
        );

        let standard_request = queue(WorkerScope::NotificationDelivery)
            .send_message_request(&prepared_job(WorkerScope::NotificationDelivery))
            .unwrap();
        assert_eq!(None, standard_request.get_message_group_id().as_deref());
        assert_eq!(
            None,
            standard_request.get_message_deduplication_id().as_deref()
        );
    }
}
