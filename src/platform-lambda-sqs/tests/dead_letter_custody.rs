use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
use aws_sdk_sqs::{
    Client,
    types::{Message, MessageSystemAttributeName},
};
use lambda_runtime::{Context, LambdaEvent};
use platform_lambda_sqs::{
    FifoDisposition, RecordAttempt, RecordOutcome, SqsDeadLetterCustody, process_fifo_batch,
};
use std::time::Duration;
use test_api::{IntegrationTestService, WorkerSqs, aura_integration_test, get_sqs_client};

// A production-shaped FIFO pair: five receives before the native redrive.
const QUEUES: WorkerSqs = WorkerSqs::new("marketing-consent-sync", 330);
const GROUP: &str = "listing-a";

#[aura_integration_test(services = [QUEUES], isolated = true)]
async fn should_park_a_poisoned_head_in_its_fifo_dlq_and_complete_its_co_batched_successor() {
    let sqs = get_sqs_client().await;
    for (body, deduplication_id) in [("poison", "head"), ("ok", "successor")] {
        sqs.send_message()
            .queue_url(QUEUES.queue_url())
            .message_body(body)
            .message_group_id(GROUP)
            .message_deduplication_id(deduplication_id)
            .send()
            .await
            .unwrap();
    }
    let custody = SqsDeadLetterCustody::new(sqs.clone(), QUEUES.dead_letter_queue_url());

    let first = receive(&sqs, &QUEUES.queue_url()).await;
    assert_eq!(bodies(&first), ["poison", "ok"]);
    assert_eq!(settle(&sqs, &custody, &first).await, ["poison", "ok"]);
    assert!(
        receive(&sqs, &QUEUES.dead_letter_queue_url())
            .await
            .is_empty()
    );

    let redelivered = receive(&sqs, &QUEUES.queue_url()).await;
    assert_eq!(bodies(&redelivered), ["poison", "ok"]);
    assert!(settle(&sqs, &custody, &redelivered).await.is_empty());

    let parked = receive(&sqs, &QUEUES.dead_letter_queue_url()).await;
    assert_eq!(bodies(&parked), ["poison"]);
    assert_eq!(
        parked[0]
            .attributes()
            .and_then(|attributes| attributes.get(&MessageSystemAttributeName::MessageGroupId))
            .map(String::as_str),
        Some(GROUP)
    );
    assert!(receive(&sqs, &QUEUES.queue_url()).await.is_empty());
}

async fn receive(sqs: &Client, queue_url: &str) -> Vec<Message> {
    sqs.receive_message()
        .queue_url(queue_url)
        .max_number_of_messages(10)
        .message_system_attribute_names(MessageSystemAttributeName::All)
        .visibility_timeout(330)
        .wait_time_seconds(1)
        .send()
        .await
        .unwrap()
        .messages
        .unwrap_or_default()
}

fn bodies(messages: &[Message]) -> Vec<&str> {
    messages.iter().filter_map(Message::body).collect()
}

/// Runs the batch as Lambda would, then applies the partial response: acknowledged records are
/// deleted, and retained ones become visible again as if their visibility timeout had expired.
/// Returns the bodies of the retained records.
async fn settle(sqs: &Client, custody: &SqsDeadLetterCustody, messages: &[Message]) -> Vec<String> {
    let response = process_fifo_batch(
        lambda_event(messages),
        Duration::from_secs(30),
        Duration::from_secs(10),
        custody,
        |body| async move { body },
        park_poison,
    )
    .await
    .unwrap()
    .response;
    let mut retained = Vec::new();
    for message in messages {
        let receipt_handle = message.receipt_handle().unwrap();
        if response
            .batch_item_failures
            .iter()
            .any(|failure| Some(failure.item_identifier.as_str()) == message.message_id())
        {
            sqs.change_message_visibility()
                .queue_url(QUEUES.queue_url())
                .receipt_handle(receipt_handle)
                .visibility_timeout(0)
                .send()
                .await
                .unwrap();
            retained.push(message.body().unwrap().to_owned());
        } else {
            sqs.delete_message()
                .queue_url(QUEUES.queue_url())
                .receipt_handle(receipt_handle)
                .send()
                .await
                .unwrap();
        }
    }
    retained
}

fn park_poison(attempt: &RecordAttempt<String>) -> FifoDisposition {
    match &attempt.outcome {
        RecordOutcome::Completed(body) if body == "poison" => FifoDisposition::Park,
        RecordOutcome::Completed(body) if body == "ok" => FifoDisposition::Complete,
        _ => FifoDisposition::Retain,
    }
}

fn lambda_event(messages: &[Message]) -> LambdaEvent<SqsEvent> {
    let mut event = SqsEvent::default();
    event.records = messages
        .iter()
        .map(|message| {
            let mut record = SqsMessage::default();
            record.message_id = message.message_id.clone();
            record.receipt_handle = message.receipt_handle.clone();
            record.body = message.body.clone();
            record.attributes = message
                .attributes()
                .into_iter()
                .flatten()
                .map(|(name, value)| (name.as_str().to_owned(), value.clone()))
                .collect();
            record
        })
        .collect();
    LambdaEvent::new(event, Context::default())
}
