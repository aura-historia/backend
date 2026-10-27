//! Failure custody for FIFO records that can never succeed. Moving such a record out of its
//! source queue releases its message group instead of holding it for the whole retry budget.

use aws_sdk_sqs::{
    Client, error::ProvideErrorMetadata,
    operation::send_message::builders::SendMessageFluentBuilder,
};
use lambda_runtime::Error;
use std::future::Future;

/// The source queue's own dead-letter queue, configured on each FIFO consumer.
pub const DEAD_LETTER_QUEUE_URL_ENV: &str = "DEAD_LETTER_QUEUE_URL";

/// The FIFO identity and unchanged body a record keeps in custody. The original SQS message ID
/// deduplicates a repeated transfer only within the SQS five-minute deduplication interval; a
/// redelivery after a lost acknowledgment that arrives later can leave a second custody copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParkedRecord {
    pub message_id: String,
    pub message_group_id: String,
    pub body: String,
}

/// Durable custody that operators inspect and redrive. A transfer counts only once confirmed.
pub trait FailureCustody {
    /// Resolves with the custody copy's message ID once custody is confirmed.
    fn park(
        &self,
        record: &ParkedRecord,
    ) -> impl Future<Output = Result<String, FailureCustodyError>> + Send;
}

#[derive(Debug, thiserror::Error)]
pub enum FailureCustodyError {
    /// The request failed or its outcome is unknown; the record may or may not be in custody.
    #[error("failure custody transfer is unconfirmed")]
    Unconfirmed(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("failure custody returned no message ID")]
    MissingMessageId,
}

/// Sends records to the source queue's own FIFO dead-letter queue, where the existing DLQ alarm
/// and operator redrive already apply.
#[derive(Clone, Debug)]
pub struct SqsDeadLetterCustody {
    client: Client,
    queue_url: String,
}

impl SqsDeadLetterCustody {
    pub fn new(client: Client, queue_url: String) -> Self {
        Self { client, queue_url }
    }

    /// Composition fails without a configured dead-letter queue, so a consumer never starts
    /// without the custody its dispositions rely on.
    pub async fn from_env() -> Result<Self, Error> {
        let queue_url = std::env::var(DEAD_LETTER_QUEUE_URL_ENV)
            .ok()
            .filter(|url| !url.trim().is_empty())
            .ok_or_else(|| Error::from(format!("missing required {DEAD_LETTER_QUEUE_URL_ENV}")))?;
        let config = aws_config::defaults(aws_config::BehaviorVersion::v2026_01_12())
            .load()
            .await;
        Ok(Self::new(Client::new(&config), queue_url))
    }

    fn send_request(&self, record: &ParkedRecord) -> SendMessageFluentBuilder {
        self.client
            .send_message()
            .queue_url(&self.queue_url)
            .message_body(&record.body)
            .message_group_id(&record.message_group_id)
            .message_deduplication_id(&record.message_id)
    }
}

impl FailureCustody for SqsDeadLetterCustody {
    fn park(
        &self,
        record: &ParkedRecord,
    ) -> impl Future<Output = Result<String, FailureCustodyError>> + Send {
        let request = self.send_request(record);
        // Boxed so the SDK's deeply nested request future does not overflow the layout queries
        // of every Lambda future that awaits a batch.
        Box::pin(async move {
            let output = request.send().await.map_err(|error| {
                // A bounded AWS error code tells a misconfigured queue or grant apart from a
                // transient failure; the error's message is not logged.
                tracing::warn!(
                    aws_error_code = error
                        .code()
                        .filter(|code| code.len() <= 128
                            && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.'))
                        .unwrap_or("unknown"),
                    "Failure custody send to the dead-letter queue failed"
                );
                FailureCustodyError::Unconfirmed(Box::new(error))
            })?;
            output
                .message_id
                .filter(|id| !id.is_empty())
                .ok_or(FailureCustodyError::MissingMessageId)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_keep_body_group_and_original_message_id_when_building_the_custody_request() {
        let client = Client::from_conf(
            aws_sdk_sqs::Config::builder()
                .behavior_version(aws_sdk_sqs::config::BehaviorVersion::v2026_01_12())
                .region(aws_sdk_sqs::config::Region::new("eu-central-1"))
                .build(),
        );
        let custody = SqsDeadLetterCustody::new(
            client,
            "https://sqs.eu-central-1.amazonaws.com/123456789012/source-dlq.fifo".to_owned(),
        );
        let request = custody.send_request(&ParkedRecord {
            message_id: "4c5c8a5e-0d36-4b55-9df3-36f0c1b3d6e1".to_owned(),
            message_group_id: "pli1_group".to_owned(),
            body: r#"{"unchanged":true}"#.to_owned(),
        });

        assert_eq!(
            request.get_queue_url().as_deref(),
            Some("https://sqs.eu-central-1.amazonaws.com/123456789012/source-dlq.fifo")
        );
        assert_eq!(
            request.get_message_body().as_deref(),
            Some(r#"{"unchanged":true}"#)
        );
        assert_eq!(
            request.get_message_group_id().as_deref(),
            Some("pli1_group")
        );
        assert_eq!(
            request.get_message_deduplication_id().as_deref(),
            Some("4c5c8a5e-0d36-4b55-9df3-36f0c1b3d6e1")
        );
        assert!(request.get_message_attributes().is_none());
    }
}
