use crate::types::HasProductId;
use aws_sdk_sqs::Client;
use common::product_id::ProductId;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use tracing::{error, warn};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageRef {
    pub message_id: String,
    pub receipt_handle: String,
    pub product_id: ProductId,
}

#[derive(Debug, Clone, Default)]
pub struct FlowInResult<InData> {
    pub data: HashMap<MessageRef, InData>,
    pub aborted: bool,
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait PipeFlowIn<InData: DeserializeOwned> {
    async fn flow_in(&self, batch_in_count: u16, visibility_timeout: u16) -> FlowInResult<InData>;
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
impl<'a, InData: DeserializeOwned + HasProductId> PipeFlowIn<InData> for PipeFlowInImpl<'a> {
    async fn flow_in(&self, batch_in_count: u16, visibility_timeout: u16) -> FlowInResult<InData> {
        let mut messages = Vec::with_capacity(batch_in_count as usize);
        let mut aborted = false;
        loop {
            let res = self
                .sqs
                .receive_message()
                .queue_url(&self.source_queue)
                .max_number_of_messages(10.min((batch_in_count - messages.len() as u16) as i32))
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

        let data = messages
            .into_iter()
            .filter_map(|message| {
                let message_id = message.message_id.expect(
                    "shouldn't receive an SQS-Message without 'message_id' because AWS sets it.",
                );
                let receipt_handle = message.receipt_handle.expect(
                "shouldn't receive an SQS-Message without 'receipt_handle' because AWS sets it.",
            );
                match message.body {
                    Some(body) => match serde_json::from_str::<InData>(&body) {
                        Ok(deserialized) => {
                            let message_ref = MessageRef {
                                message_id,
                                receipt_handle,
                                product_id: deserialized.product_id(),
                            };
                            Some((message_ref, deserialized))
                        }
                        Err(err) => {
                            error!(
                                error = %err,
                                messageId = message_id,
                                receiptHandle = receipt_handle,
                                type = %std::any::type_name::<InData>(),
                                "Failed deserializing message-body."
                            );
                            None
                        }
                    },
                    None => {
                        warn!(
                            messageId = message_id,
                            receiptHandle = receipt_handle,
                            "Message is missing body."
                        );
                        None
                    }
                }
            })
            .collect();
        FlowInResult { data, aborted }
    }
}
