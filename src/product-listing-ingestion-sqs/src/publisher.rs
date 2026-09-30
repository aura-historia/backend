//! FIFO SQS transport for service-prepared product-listing ingestion commands.
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    time::Duration,
};

use aws_sdk_sqs::{Client, error::SdkError, types::SendMessageBatchRequestEntry};
use product_listing_service::{
    ports::{ProductListingIngestionPublishError, ProductListingIngestionPublisher},
    use_cases::{
        ProductListingIngestionItemOutcome, ProductListingIngestionMessage,
        ProductListingIngestionNotAttemptedReason, ProductListingIngestionOutcome,
        ProductListingIngestionRejectionReason,
    },
};

use crate::codec;

const MAX_BATCH_ENTRIES: usize = 10;
const MAX_BATCH_BYTES: usize = 1_048_576;
const MAX_FIFO_ID_BYTES: usize = 128;
const MAX_ATTEMPTS: usize = 3;
const RETRY_BACKOFF: Duration = Duration::from_millis(25);
const DEFAULT_BUDGET: Duration = Duration::from_secs(10);

/// Reuse a configured SDK client and the URL of a FIFO queue; this adapter does not discover
/// queues or credentials. The caller must reserve any HTTP/Lambda response headroom separately.
pub struct SqsProductListingIngestionPublisher {
    client: Client,
    queue_url: String,
    budget: Duration,
}

/// An invocation-scoped view of a reusable SQS publisher. Do not retain this across requests.
pub struct InvocationProductListingIngestionPublisher<'a> {
    publisher: &'a SqsProductListingIngestionPublisher,
    deadline: tokio::time::Instant,
}

// A deadline belongs to the executing invocation future, never to the warm SDK client.
tokio::task_local! {
    static PUBLICATION_DEADLINE: tokio::time::Instant;
}

/// Scope the absolute deadline to this future. Nested scopes may shorten, but never extend it.
pub async fn with_publication_deadline<T>(
    deadline: tokio::time::Instant,
    future: impl Future<Output = T>,
) -> T {
    let deadline = PUBLICATION_DEADLINE
        .try_with(|enclosing| deadline.min(*enclosing))
        .unwrap_or(deadline);
    PUBLICATION_DEADLINE.scope(deadline, future).await
}

/// Use only at budgeted runtime boundaries. Missing scope is a configuration error, not a
/// license to start the reusable publisher's full default budget.
pub struct ScopedSqsProductListingIngestionPublisher {
    publisher: SqsProductListingIngestionPublisher,
}

impl ScopedSqsProductListingIngestionPublisher {
    pub fn new(publisher: SqsProductListingIngestionPublisher) -> Self {
        Self { publisher }
    }
}

#[async_trait::async_trait]
impl ProductListingIngestionPublisher for ScopedSqsProductListingIngestionPublisher {
    async fn publish(
        &self,
        commands: Vec<ProductListingIngestionMessage>,
    ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError> {
        let deadline = PUBLICATION_DEADLINE
            .try_with(|deadline| *deadline)
            .map_err(|_| {
                ProductListingIngestionPublishError::new(std::io::Error::other(
                    "ingestion publication deadline not installed",
                ))
            })?;
        self.publisher
            .with_deadline(deadline)
            .publish(commands)
            .await
    }
}

impl SqsProductListingIngestionPublisher {
    pub fn new(client: Client, queue_url: impl Into<String>) -> Self {
        Self {
            client,
            queue_url: queue_url.into(),
            budget: DEFAULT_BUDGET,
        }
    }

    pub fn with_budget(mut self, budget: Duration) -> Self {
        self.budget = budget;
        self
    }

    /// Create a separate view for each call, with the response headroom already subtracted.
    /// The underlying SDK client and default publisher budget remain reusable.
    pub fn with_deadline(
        &self,
        deadline: tokio::time::Instant,
    ) -> InvocationProductListingIngestionPublisher<'_> {
        InvocationProductListingIngestionPublisher {
            publisher: self,
            deadline,
        }
    }

    async fn publish_until(
        &self,
        commands: Vec<ProductListingIngestionMessage>,
        caller_deadline: Option<tokio::time::Instant>,
    ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError> {
        if commands.is_empty() {
            return Ok(Vec::new());
        }
        if !url::Url::parse(&self.queue_url).is_ok_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.query().is_none()
                && url.fragment().is_none()
                && url
                    .path_segments()
                    .and_then(|mut parts| parts.next_back())
                    .is_some_and(|name| !name.is_empty() && name.ends_with(".fifo"))
        }) {
            return Err(ProductListingIngestionPublishError::new(
                std::io::Error::other("ingestion FIFO queue URL is not configured"),
            ));
        }

        let budget_deadline = tokio::time::Instant::now() + self.budget;
        let deadline = caller_deadline
            .unwrap_or(budget_deadline)
            .min(budget_deadline);
        // Preflight all commands independently, before starting any SQS request. Invalid commands
        // do not occupy a FIFO position; their valid successors can still be sent.
        let mut outcomes = vec![None; commands.len()];
        let encoded: Vec<_> = commands
            .iter()
            .enumerate()
            .map(|(position, command)| match encode_message(command) {
                Ok(message) => match validate_message(&message) {
                    Ok(()) => Some(message),
                    Err(code) => {
                        outcomes[position] = Some(rejected(code, false));
                        None
                    }
                },
                Err(codec::CodecError::TooLarge) => {
                    outcomes[position] = Some(rejected("INVALID_MESSAGE_SIZE", false));
                    None
                }
                Err(_) => {
                    outcomes[position] = Some(rejected("ENCODING_FAILED", false));
                    None
                }
            })
            .collect();

        submit_batches(
            &SdkTransport {
                client: &self.client,
                queue_url: &self.queue_url,
            },
            &encoded,
            &mut outcomes,
            deadline,
        )
        .await;

        Ok(commands
            .into_iter()
            .enumerate()
            .map(|(position, command)| ProductListingIngestionItemOutcome {
                index: command.metadata.index,
                command_id: command.metadata.command_id,
                outcome: outcomes[position]
                    .take()
                    .unwrap_or(ProductListingIngestionOutcome::Unconfirmed),
            })
            .collect())
    }
}

#[async_trait::async_trait]
impl ProductListingIngestionPublisher for SqsProductListingIngestionPublisher {
    async fn publish(
        &self,
        commands: Vec<ProductListingIngestionMessage>,
    ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError> {
        self.publish_until(commands, None).await
    }
}

#[async_trait::async_trait]
impl ProductListingIngestionPublisher for InvocationProductListingIngestionPublisher<'_> {
    async fn publish(
        &self,
        commands: Vec<ProductListingIngestionMessage>,
    ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError> {
        self.publisher
            .publish_until(commands, Some(self.deadline))
            .await
    }
}

struct EncodedMessage {
    body: String,
    group_id: String,
    deduplication_id: String,
}

fn encode_message(
    message: &ProductListingIngestionMessage,
) -> Result<EncodedMessage, codec::CodecError> {
    let body = codec::encode(message)?;
    let envelope = codec::decode(&body)?;
    Ok(EncodedMessage {
        group_id: codec::fifo_group_id(&envelope)?,
        deduplication_id: codec::fifo_deduplication_id(&envelope)?,
        body,
    })
}

fn validate_message(message: &EncodedMessage) -> Result<(), &'static str> {
    if message.body.is_empty() || message.body.len() > MAX_BATCH_BYTES {
        return Err("INVALID_MESSAGE_SIZE");
    }
    if message.group_id.is_empty()
        || message.group_id.len() > MAX_FIFO_ID_BYTES
        || message.deduplication_id.is_empty()
        || message.deduplication_id.len() > MAX_FIFO_ID_BYTES
    {
        return Err("INVALID_FIFO_ID");
    }
    Ok(())
}

fn rejected(code: &str, retryable: bool) -> ProductListingIngestionOutcome {
    ProductListingIngestionOutcome::Rejected {
        reason: ProductListingIngestionRejectionReason::Publisher {
            code: code.to_owned(),
        },
        retryable,
    }
}

fn not_attempted(
    reason: ProductListingIngestionNotAttemptedReason,
    retryable: bool,
) -> ProductListingIngestionOutcome {
    ProductListingIngestionOutcome::NotAttempted { reason, retryable }
}

#[derive(Clone, Debug)]
struct BatchEntry {
    id: String,
    body: String,
    group_id: String,
    deduplication_id: String,
}

struct BatchSuccess {
    id: String,
    message_id: String,
}

struct BatchFailure {
    id: String,
    sender_fault: bool,
}

struct BatchResponse {
    successful: Vec<BatchSuccess>,
    failed: Vec<BatchFailure>,
}

enum SendError {
    /// A request may have reached SQS; retry only with the exact same body and deduplication ID.
    Uncertain,
    /// SDK request construction failed before any network dispatch.
    NotSent,
}

#[async_trait::async_trait]
trait BatchTransport: Send + Sync {
    async fn send(&self, entries: Vec<BatchEntry>) -> Result<BatchResponse, SendError>;
}

struct SdkTransport<'a> {
    client: &'a Client,
    queue_url: &'a str,
}

#[async_trait::async_trait]
impl BatchTransport for SdkTransport<'_> {
    async fn send(&self, entries: Vec<BatchEntry>) -> Result<BatchResponse, SendError> {
        let mut request = self.client.send_message_batch().queue_url(self.queue_url);
        for entry in entries {
            let entry = SendMessageBatchRequestEntry::builder()
                .id(entry.id)
                .message_body(entry.body)
                .message_group_id(entry.group_id)
                .message_deduplication_id(entry.deduplication_id)
                .build()
                .map_err(|_| SendError::NotSent)?;
            request = request.entries(entry);
        }
        // A transport call must be one HTTP attempt: only the outer publisher can track
        // uncertainty across retries and distinguish possible acceptance from rejection.
        let response = request
            .customize()
            .config_override(aws_sdk_sqs::config::Builder::new().retry_config(
                aws_sdk_sqs::config::retry::RetryConfig::standard().with_max_attempts(1),
            ))
            .send()
            .await
            .map_err(|error| match error {
                SdkError::ConstructionFailure(_) => SendError::NotSent,
                _ => SendError::Uncertain,
            })?;
        Ok(BatchResponse {
            successful: response
                .successful()
                .iter()
                .map(|entry| BatchSuccess {
                    id: entry.id().to_owned(),
                    message_id: entry.message_id().to_owned(),
                })
                .collect(),
            failed: response
                .failed()
                .iter()
                .map(|entry| BatchFailure {
                    id: entry.id().to_owned(),
                    sender_fault: entry.sender_fault(),
                })
                .collect(),
        })
    }
}

async fn submit_batches<T: BatchTransport>(
    transport: &T,
    encoded: &[Option<EncodedMessage>],
    outcomes: &mut [Option<ProductListingIngestionOutcome>],
    deadline: tokio::time::Instant,
) {
    let mut blocked = HashMap::new();
    loop {
        let mut batch = Vec::new();
        let mut positions = Vec::new();
        let mut seen_groups = HashSet::new();
        let mut bytes = 0;
        for (position, message) in encoded.iter().enumerate() {
            let Some(message) = message.as_ref() else {
                continue;
            };
            if outcomes[position].is_some() || blocked.contains_key(&message.group_id) {
                continue;
            }
            // Claim the group's earliest unresolved eligible item *before* capacity checks.
            // Even when this head does not fit, no successor may bypass it in this batch.
            if !seen_groups.insert(message.group_id.as_str()) {
                continue;
            }
            if batch.len() == MAX_BATCH_ENTRIES || bytes + message.body.len() > MAX_BATCH_BYTES {
                continue;
            }
            bytes += message.body.len();
            positions.push(position);
            batch.push(BatchEntry {
                id: position.to_string(),
                body: message.body.clone(),
                group_id: message.group_id.clone(),
                deduplication_id: message.deduplication_id.clone(),
            });
        }
        if batch.is_empty() {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        for (position, result) in positions
            .into_iter()
            .zip(send_batch(transport, batch, deadline).await)
        {
            let retryable = match &result {
                ProductListingIngestionOutcome::Rejected { retryable, .. } => Some(*retryable),
                ProductListingIngestionOutcome::Unconfirmed => Some(true),
                _ => None,
            };
            if let (Some(retryable), Some(message)) = (retryable, &encoded[position]) {
                // A rejected/ambiguous eligible predecessor blocks all its successors.
                blocked.insert(message.group_id.clone(), retryable);
            }
            outcomes[position] = Some(result);
        }
    }
    for (position, message) in encoded.iter().enumerate() {
        if outcomes[position].is_none() {
            outcomes[position] = Some(
                match message.as_ref().and_then(|m| blocked.get(&m.group_id)) {
                    Some(&retryable) => not_attempted(
                        ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
                        retryable,
                    ),
                    None => not_attempted(
                        ProductListingIngestionNotAttemptedReason::DeadlineExceeded,
                        true,
                    ),
                },
            );
        }
    }
}

fn unresolved_failure(uncertain: bool) -> ProductListingIngestionOutcome {
    if uncertain {
        ProductListingIngestionOutcome::Unconfirmed
    } else {
        rejected("SQS_RETRYABLE_FAILURE", true)
    }
}

// Only retry explicit entry failures or transport errors that could have lost a reply. A later
// definite failure cannot disprove an earlier possible acceptance; only a success confirms it.
// Missing/duplicate receipts are ambiguous, but are not retried automatically.
async fn send_batch<T: BatchTransport>(
    transport: &T,
    entries: Vec<BatchEntry>,
    deadline: tokio::time::Instant,
) -> Vec<ProductListingIngestionOutcome> {
    let mut results = vec![None; entries.len()];
    let mut uncertain = vec![false; entries.len()];
    let mut pending: Vec<usize> = (0..entries.len()).collect();
    for attempt in 0..MAX_ATTEMPTS {
        if tokio::time::Instant::now() >= deadline {
            for position in pending {
                results[position] = Some(if attempt == 0 {
                    not_attempted(
                        ProductListingIngestionNotAttemptedReason::DeadlineExceeded,
                        true,
                    )
                } else {
                    unresolved_failure(uncertain[position])
                });
            }
            break;
        }
        let request: Vec<_> = pending.iter().map(|&i| entries[i].clone()).collect();
        let response = tokio::time::timeout_at(deadline, transport.send(request)).await;
        let response = match response {
            Ok(Ok(response)) => response,
            Ok(Err(SendError::NotSent)) => {
                for position in pending {
                    results[position] = Some(if uncertain[position] {
                        ProductListingIngestionOutcome::Unconfirmed
                    } else {
                        rejected("SQS_NOT_SENT", false)
                    });
                }
                break;
            }
            Ok(Err(SendError::Uncertain)) if attempt + 1 < MAX_ATTEMPTS => {
                for &position in &pending {
                    uncertain[position] = true;
                }
                // Retry exactly the same entries with the same FIFO deduplication IDs.
                if !wait_for_retry(attempt, deadline).await {
                    for position in pending {
                        results[position] = Some(ProductListingIngestionOutcome::Unconfirmed);
                    }
                    break;
                }
                continue;
            }
            _ => {
                for position in pending {
                    results[position] = Some(ProductListingIngestionOutcome::Unconfirmed);
                }
                break;
            }
        };
        #[derive(Clone, Copy)]
        enum Receipt {
            Accepted,
            Failed(bool),
            Ambiguous,
        }
        let requested: HashSet<_> = pending.iter().map(|&i| entries[i].id.as_str()).collect();
        let mut receipts = HashMap::new();
        let mut unknown_id = false;
        for success in &response.successful {
            if !requested.contains(success.id.as_str()) {
                unknown_id = true;
                continue;
            }
            let receipt = if success.message_id.is_empty() {
                Receipt::Ambiguous
            } else {
                Receipt::Accepted
            };
            if receipts.insert(success.id.as_str(), receipt).is_some() {
                receipts.insert(success.id.as_str(), Receipt::Ambiguous);
            }
        }
        for failure in &response.failed {
            if !requested.contains(failure.id.as_str()) {
                unknown_id = true;
                continue;
            }
            if receipts
                .insert(failure.id.as_str(), Receipt::Failed(failure.sender_fault))
                .is_some()
            {
                receipts.insert(failure.id.as_str(), Receipt::Ambiguous);
            }
        }
        // An unexpected correlation ID invalidates the whole response; no confirmations are
        // trusted. Otherwise a missing/duplicate entry poisons only its own FIFO group.
        if unknown_id {
            for position in pending {
                results[position] = Some(ProductListingIngestionOutcome::Unconfirmed);
            }
            break;
        }
        let mut retry = Vec::new();
        for position in pending {
            let id = entries[position].id.as_str();
            match receipts.get(id) {
                Some(Receipt::Accepted) => {
                    results[position] = Some(ProductListingIngestionOutcome::Accepted)
                }
                Some(Receipt::Failed(true)) if !uncertain[position] => {
                    results[position] = Some(rejected("SQS_SENDER_FAILURE", false))
                }
                Some(Receipt::Failed(false)) if attempt + 1 < MAX_ATTEMPTS => retry.push(position),
                Some(Receipt::Failed(_)) => {
                    results[position] = Some(unresolved_failure(uncertain[position]))
                }
                _ => results[position] = Some(ProductListingIngestionOutcome::Unconfirmed),
            }
        }
        pending = retry;
        if pending.is_empty() {
            break;
        }
        if !wait_for_retry(attempt, deadline).await {
            for position in pending {
                results[position] = Some(unresolved_failure(uncertain[position]));
            }
            break;
        }
    }
    results
        .into_iter()
        .map(|result| result.unwrap_or(ProductListingIngestionOutcome::Unconfirmed))
        .collect()
}

async fn wait_for_retry(attempt: usize, deadline: tokio::time::Instant) -> bool {
    let delay = RETRY_BACKOFF * (1 << attempt);
    if tokio::time::Instant::now() + delay >= deadline {
        return false;
    }
    tokio::time::sleep(delay).await;
    tokio::time::Instant::now() < deadline
}

#[cfg(test)]
mod sdk_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, sync::Mutex};

    struct Fake {
        calls: Mutex<Vec<Vec<BatchEntry>>>,
        replies: Mutex<VecDeque<FakeReply>>,
    }

    enum FakeReply {
        Success,
        Partial(Vec<usize>, Vec<(usize, bool)>),
        Lost,
        NotSent,
        Hang,
        Malformed,
    }

    impl Fake {
        fn new(replies: Vec<FakeReply>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                replies: Mutex::new(replies.into()),
            }
        }
        fn calls(&self) -> Vec<Vec<String>> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|call| call.iter().map(|e| e.id.clone()).collect())
                .collect()
        }
    }

    #[async_trait::async_trait]
    impl BatchTransport for Fake {
        async fn send(&self, entries: Vec<BatchEntry>) -> Result<BatchResponse, SendError> {
            let reply = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected send");
            self.calls.lock().unwrap().push(entries.clone());
            match reply {
                FakeReply::Lost => Err(SendError::Uncertain),
                FakeReply::NotSent => Err(SendError::NotSent),
                FakeReply::Hang => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    Err(SendError::Uncertain)
                }
                FakeReply::Malformed => Ok(BatchResponse {
                    successful: vec![BatchSuccess {
                        id: "unknown".into(),
                        message_id: "msg".into(),
                    }],
                    failed: Vec::new(),
                }),
                FakeReply::Success => Ok(BatchResponse {
                    successful: entries
                        .iter()
                        .map(|e| BatchSuccess {
                            id: e.id.clone(),
                            message_id: "msg".into(),
                        })
                        .collect(),
                    failed: Vec::new(),
                }),
                FakeReply::Partial(success, failed) => Ok(BatchResponse {
                    successful: success
                        .into_iter()
                        .map(|i| BatchSuccess {
                            id: entries[i].id.clone(),
                            message_id: "msg".into(),
                        })
                        .collect(),
                    failed: failed
                        .into_iter()
                        .map(|(i, sender_fault)| BatchFailure {
                            id: entries[i].id.clone(),
                            sender_fault,
                        })
                        .collect(),
                }),
            }
        }
    }

    fn message(group: &str, size: usize) -> Option<EncodedMessage> {
        Some(EncodedMessage {
            body: "x".repeat(size),
            group_id: group.to_owned(),
            deduplication_id: "stable-id".to_owned(),
        })
    }

    async fn run(
        fake: &Fake,
        messages: Vec<Option<EncodedMessage>>,
        budget: Duration,
    ) -> Vec<ProductListingIngestionOutcome> {
        let mut results = vec![None; messages.len()];
        submit_batches(
            fake,
            &messages,
            &mut results,
            tokio::time::Instant::now() + budget,
        )
        .await;
        results.into_iter().map(Option::unwrap).collect()
    }

    #[tokio::test]
    async fn same_group_waits_for_acceptance_and_keeps_input_order() {
        let fake = Fake::new(vec![FakeReply::Success, FakeReply::Success]);
        let results = run(
            &fake,
            vec![message("a", 1), message("a", 1), message("b", 1)],
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(
            fake.calls(),
            vec![vec!["0".to_owned(), "2".to_owned()], vec!["1".to_owned()]]
        );
        assert!(
            results
                .iter()
                .all(|r| *r == ProductListingIngestionOutcome::Accepted)
        );
    }

    #[tokio::test]
    async fn independent_preflight_rejections_do_not_reserve_fifo_positions() {
        let fake = Fake::new(vec![FakeReply::Success]);
        let mut messages = vec![message("a", 1), message("a", 1), message("b", 1)];
        messages[0] = None;
        let mut outcomes = vec![Some(rejected("ENCODING_FAILED", false)), None, None];
        submit_batches(
            &fake,
            &messages,
            &mut outcomes,
            tokio::time::Instant::now() + Duration::from_secs(1),
        )
        .await;
        assert_eq!(fake.calls(), vec![vec!["1".to_owned(), "2".to_owned()]]);
        assert!(matches!(
            outcomes[0],
            Some(ProductListingIngestionOutcome::Rejected { .. })
        ));
    }

    #[tokio::test]
    async fn bounded_by_count_and_utf8_body_bytes() {
        let fake = Fake::new(vec![FakeReply::Success, FakeReply::Success]);
        let messages: Vec<_> = (0..11).map(|i| message(&i.to_string(), 1)).collect();
        let results = run(&fake, messages, Duration::from_secs(1)).await;
        assert_eq!(
            fake.calls().iter().map(Vec::len).collect::<Vec<_>>(),
            vec![10, 1]
        );
        assert!(
            results
                .iter()
                .all(|r| *r == ProductListingIngestionOutcome::Accepted)
        );
        let fake = Fake::new(vec![FakeReply::Success, FakeReply::Success]);
        let results = run(
            &fake,
            vec![message("a", MAX_BATCH_BYTES), message("b", 1)],
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(
            fake.calls().iter().map(Vec::len).collect::<Vec<_>>(),
            vec![1, 1]
        );
        assert!(
            results
                .iter()
                .all(|r| *r == ProductListingIngestionOutcome::Accepted)
        );
        assert_eq!(
            validate_message(&message("a", MAX_BATCH_BYTES + 1).unwrap()),
            Err("INVALID_MESSAGE_SIZE")
        );
        let mut multi_byte = message("a", 1).unwrap();
        multi_byte.body = "é".repeat(MAX_BATCH_BYTES / 2 + 1);
        assert_eq!(validate_message(&multi_byte), Err("INVALID_MESSAGE_SIZE"));
        multi_byte.body = "valid".into();
        multi_byte.group_id = "x".repeat(MAX_FIFO_ID_BYTES + 1);
        assert_eq!(validate_message(&multi_byte), Err("INVALID_FIFO_ID"));
    }

    #[tokio::test]
    async fn retries_only_definite_retryable_failures_without_advancing_group() {
        let fake = Fake::new(vec![
            FakeReply::Partial(vec![1], vec![(0, false)]),
            FakeReply::Success,
            FakeReply::Success,
        ]);
        let results = run(
            &fake,
            vec![message("a", 1), message("a", 1), message("b", 1)],
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(
            fake.calls(),
            vec![
                vec!["0".to_owned(), "2".to_owned()],
                vec!["0".to_owned()],
                vec!["1".to_owned()]
            ]
        );
        assert!(
            results
                .iter()
                .all(|r| *r == ProductListingIngestionOutcome::Accepted)
        );
    }

    #[tokio::test]
    async fn sender_failure_blocks_only_its_group() {
        let fake = Fake::new(vec![FakeReply::Partial(vec![1], vec![(0, true)])]);
        let results = run(
            &fake,
            vec![message("a", 1), message("a", 1), message("b", 1)],
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(fake.calls().len(), 1);
        assert!(matches!(
            results[0],
            ProductListingIngestionOutcome::Rejected {
                retryable: false,
                ..
            }
        ));
        assert_eq!(
            results[1],
            not_attempted(
                ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
                false
            )
        );
        assert_eq!(results[2], ProductListingIngestionOutcome::Accepted);
    }

    #[tokio::test]
    async fn exhausted_definite_failure_is_retryable_but_not_accepted() {
        let fake = Fake::new(vec![
            FakeReply::Partial(vec![1], vec![(0, false)]),
            FakeReply::Partial(vec![], vec![(0, false)]),
            FakeReply::Partial(vec![], vec![(0, false)]),
        ]);
        let results = run(
            &fake,
            vec![message("a", 1), message("a", 1), message("b", 1)],
            Duration::from_secs(1),
        )
        .await;
        assert!(matches!(
            results[0],
            ProductListingIngestionOutcome::Rejected {
                retryable: true,
                ..
            }
        ));
        assert_eq!(
            results[1],
            not_attempted(
                ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
                true
            )
        );
        assert_eq!(results[2], ProductListingIngestionOutcome::Accepted);
    }

    #[tokio::test]
    async fn malformed_confirmation_never_retries_or_advances() {
        let fake = Fake::new(vec![FakeReply::Malformed]);
        let results = run(
            &fake,
            vec![message("a", 1), message("a", 1)],
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(fake.calls().len(), 1);
        assert_eq!(results[0], ProductListingIngestionOutcome::Unconfirmed);
        assert_eq!(
            results[1],
            not_attempted(
                ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
                true
            )
        );
    }

    #[tokio::test]
    async fn missing_receipt_blocks_its_group_but_preserves_other_confirmations() {
        let fake = Fake::new(vec![
            FakeReply::Partial(vec![1], vec![]),
            FakeReply::Success,
        ]);
        let results = run(
            &fake,
            vec![
                message("a", 1),
                message("a", 1),
                message("b", 1),
                message("b", 1),
            ],
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(
            fake.calls(),
            vec![vec!["0".to_owned(), "2".to_owned()], vec!["3".to_owned()]]
        );
        assert_eq!(results[0], ProductListingIngestionOutcome::Unconfirmed);
        assert_eq!(
            results[1],
            not_attempted(
                ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
                true
            )
        );
        assert_eq!(results[2], ProductListingIngestionOutcome::Accepted);
        assert_eq!(results[3], ProductListingIngestionOutcome::Accepted);
    }

    #[tokio::test]
    async fn duplicate_receipt_is_ambiguous_only_for_that_entry() {
        let fake = Fake::new(vec![FakeReply::Partial(vec![0, 0, 1], vec![])]);
        let results = run(
            &fake,
            vec![message("a", 1), message("a", 1), message("b", 1)],
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(fake.calls().len(), 1);
        assert_eq!(results[0], ProductListingIngestionOutcome::Unconfirmed);
        assert_eq!(
            results[1],
            not_attempted(
                ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
                true
            )
        );
        assert_eq!(results[2], ProductListingIngestionOutcome::Accepted);
    }

    #[tokio::test]
    async fn byte_limit_cannot_skip_b1_in_favor_of_b2() {
        let fake = Fake::new(vec![
            FakeReply::Success,
            FakeReply::Partial(vec![1], vec![(0, true)]),
        ]);
        let results = run(
            &fake,
            vec![
                message("a", MAX_BATCH_BYTES),
                message("b", 2),
                message("a", 1),
                message("b", 1),
            ],
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(
            fake.calls(),
            vec![vec!["0".to_owned()], vec!["1".to_owned(), "2".to_owned()]]
        );
        assert_eq!(results[0], ProductListingIngestionOutcome::Accepted);
        assert!(matches!(
            results[1],
            ProductListingIngestionOutcome::Rejected {
                retryable: false,
                ..
            }
        ));
        assert_eq!(results[2], ProductListingIngestionOutcome::Accepted);
        assert_eq!(
            results[3],
            not_attempted(
                ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
                false
            )
        );
    }

    #[tokio::test]
    async fn count_limit_cannot_skip_predecessor_or_lose_items() {
        let fake = Fake::new(vec![
            FakeReply::Success,
            FakeReply::Partial(vec![], vec![(0, true)]),
        ]);
        let mut messages: Vec<_> = (0..10).map(|i| message(&format!("other-{i}"), 1)).collect();
        messages.extend([message("b", 1), message("b", 1)]);
        let results = run(&fake, messages, Duration::from_secs(1)).await;
        assert_eq!(
            fake.calls().iter().map(Vec::len).collect::<Vec<_>>(),
            vec![10, 1]
        );
        assert_eq!(fake.calls()[1], vec!["10".to_owned()]);
        assert!(
            results[..10]
                .iter()
                .all(|r| *r == ProductListingIngestionOutcome::Accepted)
        );
        assert!(matches!(
            results[10],
            ProductListingIngestionOutcome::Rejected {
                retryable: false,
                ..
            }
        ));
        assert_eq!(
            results[11],
            not_attempted(
                ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
                false
            )
        );
    }

    #[tokio::test]
    async fn uncertain_then_rejected_stays_unconfirmed_even_for_sender_fault() {
        for sender_fault in [true, false] {
            let mut replies = vec![
                FakeReply::Lost,
                FakeReply::Partial(vec![], vec![(0, sender_fault)]),
            ];
            if !sender_fault {
                replies.push(FakeReply::Partial(vec![], vec![(0, false)]));
            }
            let fake = Fake::new(replies);
            let results = run(
                &fake,
                vec![message("a", 1), message("a", 1)],
                Duration::from_secs(1),
            )
            .await;
            assert_eq!(fake.calls().len(), if sender_fault { 2 } else { 3 });
            assert!(fake.calls().iter().all(|call| call == &["0".to_owned()]));
            assert!(
                fake.calls
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|call| { call[0].deduplication_id == "stable-id" && call[0].body == "x" })
            );
            assert_eq!(results[0], ProductListingIngestionOutcome::Unconfirmed);
            assert_eq!(
                results[1],
                not_attempted(
                    ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
                    true
                )
            );
        }
    }

    #[tokio::test]
    async fn uncertain_then_accepted_resolves_and_unblocks_successor() {
        let fake = Fake::new(vec![
            FakeReply::Lost,
            FakeReply::Success,
            FakeReply::Success,
        ]);
        let results = run(
            &fake,
            vec![message("a", 1), message("a", 1)],
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(
            fake.calls(),
            vec![
                vec!["0".to_owned()],
                vec!["0".to_owned()],
                vec!["1".to_owned()]
            ]
        );
        assert_eq!(results, vec![ProductListingIngestionOutcome::Accepted; 2]);
    }

    #[tokio::test]
    async fn ambiguous_retries_are_bounded_and_construction_failure_is_not_retried() {
        let fake = Fake::new(vec![FakeReply::Lost, FakeReply::Lost, FakeReply::Lost]);
        let results = run(
            &fake,
            vec![message("a", 1), message("a", 1)],
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(fake.calls().len(), MAX_ATTEMPTS);
        assert_eq!(results[0], ProductListingIngestionOutcome::Unconfirmed);
        assert_eq!(
            results[1],
            not_attempted(
                ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
                true
            )
        );

        let fake = Fake::new(vec![FakeReply::NotSent]);
        let results = run(&fake, vec![message("a", 1)], Duration::from_secs(1)).await;
        assert_eq!(fake.calls().len(), 1);
        assert!(matches!(
            results[0],
            ProductListingIngestionOutcome::Rejected {
                retryable: false,
                ..
            }
        ));

        let fake = Fake::new(vec![FakeReply::Lost, FakeReply::NotSent]);
        let results = run(&fake, vec![message("a", 1)], Duration::from_secs(1)).await;
        assert_eq!(fake.calls().len(), 2);
        assert_eq!(results[0], ProductListingIngestionOutcome::Unconfirmed);
    }

    #[tokio::test]
    async fn hundred_items_return_hundred_outcomes_and_preserve_group_order() {
        let fake = Fake::new((0..10).map(|_| FakeReply::Success).collect());
        let messages = (0..100)
            .map(|i| message(&format!("group-{}", i % 10), 1))
            .collect();
        let results = run(&fake, messages, Duration::from_secs(2)).await;
        let calls = fake.calls();
        assert_eq!(calls.len(), 10);
        for (wave, call) in calls.iter().enumerate() {
            assert_eq!(
                *call,
                (wave * 10..wave * 10 + 10)
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
            );
        }
        assert_eq!(results, vec![ProductListingIngestionOutcome::Accepted; 100]);
    }

    #[tokio::test]
    async fn retry_backoff_yields_to_deadline_without_reclassifying_uncertainty() {
        let fake = Fake::new(vec![FakeReply::Lost]);
        let results = run(
            &fake,
            vec![message("a", 1), message("a", 1)],
            Duration::from_millis(10),
        )
        .await;
        assert_eq!(fake.calls().len(), 1);
        assert_eq!(results[0], ProductListingIngestionOutcome::Unconfirmed);
        assert_eq!(
            results[1],
            not_attempted(
                ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
                true
            )
        );
        let fake = Fake::new(vec![FakeReply::Partial(vec![], vec![(0, false)])]);
        let results = run(&fake, vec![message("a", 1)], Duration::from_millis(10)).await;
        assert_eq!(fake.calls().len(), 1);
        assert!(matches!(
            results[0],
            ProductListingIngestionOutcome::Rejected {
                retryable: true,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn deadline_distinguishes_in_flight_from_never_attempted() {
        let fake = Fake::new(vec![FakeReply::Hang]);
        let results = run(
            &fake,
            vec![message("a", 1), message("a", 1), message("b", 1)],
            Duration::from_millis(5),
        )
        .await;
        assert_eq!(results[0], ProductListingIngestionOutcome::Unconfirmed);
        assert_eq!(results[2], ProductListingIngestionOutcome::Unconfirmed);
        assert_eq!(
            results[1],
            not_attempted(
                ProductListingIngestionNotAttemptedReason::BlockedByFifoPredecessor,
                true
            )
        );
        let fake = Fake::new(vec![]);
        let results = run(&fake, vec![message("a", 1)], Duration::ZERO).await;
        assert_eq!(
            results[0],
            not_attempted(
                ProductListingIngestionNotAttemptedReason::DeadlineExceeded,
                true
            )
        );
        assert!(fake.calls().is_empty());
    }
}
