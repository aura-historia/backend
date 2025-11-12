use std::sync::Arc;

use crate::pipeline::pipe::{PipeItem, PipeItemSource, PipeItemUpdate};
use common::product_id::ProductId;
use product_lambda_common::extract_product_event_record;
use product::core::product_event::{ProductEvent, ProductEventPayload};
use product::dynamodb::product_event_record::ProductEventRecord;
use tracing::{error, info};

#[async_trait::async_trait]
#[mockall::automock]
pub trait EnrichmentPipeFaucet {
    async fn pour(&self, count: i32) -> Vec<(PipeItem, MessageRef)>;
}

pub struct EnrichmentPipeFaucetImpl {
    sqs_client: Arc<aws_sdk_sqs::Client>,
    enrichment_queue_url: String,
}

impl EnrichmentPipeFaucetImpl {
    pub fn new(sqs_client: Arc<aws_sdk_sqs::Client>, enrichment_queue_url: String) -> Self {
        Self {
            sqs_client,
            enrichment_queue_url,
        }
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageRef {
    pub message_id: String,
    pub receipt_handle: String,
    pub product_id: ProductId,
}

#[async_trait::async_trait]
impl EnrichmentPipeFaucet for EnrichmentPipeFaucetImpl {
    async fn pour(&self, count: i32) -> Vec<(PipeItem, MessageRef)> {
        let count = count as usize;
        let mut messages = Vec::with_capacity(count);
        loop {
            let res = self
                .sqs_client
                .receive_message()
                .queue_url(&self.enrichment_queue_url)
                .max_number_of_messages(10.min((count - messages.len()) as i32))
                .visibility_timeout(120)
                .send()
                .await;
            match res {
                Err(err) => {
                    error!(
                        error = ?err,
                        "Failed receiving messages. Aborting receiving - continuing pipe."
                    );
                    break;
                }
                Ok(output) => {
                    let mut local_messages = output.messages.unwrap_or_default();
                    let local_messages_count = local_messages.len();
                    messages.append(&mut local_messages);

                    if local_messages_count < 10 || messages.len() >= count {
                        break;
                    }
                }
            }
        }
        info!(
            count = count,
            received = messages.len(),
            "Received messages."
        );

        let mut water = Vec::with_capacity(messages.len());
        let mut failed_message_ids = Vec::new(); // ignore here as: no explicit sqs-succeed == failure
        let mut skipped_count = 0;
        for msg in messages {
            let message_id = msg.message_id.clone().expect(
                "shouldn't receive an SQS-Message without 'message_id' because AWS sets it.",
            );
            let receipt_handle = msg.receipt_handle.clone().expect(
                "shouldn't receive an SQS-Message without 'receipt_handle' because AWS sets it.",
            );
            if let Some(event_record) =
                extract_product_event_record(msg, &mut failed_message_ids, &mut skipped_count)
            {
                match ProductEvent::try_from(event_record) {
                    Ok(event) => match event.payload {
                        ProductEventPayload::Created(payload) => {
                            let message_ref = MessageRef {
                                message_id,
                                receipt_handle,
                                product_id: event.aggregate_id,
                            };
                            water.push((payload, message_ref));
                        }
                        other => {
                            error!(
                                itemId = %event.aggregate_id,
                                payload = ?other,
                                "Expected 'ProductEventPayload::Created' but got other.",
                            );
                            skipped_count += 1;
                        }
                    },
                    Err(err) => {
                        error!(
                            error = %err,
                            fromType = %std::any::type_name::<ProductEventRecord>(),
                            toType = %std::any::type_name::<ProductEvent>(),
                            "Failed mapping types. Skipping event."
                        );
                        skipped_count += 1;
                    }
                };
            }
        }

        let pipe_items: Vec<(PipeItem, MessageRef)> = water
            .into_iter()
            .map(|(payload, message_ref)| {
                let pipe_item = PipeItem {
                    source: PipeItemSource {
                        product_id: message_ref.product_id,
                        payload,
                    },
                    update: PipeItemUpdate::default(),
                };
                (pipe_item, message_ref)
            })
            .collect();

        info!(
            count = count,
            successes = pipe_items.len(),
            skipped = skipped_count,
            failures = failed_message_ids.len(),
            "Faucet poured."
        );

        pipe_items
    }
}
