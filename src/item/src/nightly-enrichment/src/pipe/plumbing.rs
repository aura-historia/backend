use crate::pipe::{
    faucet::{EnrichmentPipeFaucet, MessageRef},
    sink::EnrichmentPipeSink,
    spec::{EnrichmentPipe, PipeItem},
};
use aws_sdk_sqs::types::DeleteMessageBatchRequestEntry;
use common::{
    batch::Batch,
    item_id::{ItemId, ItemKey},
};
use std::collections::{HashMap, HashSet};
use tracing::{error, info, warn};

#[async_trait::async_trait]
#[mockall::automock]
pub trait EnrichmentPlumbing {
    async fn plumb(&self, count: i32);
}

pub struct EnrichmentPlumbingImpl<'a> {
    faucet: &'a (dyn EnrichmentPipeFaucet + Sync),
    pipes: &'a [&'a (dyn EnrichmentPipe + Sync)],
    sink: &'a (dyn EnrichmentPipeSink + Sync),
    sqs_client: &'a aws_sdk_sqs::Client,
    enrichment_queue_url: &'a str,
}

#[async_trait::async_trait]
impl<'a> EnrichmentPlumbing for EnrichmentPlumbingImpl<'a> {
    async fn plumb(&self, count: i32) {
        info!(count = count, "Started plumbing.");

        // find water
        match self.faucet.pour(count).await {
            Err(err) => {
                error!(
                    error = ?err,
                    count = count,
                    queueUrl = self.enrichment_queue_url,
                    "Failed pouring ItemEventRecords from enrichment-queue."
                );
            }
            Ok(water) => {
                let (pipe_items, message_refs): (Vec<PipeItem>, Vec<MessageRef>) =
                    water.into_iter().unzip();
                let mut message_refs = message_refs
                    .into_iter()
                    .map(|message_ref| (message_ref.item_id, message_ref))
                    .collect::<HashMap<_, _>>();

                // run water through pipes
                let (enriched_items, mut failed_items) =
                    self.pipes
                        .iter()
                        .fold((pipe_items, vec![]), |(pipe_in, leak_in), pipe| {
                            let mut pipe_res = pipe.enrich(pipe_in);
                            let pipe_out = pipe_res.successes;
                            let mut pipe_leak = leak_in;
                            pipe_leak.append(&mut pipe_res.failures);

                            (pipe_out, pipe_leak)
                        });

                // route water to sink
                let (drain_documents, drain_records) = enriched_items.into_iter().fold(
                    (HashMap::new(), HashMap::new()),
                    |(mut drain_documents, mut drain_records), pipe_item| {
                        if let Some(document_update) = pipe_item.update.document {
                            drain_documents.insert(pipe_item.source.item_id, document_update);
                        };
                        if let Some(record_update) = pipe_item.update.record {
                            let item_key = ItemKey {
                                shop_id: pipe_item.source.payload.shop_id,
                                shops_item_id: pipe_item.source.payload.shops_item_id,
                            };
                            drain_records
                                .insert(pipe_item.source.item_id, (item_key, record_update));
                        };
                        (drain_documents, drain_records)
                    },
                );
                failed_items.append(&mut self.sink.drain_documents(drain_documents).await);
                failed_items.append(&mut self.sink.drain_records(drain_records).await);
                let failed_piped_water = failed_items.len();

                // remove leaked water - leaving behind arrived water
                for failed_item in failed_items {
                    message_refs.remove(&failed_item);
                }
                let successes = message_refs.len();
                let failed_deleted_water = self
                    .handle_delete_messages_with_retry(message_refs.into_values().collect())
                    .await;
                let failures = failed_piped_water + failed_deleted_water.len();

                info!(
                    count = count,
                    successes = successes,
                    failures = failures,
                    "Finished plumbing."
                );
            }
        }
    }
}

impl<'a> EnrichmentPlumbingImpl<'a> {
    pub fn new(
        faucet: &'a (dyn EnrichmentPipeFaucet + Sync),
        pipes: &'a [&'a (dyn EnrichmentPipe + Sync)],
        sink: &'a (dyn EnrichmentPipeSink + Sync),
        sqs_client: &'a aws_sdk_sqs::Client,
        enrichment_queue_url: &'a str,
    ) -> Self {
        Self {
            faucet,
            pipes,
            sink,
            sqs_client,
            enrichment_queue_url,
        }
    }

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
