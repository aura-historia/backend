//! Real SQS SDK serialization, HTTP retry, and publisher custody tests. No AWS calls are made.
use super::*;
use aws_sdk_sqs::config::{Credentials, Region, retry::RetryConfig};
use aws_smithy_runtime_api::{
    client::{
        http::{
            HttpClient, HttpConnector, HttpConnectorFuture, HttpConnectorSettings,
            SharedHttpConnector,
        },
        orchestrator::{HttpRequest, HttpResponse},
        result::ConnectorError,
        runtime_components::RuntimeComponents,
    },
    shared::IntoShared,
};
use aws_smithy_types::body::SdkBody;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

const SQS_ENDPOINT: &str = "https://sqs.us-east-1.amazonaws.com/";
const QUEUE_URL: &str = "https://sqs.us-east-1.amazonaws.com/123456789012/ingestion.fifo";
const FIXTURES: &str = include_str!("../../tests/fixtures/ingestion_v1.json");

// The connector is below SdkTransport: requests are signed/serialized and responses decoded by
// the real SDK. A connector I/O failure is ambiguous even if a later SQS entry says senderFault.
#[derive(Clone, Debug)]
struct ReplayHttp(Arc<Mutex<ReplayState>>);

#[derive(Debug)]
struct ReplayState {
    replies: VecDeque<Reply>,
    requests: Vec<Value>,
}

#[derive(Debug)]
enum Reply {
    IoFailure,
    Batch(Value),
    Hang,
}

impl ReplayHttp {
    fn new(replies: impl IntoIterator<Item = Reply>) -> Self {
        Self(Arc::new(Mutex::new(ReplayState {
            replies: replies.into_iter().collect(),
            requests: Vec::new(),
        })))
    }

    fn requests(&self) -> Vec<Value> {
        self.0.lock().unwrap().requests.clone()
    }

    fn assert_consumed(&self) {
        assert!(
            self.0.lock().unwrap().replies.is_empty(),
            "unused HTTP replay responses"
        );
    }
}

impl HttpConnector for ReplayHttp {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        assert_eq!(request.method(), "POST");
        assert_eq!(request.uri(), SQS_ENDPOINT);
        assert_eq!(
            request.headers().get("x-amz-target"),
            Some("AmazonSQS.SendMessageBatch")
        );
        assert_eq!(
            request.headers().get("content-type"),
            Some("application/x-amz-json-1.0")
        );
        assert!(request.headers().get("authorization").is_some());
        let body = request
            .body()
            .bytes()
            .expect("SQS request body is replayable");
        let body: Value = serde_json::from_slice(body).expect("SQS JSON protocol request");
        let mut state = self.0.lock().unwrap();
        assert_eq!(body["QueueUrl"], QUEUE_URL);
        assert!((1..=MAX_BATCH_ENTRIES).contains(&body["Entries"].as_array().unwrap().len()));
        state.requests.push(body);
        let reply = state
            .replies
            .pop_front()
            .expect("unexpected extra HTTP request");
        drop(state);
        match reply {
            Reply::IoFailure => HttpConnectorFuture::ready(Err(ConnectorError::io(
                std::io::Error::other("lost SQS HTTP reply").into(),
            ))),
            Reply::Batch(body) => {
                let response: HttpResponse = http::Response::builder()
                    .status(200)
                    .header("content-type", "application/x-amz-json-1.0")
                    .body(SdkBody::from(body.to_string()))
                    .unwrap()
                    .try_into()
                    .unwrap();
                HttpConnectorFuture::ready(Ok(response))
            }
            Reply::Hang => HttpConnectorFuture::new(async {
                std::future::pending::<Result<HttpResponse, ConnectorError>>().await
            }),
        }
    }
}

impl HttpClient for ReplayHttp {
    fn http_connector(
        &self,
        _: &HttpConnectorSettings,
        _: &RuntimeComponents,
    ) -> SharedHttpConnector {
        self.clone().into_shared()
    }
}

fn publisher(http: ReplayHttp) -> SqsProductListingIngestionPublisher {
    // Deliberately allow SDK retries on the base client. The operation-local override in
    // SdkTransport must prevent an invisible retry inside a single publisher attempt.
    let config = aws_sdk_sqs::Config::builder()
        .behavior_version_latest()
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new("test", "test", None, None, "sdk-test"))
        .http_client(http)
        .retry_config(RetryConfig::standard().with_max_attempts(3))
        .build();
    SqsProductListingIngestionPublisher::new(Client::from_conf(config), QUEUE_URL)
}

fn commands(indices: &[usize]) -> Vec<ProductListingIngestionMessage> {
    let fixtures: Vec<Value> = serde_json::from_str(FIXTURES).unwrap();
    indices
        .iter()
        .map(|&index| {
            codec::decode(&fixtures[index].to_string())
                .unwrap()
                .into_service_envelope()
                .unwrap()
                .message
        })
        .collect()
}

fn success(id: &str) -> Value {
    json!({"Id": id, "MessageId": format!("message-{id}"), "MD5OfMessageBody": "0123456789abcdef0123456789abcdef"})
}

fn failure(id: &str, sender_fault: bool) -> Value {
    json!({"Id": id, "SenderFault": sender_fault, "Code": "InvalidMessageContents", "Message": "rejected"})
}

fn entry(request: &Value, id: &str) -> Value {
    request["Entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["Id"] == id)
        .unwrap_or_else(|| panic!("missing SQS entry {id}"))
        .clone()
}

fn assert_wire_entry(request: &Value, id: &str, message: &ProductListingIngestionMessage) {
    let actual = entry(request, id);
    let body = actual["MessageBody"].as_str().expect("string message body");
    let envelope = codec::decode(body).expect("valid signed-off ingestion envelope");
    assert_eq!(
        envelope.clone().into_service_envelope().unwrap().message,
        *message
    );
    assert_eq!(
        actual["MessageGroupId"],
        codec::fifo_group_id(&envelope).unwrap()
    );
    assert_eq!(
        actual["MessageDeduplicationId"],
        codec::fifo_deduplication_id(&envelope).unwrap()
    );
    assert_eq!(actual.as_object().unwrap().len(), 4);
}

#[tokio::test]
async fn sdk_retry_must_not_turn_ambiguous_send_into_definite_sender_rejection() {
    let http = ReplayHttp::new([
        Reply::IoFailure,
        Reply::Batch(json!({"Successful": [success("2")], "Failed": [failure("0", true)]})),
    ]);
    let commands = commands(&[0, 1, 4]); // 0 and 1 share a group; raw capture 4 is independent.
    let outcomes = publisher(http.clone())
        .with_budget(Duration::from_secs(2))
        .publish(commands.clone())
        .await
        .unwrap();
    assert_eq!(
        outcomes.iter().map(|o| o.index).collect::<Vec<_>>(),
        [0, 1, 4]
    );
    for (outcome, command) in outcomes.iter().zip(&commands) {
        assert_eq!(outcome.command_id, command.metadata.command_id);
    }
    assert_eq!(
        outcomes[0].outcome,
        ProductListingIngestionOutcome::Unconfirmed
    );
    assert_eq!(
        outcomes[1].outcome,
        not_attempted(
            ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
            true
        )
    );
    assert_eq!(
        outcomes[2].outcome,
        ProductListingIngestionOutcome::Accepted
    );
    let requests = http.requests();
    assert_eq!(
        requests.len(),
        2,
        "publisher attempts, not hidden SDK retries"
    );
    for request in &requests {
        assert_eq!(request["Entries"].as_array().unwrap().len(), 2);
        assert_wire_entry(request, "0", &commands[0]);
        assert_wire_entry(request, "2", &commands[2]);
        assert!(
            request["Entries"]
                .as_array()
                .unwrap()
                .iter()
                .all(|e| e["Id"] != "1")
        );
    }
    assert_eq!(
        requests[0]["Entries"], requests[1]["Entries"],
        "retry must preserve exact body, group and dedup IDs"
    );
    assert_ne!(
        entry(&requests[0], "0")["MessageGroupId"],
        entry(&requests[0], "2")["MessageGroupId"]
    );
    http.assert_consumed();
}

#[tokio::test]
async fn sdk_receipts_gate_same_group_successors_and_retry_only_failed_entries() {
    let http = ReplayHttp::new([
        Reply::Batch(json!({"Successful": [success("2")], "Failed": [failure("0", false)]})),
        Reply::Batch(json!({"Successful": [success("0")], "Failed": []})),
        Reply::Batch(json!({"Successful": [success("1")], "Failed": []})),
    ]);
    let commands = commands(&[0, 1, 4]);
    let outcomes = publisher(http.clone())
        .with_budget(Duration::from_secs(2))
        .publish(commands.clone())
        .await
        .unwrap();
    assert!(
        outcomes
            .iter()
            .all(|o| o.outcome == ProductListingIngestionOutcome::Accepted)
    );
    let requests = http.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0]["Entries"].as_array().unwrap().len(), 2);
    assert_wire_entry(&requests[0], "0", &commands[0]);
    assert_wire_entry(&requests[0], "2", &commands[2]);
    assert_eq!(requests[1]["Entries"], json!([entry(&requests[0], "0")]));
    assert_wire_entry(&requests[2], "1", &commands[1]);
    assert_eq!(requests[2]["Entries"].as_array().unwrap().len(), 1);
    assert_eq!(
        entry(&requests[0], "0")["MessageGroupId"],
        entry(&requests[2], "1")["MessageGroupId"]
    );
    http.assert_consumed();
}

#[tokio::test]
async fn sdk_definite_sender_fault_rejects_only_its_fifo_group() {
    let http = ReplayHttp::new([Reply::Batch(json!({
        "Successful": [success("2")], "Failed": [failure("0", true)]
    }))]);
    let outcomes = publisher(http.clone())
        .with_budget(Duration::from_secs(2))
        .publish(commands(&[0, 1, 4]))
        .await
        .unwrap();
    assert_eq!(outcomes[0].outcome, rejected("SQS_SENDER_FAILURE", false));
    assert_eq!(
        outcomes[1].outcome,
        not_attempted(
            ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
            false
        )
    );
    assert_eq!(
        outcomes[2].outcome,
        ProductListingIngestionOutcome::Accepted
    );
    assert_eq!(http.requests().len(), 1);
    http.assert_consumed();
}

#[tokio::test]
async fn sdk_transport_failures_are_bounded_to_publisher_attempts() {
    let http = ReplayHttp::new([Reply::IoFailure, Reply::IoFailure, Reply::IoFailure]);
    let outcomes = publisher(http.clone())
        .with_budget(Duration::from_secs(2))
        .publish(commands(&[0, 1]))
        .await
        .unwrap();
    assert_eq!(
        outcomes[0].outcome,
        ProductListingIngestionOutcome::Unconfirmed
    );
    assert_eq!(
        outcomes[1].outcome,
        not_attempted(
            ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
            true
        )
    );
    let requests = http.requests();
    assert_eq!(requests.len(), MAX_ATTEMPTS);
    assert!(
        requests
            .iter()
            .all(|request| request["Entries"] == requests[0]["Entries"])
    );
    http.assert_consumed();
}

#[tokio::test]
async fn sdk_uncertain_then_retryable_failure_exhaustion_remains_unconfirmed() {
    let http = ReplayHttp::new([
        Reply::IoFailure,
        Reply::Batch(json!({"Successful": [], "Failed": [failure("0", false)]})),
        Reply::Batch(json!({"Successful": [], "Failed": [failure("0", false)]})),
    ]);
    let commands = commands(&[0, 1]);
    let outcomes = publisher(http.clone())
        .with_budget(Duration::from_secs(2))
        .publish(commands.clone())
        .await
        .unwrap();
    assert_eq!(
        outcomes[0].outcome,
        ProductListingIngestionOutcome::Unconfirmed
    );
    assert_eq!(
        outcomes[1].outcome,
        not_attempted(
            ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
            true
        )
    );
    let requests = http.requests();
    assert_eq!(requests.len(), MAX_ATTEMPTS);
    for request in &requests {
        assert_eq!(request["Entries"].as_array().unwrap().len(), 1);
        assert_wire_entry(request, "0", &commands[0]);
    }
    assert!(
        requests
            .iter()
            .all(|request| request["Entries"] == requests[0]["Entries"])
    );
    http.assert_consumed();
}

#[tokio::test]
async fn sdk_uncertain_then_success_confirms_and_unblocks_next_fifo_entry() {
    let http = ReplayHttp::new([
        Reply::IoFailure,
        Reply::Batch(json!({"Successful": [success("0")], "Failed": []})),
        Reply::Batch(json!({"Successful": [success("1")], "Failed": []})),
    ]);
    let commands = commands(&[0, 1]);
    let outcomes = publisher(http.clone())
        .with_budget(Duration::from_secs(2))
        .publish(commands.clone())
        .await
        .unwrap();
    assert!(
        outcomes
            .iter()
            .all(|o| o.outcome == ProductListingIngestionOutcome::Accepted)
    );
    let requests = http.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0]["Entries"], requests[1]["Entries"]);
    assert_wire_entry(&requests[0], "0", &commands[0]);
    assert_wire_entry(&requests[2], "1", &commands[1]);
    assert_eq!(requests[2]["Entries"].as_array().unwrap().len(), 1);
    assert_eq!(
        entry(&requests[0], "0")["MessageGroupId"],
        entry(&requests[2], "1")["MessageGroupId"]
    );
    assert_ne!(
        entry(&requests[0], "0")["MessageDeduplicationId"],
        entry(&requests[2], "1")["MessageDeduplicationId"]
    );
    http.assert_consumed();
}

#[tokio::test]
async fn sdk_invalid_endpoint_is_conservatively_unconfirmed_without_http_dispatch() {
    let http = ReplayHttp::new([]);
    let config = aws_sdk_sqs::Config::builder()
        .behavior_version_latest()
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new("test", "test", None, None, "sdk-test"))
        .http_client(http.clone())
        .endpoint_url("not a valid URL")
        .retry_config(RetryConfig::standard().with_max_attempts(3))
        .build();
    // The SDK reports this endpoint-resolution error as an ambiguous error rather than a
    // ConstructionFailure. The publisher must not turn it into a definite SQS rejection.
    let outcomes = SqsProductListingIngestionPublisher::new(Client::from_conf(config), QUEUE_URL)
        .with_budget(Duration::from_secs(2))
        .publish(commands(&[0, 1]))
        .await
        .unwrap();
    assert_eq!(
        outcomes[0].outcome,
        ProductListingIngestionOutcome::Unconfirmed
    );
    assert_eq!(
        outcomes[1].outcome,
        not_attempted(
            ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
            true
        )
    );
    assert!(
        http.requests().is_empty(),
        "endpoint resolution cannot reach HTTP"
    );
    http.assert_consumed();
}

#[tokio::test]
async fn sdk_expired_invocation_does_not_poison_reusable_publisher_or_client() {
    let http = ReplayHttp::new([Reply::Batch(json!({
        "Successful": [success("0")], "Failed": []
    }))]);
    let base = publisher(http.clone()).with_budget(Duration::from_secs(2));
    let commands = commands(&[0]);

    let expired = base.with_deadline(tokio::time::Instant::now() - Duration::from_millis(1));
    let first = expired.publish(commands.clone()).await.unwrap();
    assert_eq!(first[0].index, commands[0].metadata.index);
    assert_eq!(first[0].command_id, commands[0].metadata.command_id);
    assert_eq!(
        first[0].outcome,
        not_attempted(
            ProductListingIngestionNotAttemptedReason::DeadlineExceeded,
            true
        )
    );
    assert!(
        http.requests().is_empty(),
        "expired invocation must not dispatch"
    );

    let fresh = base.with_deadline(tokio::time::Instant::now() + Duration::from_secs(1));
    let second = fresh.publish(commands.clone()).await.unwrap();
    assert_eq!(second[0].index, commands[0].metadata.index);
    assert_eq!(second[0].command_id, commands[0].metadata.command_id);
    assert_eq!(second[0].outcome, ProductListingIngestionOutcome::Accepted);
    let requests = http.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["Entries"].as_array().unwrap().len(), 1);
    assert_wire_entry(&requests[0], "0", &commands[0]);
    http.assert_consumed();
}

#[tokio::test]
async fn sdk_deadline_caps_in_flight_request_and_never_sends_successor() {
    let http = ReplayHttp::new([Reply::Hang]);
    let base = publisher(http.clone()).with_budget(Duration::from_secs(2));
    let invocation = base.with_deadline(tokio::time::Instant::now() + Duration::from_millis(150));
    let outcomes = tokio::time::timeout(
        Duration::from_millis(750),
        invocation.publish(commands(&[0, 1])),
    )
    .await
    .expect("caller headroom must win over the two-second publisher budget")
    .unwrap();
    assert_eq!(
        outcomes[0].outcome,
        ProductListingIngestionOutcome::Unconfirmed
    );
    assert_eq!(
        outcomes[1].outcome,
        not_attempted(
            ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
            true
        )
    );
    assert_eq!(http.requests().len(), 1);
    http.assert_consumed();

    let http = ReplayHttp::new([]);
    let outcomes = publisher(http.clone())
        .with_budget(Duration::ZERO)
        .publish(commands(&[0]))
        .await
        .unwrap();
    assert_eq!(
        outcomes[0].outcome,
        not_attempted(
            ProductListingIngestionNotAttemptedReason::DeadlineExceeded,
            true
        )
    );
    assert!(http.requests().is_empty());
}
