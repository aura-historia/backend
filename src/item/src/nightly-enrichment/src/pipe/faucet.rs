use crate::pipe::spec::{PipeItem, PipeItemSource, PipeItemUpdate};
use aws_sdk_sqs::{
    error::SdkError, operation::receive_message::ReceiveMessageError,
    types::DeleteMessageBatchRequestEntry,
};
use common::{batch::Batch, item_id::ItemId};
use item_core::item_event::{ItemEvent, ItemEventPayload};
use item_dynamodb::item_event_record::ItemEventRecord;
use item_lambda_common::extract_item_event_record;
use std::collections::{HashMap, HashSet};
use tracing::{error, warn};

#[async_trait::async_trait]
#[mockall::automock]
pub trait EnrichmentPipeFaucet {
    async fn pour(&self, count: i32) -> Result<Vec<PipeItem>, SdkError<ReceiveMessageError>>;
}

pub struct EnrichmentPipeFaucetImpl<'a> {
    sqs_client: &'a aws_sdk_sqs::Client,
    enrichment_queue_url: &'a str,
}

impl<'a> EnrichmentPipeFaucetImpl<'a> {
    pub fn new(sqs_client: &'a aws_sdk_sqs::Client, enrichment_queue_url: &'a str) -> Self {
        Self {
            sqs_client,
            enrichment_queue_url,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MessageRef {
    message_id: String,
    receipt_handle: String,
    item_id: ItemId,
}

#[async_trait::async_trait]
impl<'a> EnrichmentPipeFaucet for EnrichmentPipeFaucetImpl<'a> {
    async fn pour(&self, count: i32) -> Result<Vec<PipeItem>, SdkError<ReceiveMessageError>> {
        let messages = self
            .sqs_client
            .receive_message()
            .queue_url(self.enrichment_queue_url)
            .max_number_of_messages(count)
            .visibility_timeout(count)
            .send()
            .await?
            .messages
            .unwrap_or_default();

        let mut event_payloads = Vec::with_capacity(messages.len());
        let mut successes: HashSet<MessageRef> = HashSet::with_capacity(messages.len());
        let mut failed_message_ids = Vec::new(); // ignore here as no explicit sqs-succeed == failure
        let mut skipped_count = 0;
        for msg in messages {
            let message_id = msg.message_id.clone().expect(
                "shouldn't receive an SQS-Message without 'message_id' because AWS sets it.",
            );
            let receipt_handle = msg.receipt_handle.clone().expect(
                "shouldn't receive an SQS-Message without 'receipt_handle' because AWS sets it.",
            );
            if let Some(event_record) =
                extract_item_event_record(msg, &mut failed_message_ids, &mut skipped_count)
            {
                match ItemEvent::try_from(event_record) {
                    Ok(event) => match event.payload {
                        ItemEventPayload::Created(payload) => {
                            event_payloads.push((payload, event.aggregate_id));
                            successes.insert(MessageRef {
                                message_id,
                                receipt_handle,
                                item_id: event.aggregate_id,
                            });
                        }
                        other => {
                            error!(
                                itemId = %event.aggregate_id,
                                payload = ?other,
                                "Expected 'ItemEventPayload::Created' but got other.",
                            );
                            skipped_count += 1;
                        }
                    },
                    Err(err) => {
                        error!(
                            error = %err,
                            fromType = %std::any::type_name::<ItemEventRecord>(),
                            toType = %std::any::type_name::<ItemEvent>(),
                            "Failed mapping types. Skipping event."
                        );
                        skipped_count += 1;
                    }
                };
            }
        }

        let failed_deletes = self.handle_delete_messages_with_retry(successes).await;

        let pipe_items = event_payloads
            .into_iter()
            .filter(|(_, item_id)| !failed_deletes.contains(item_id))
            .map(|(payload, item_id)| PipeItem {
                source: PipeItemSource { item_id, payload },
                update: PipeItemUpdate::default(),
            })
            .collect();

        Ok(pipe_items)
    }
}

impl<'a> EnrichmentPipeFaucetImpl<'a> {
    async fn handle_delete_messages_with_retry(
        &self,
        successes: HashSet<MessageRef>,
    ) -> HashSet<ItemId> {
        const MAX_RETRIES: u32 = 5;
        const BASE_DELAY_MS: u64 = 100;

        let mut message_refs = successes;
        let mut retry_count = 0;
        loop {
            let failed = self.handle_delete_messages(message_refs).await;
            if failed.is_empty() {
                return HashSet::default();
            }
            if retry_count >= MAX_RETRIES {
                warn!(
                    messageCount = failed.len(),
                    "Failed deleting message after '{MAX_RETRIES}' retries."
                );
                return failed
                    .into_iter()
                    .map(|message_ref| message_ref.item_id)
                    .collect();
            }

            retry_count += 1;
            let delay_ms = BASE_DELAY_MS * 2_u64.pow(retry_count - 1);
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;

            message_refs = failed;
        }
    }

    async fn handle_delete_messages(&self, successes: HashSet<MessageRef>) -> HashSet<MessageRef> {
        let mut failed_deletes = HashSet::new();
        for batch in Batch::chunked_from(successes.into_iter()) {
            failed_deletes.extend(self.handle_delete_messages_batch(batch).await);
        }

        failed_deletes
    }

    async fn handle_delete_messages_batch(
        &self,
        batch: Batch<MessageRef, 10>,
    ) -> HashSet<MessageRef> {
        let mut message_ids_message_refs: HashMap<String, MessageRef> = batch
            .iter()
            .map(|message_ref| (message_ref.message_id.clone(), message_ref.clone()))
            .collect();

        let delete_message_batch_entries = batch
            .into_iter()
            .map(|message_ref| {
                DeleteMessageBatchRequestEntry::builder()
                    .id(message_ref.message_id)
                    .receipt_handle(message_ref.receipt_handle)
                    .build()
                    .expect("shouldn't fail because we explicitly set 'id' and 'receipt_handle'")
            })
            .collect();
        let res = self
            .sqs_client
            .delete_message_batch()
            .queue_url(self.enrichment_queue_url)
            .set_entries(Some(delete_message_batch_entries))
            .send()
            .await;

        match res {
            Ok(output) => {
                output.failed.into_iter().map(|failure| failure.id).filter_map(|message_id| {
                    match message_ids_message_refs.remove(&message_id) {
                        Some(message_ref) => Some(message_ref),
                        None => {
                            error!("Failed re-collecting ItemId belonging to a failed messageId. This is a bug.");
                            None
                        },
                    }
                })
                .collect()
            }
            Err(err) => {
                error!(error = ?err, "Failed deleting entire MessageBatch.");
                message_ids_message_refs.into_values().collect()
            }
        }
    }
}
