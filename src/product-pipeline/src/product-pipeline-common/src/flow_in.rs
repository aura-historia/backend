use aws_sdk_sqs::Client;
use common::{
    dynamodb_stream::extract_sqs_event_bridge_dynamodb_record,
    has_key::HasKey,
    product_id::{ProductId, ProductKey},
};
use product::dynamodb::product_event_record::ProductEventRecord;
use std::collections::HashMap;
use tracing::error;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageRef {
    pub message_id: String,
    pub receipt_handle: String,
    pub product_id: ProductId,
}

#[derive(Debug, Clone, Default)]
pub struct FlowInResult {
    pub data: HashMap<MessageRef, ProductKey>,
    pub aborted: bool,
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait PipeFlowIn {
    async fn flow_in(&self, batch_in_count: u16, visibility_timeout: u16) -> FlowInResult;
}

#[derive(Debug, Clone)]
pub struct PipeFlowInImpl<'a> {
    sqs: &'a Client,
    source_queue: String,
}

impl<'a> PipeFlowInImpl<'a> {
    pub fn new(sqs: &'a Client, source_queue: impl Into<String>) -> PipeFlowInImpl<'a> {
        Self {
            sqs,
            source_queue: source_queue.into(),
        }
    }
}

#[async_trait::async_trait]
impl<'a> PipeFlowIn for PipeFlowInImpl<'a> {
    async fn flow_in(&self, batch_in_count: u16, visibility_timeout: u16) -> FlowInResult {
        let mut aborted = false;
        let mut messages = Vec::with_capacity(batch_in_count as usize);
        loop {
            let res = self
                .sqs
                .receive_message()
                .queue_url(&self.source_queue)
                .max_number_of_messages(10.min((batch_in_count - messages.len() as u16) as i32))
                .wait_time_seconds(10)
                .visibility_timeout(visibility_timeout as i32)
                .send()
                .await;
            match res {
                Err(err) => {
                    error!(
                        error = ?err,
                        "Failed receiving messages. Aborting receiving - continuing pipe."
                    );
                    aborted = true;
                    break;
                }
                Ok(output) => {
                    let mut local_messages = output.messages.unwrap_or_default();
                    let local_messages_count = local_messages.len();
                    messages.append(&mut local_messages);

                    if local_messages_count < 10 || messages.len() as u16 >= batch_in_count {
                        break;
                    }
                }
            }
        }

        let mut data = HashMap::with_capacity(messages.len());
        let mut failed_message_ids = Vec::new(); // ignore here as: no explicit sqs-succeed == failure
        let mut skipped_count = 0;
        for msg in messages {
            let message_id = msg.message_id.clone().expect(
                "shouldn't receive an SQS-Message without 'message_id' because AWS sets it.",
            );
            let receipt_handle = msg.receipt_handle.clone().expect(
                "shouldn't receive an SQS-Message without 'receipt_handle' because AWS sets it.",
            );
            let extracted = extract_sqs_event_bridge_dynamodb_record::<ProductEventRecord>(
                msg,
                &mut failed_message_ids,
                &mut skipped_count,
            );
            if let Some(event_record) = extracted {
                let message_ref = MessageRef {
                    message_id,
                    receipt_handle,
                    product_id: *event_record.product_id(),
                };
                data.insert(message_ref, event_record.key());
            }
        }

        FlowInResult { data, aborted }
    }
}
