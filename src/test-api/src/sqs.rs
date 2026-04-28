use crate::IntegrationTestService;
use crate::localstack::get_aws_config;
use async_trait::async_trait;
use aws_sdk_sqs::Client;
#[cfg(feature = "cloudformation")]
use aws_sdk_sqs::types::DeleteMessageBatchRequestEntry;
use aws_sdk_sqs::types::QueueAttributeName;
use derive_builder::Builder;
use tokio::sync::OnceCell;
use tracing::debug;

/// A lazily-initialized, globally shared SQS client for integration testing.
///
/// This `OnceCell` ensures that the client is only created once during the test lifecycle,
/// using the shared [`SdkConfig`] provided by [`get_aws_config()`].
static SQS_CLIENT: OnceCell<Client> = OnceCell::const_new();

/// Returns a shared `aws_sdk_sqs::Client` for interacting with LocalStack.
///
/// The client is initialized only once using a global `OnceCell`, and internally depends on
/// [`get_aws_config()`] for configuration (test credentials, region, LocalStack endpoint).
///
/// # Returns
///
/// A reference to a lazily-initialized `Client` instance.
pub async fn get_sqs_client() -> &'static Client {
    let client = SQS_CLIENT
        .get_or_init(|| async { Client::new(get_aws_config().await) })
        .await;
    debug!("Successfully initialized SQS-Client.");
    client
}

/// Marker type representing the SQS service in LocalStack-based tests.
///
/// Implements the [`IntegrationTestService`] trait to support lifecycle management
/// when used with the `#[localstack_test]` macro.
#[derive(Debug, Builder)]
pub struct Sqs {
    pub name: &'static str,
}

impl Sqs {
    pub fn queue_url(&self) -> String {
        format!(
            "http://sqs.eu-central-1.localhost.localstack.cloud:4566/000000000000/{}",
            self.name
        )
    }

    pub fn dead_letter_queue_url(&self) -> String {
        format!(
            "http://sqs.eu-central-1.localhost.localstack.cloud:4566/000000000000/dead-letter-{}",
            self.name
        )
    }
}

#[async_trait]
impl IntegrationTestService for Sqs {
    fn service_names(&self) -> &'static [&'static str] {
        &["sqs"]
    }

    async fn set_up(&self) {
        let sqs_client = get_sqs_client().await;

        let dead_letter_queue_url = sqs_client
            .create_queue()
            .queue_name(format!("dead-letter-{}", self.name))
            .send()
            .await
            .unwrap_or_else(|e| panic!("Failed creating DLQ '{}': {e}", self.name))
            .queue_url()
            .expect("Dead-letter queue URL not returned")
            .to_string();

        let dead_letter_queue_arn = sqs_client
            .get_queue_attributes()
            .queue_url(&dead_letter_queue_url)
            .attribute_names(QueueAttributeName::QueueArn)
            .send()
            .await
            .unwrap()
            .attributes
            .unwrap()
            .get(&QueueAttributeName::QueueArn)
            .unwrap()
            .to_string();

        let redrive_policy = serde_json::json!({
            "deadLetterTargetArn": dead_letter_queue_arn,
            "maxReceiveCount": 3
        })
        .to_string();

        let queue_url = sqs_client
            .create_queue()
            .queue_name(self.name)
            .attributes(QueueAttributeName::RedrivePolicy, redrive_policy)
            .send()
            .await
            .unwrap_or_else(|e| panic!("Failed creating SQS queue '{}': {e}", self.name))
            .queue_url()
            .expect("Queue URL not returned")
            .to_string();

        assert_eq!(
            self.queue_url(),
            queue_url,
            "Expected Queue-URL '{}' and actual differ '{queue_url}'.",
            self.queue_url()
        );

        assert_eq!(
            self.dead_letter_queue_url(),
            dead_letter_queue_url,
            "Expected Dead-Letter-Queue-URL '{}' and actual differ '{dead_letter_queue_url}'.",
            self.dead_letter_queue_url()
        );
    }

    async fn tear_down(&self) {
        let client = get_sqs_client().await;
        // Use purge_queue to remove ALL messages, including those with active visibility
        // timeouts (invisible messages that drain_queue cannot reach). In LocalStack used
        // for integration tests, the AWS-imposed 60-second cooldown between purge_queue
        // calls is not enforced, making this safe for per-test teardown.
        for queue_url in [self.queue_url(), self.dead_letter_queue_url()] {
            client
                .purge_queue()
                .queue_url(&queue_url)
                .send()
                .await
                .unwrap_or_else(|e| panic!("Failed purging SQS queue '{}': {e}", queue_url));
        }
        debug!("Purged SQS queues '{}' for test isolation", self.name);
    }
}

/// Drains all **visible** messages from each of the given SQS queue URLs using a
/// receive-and-delete loop.
///
/// Unlike `purge_queue`, this approach avoids the AWS-imposed 60-second
/// cooldown between purge calls, but it only removes currently visible messages.
/// Messages with an active visibility timeout (invisible messages) are not removed.
///
/// Prefer [`Sqs::tear_down`] for test teardown when full isolation including
/// invisible messages is required.
#[cfg(feature = "cloudformation")]
pub(crate) async fn drain_queues(queue_urls: Vec<String>) {
    for queue_url in queue_urls {
        drain_queue(&queue_url).await;
    }
}

/// Receives and deletes all currently **visible** messages from a single SQS queue.
#[cfg(feature = "cloudformation")]
async fn drain_queue(queue_url: &str) {
    let client = get_sqs_client().await;
    loop {
        let resp = client
            .receive_message()
            .queue_url(queue_url)
            .max_number_of_messages(10)
            .wait_time_seconds(0)
            .send()
            .await
            .unwrap_or_else(|e| {
                panic!("shouldn't fail receiving messages from SQS queue '{queue_url}': {e}")
            });

        let messages = resp.messages.unwrap_or_default();
        if messages.is_empty() {
            break;
        }

        let entries: Vec<DeleteMessageBatchRequestEntry> = messages
            .into_iter()
            .enumerate()
            .filter_map(|(idx, m)| {
                m.receipt_handle.map(|handle| {
                    DeleteMessageBatchRequestEntry::builder()
                        .id(idx.to_string())
                        .receipt_handle(handle)
                        .build()
                        .expect("shouldn't fail building DeleteMessageBatchRequestEntry")
                })
            })
            .collect();

        client
            .delete_message_batch()
            .queue_url(queue_url)
            .set_entries(Some(entries))
            .send()
            .await
            .unwrap_or_else(|e| {
                panic!("shouldn't fail deleting messages from SQS queue '{queue_url}': {e}")
            });
    }
    debug!("Drained SQS queue '{queue_url}'.");
}
