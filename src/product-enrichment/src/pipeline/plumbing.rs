use crate::pipeline::{
    faucet::{EnrichmentPipeFaucet, MessageRef},
    pipe::{EnrichmentPipe, PipeProduct},
    sink::EnrichmentPipeSink,
};
use aws_sdk_sqs::types::DeleteMessageBatchRequestEntry;
use common::{
    batch::Batch,
    product_id::{ProductId, ProductKey},
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tracing::{error, info, warn};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlumbingResult {
    pub successes: u64,
    pub failures: u64,
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait EnrichmentPlumbing {
    async fn plumb(&self, count: i32) -> PlumbingResult;
}

pub struct EnrichmentPlumbingImpl {
    faucet: Arc<dyn EnrichmentPipeFaucet + Send + Sync>,
    pipes: Vec<Box<dyn EnrichmentPipe + Send + Sync>>,
    sink: Arc<dyn EnrichmentPipeSink + Send + Sync>,
    sqs_client: Arc<aws_sdk_sqs::Client>,
    enrichment_queue_url: String,
}

#[async_trait::async_trait]
impl EnrichmentPlumbing for EnrichmentPlumbingImpl {
    async fn plumb(&self, count: i32) -> PlumbingResult {
        info!(count = count, "Started plumbing.");

        // find water
        let (pipe_items, message_refs): (Vec<PipeProduct>, Vec<MessageRef>) =
            self.faucet.pour(count).await.into_iter().unzip();
        let mut message_refs = message_refs
            .into_iter()
            .map(|message_ref| (message_ref.product_id, message_ref))
            .collect::<HashMap<_, _>>();

        // run water through pipes
        let (enriched_products, mut failed_items) =
            self.pipes
                .iter()
                .fold((pipe_items, HashSet::new()), |(pipe_in, leak_in), pipe| {
                    let pipe_res = pipe.enrich(pipe_in);
                    let pipe_out = pipe_res.successes;
                    let mut pipe_leak = leak_in;
                    pipe_leak.extend(&mut pipe_res.failures.into_iter());

                    (pipe_out, pipe_leak)
                });

        // route water to sink
        let (drain_documents, drain_records) = enriched_products.into_iter().fold(
            (HashMap::new(), HashMap::new()),
            |(mut drain_documents, mut drain_records), pipe_item| {
                if let Some(document_update) = pipe_item.update.document {
                    drain_documents.insert(pipe_item.source.product_id, document_update);
                };
                if let Some(record_update) = pipe_item.update.record {
                    let product_key = ProductKey {
                        shop_id: pipe_item.source.payload.shop_id,
                        shops_product_id: pipe_item.source.payload.shops_product_id,
                    };
                    drain_records.insert(pipe_item.source.product_id, (product_key, record_update));
                };
                (drain_documents, drain_records)
            },
        );
        failed_items.extend(&mut self.sink.drain_documents(drain_documents).await.into_iter());
        failed_items.extend(&mut self.sink.drain_records(drain_records).await.into_iter());
        let failed_piped_water = failed_items.len();

        // remove leaked water - leaving behind arrived water
        for failed_item in failed_items {
            message_refs.remove(&failed_item);
        }
        let successes_pre_deletion = message_refs.len();
        let failed_deleted_water = self
            .handle_delete_messages_with_retry(message_refs.into_values().collect())
            .await;
        let failures = failed_piped_water + failed_deleted_water.len();
        let successes_post_deletion = successes_pre_deletion - failed_deleted_water.len();

        info!(
            successes = successes_post_deletion,
            failures = failures,
            "Finished plumbing."
        );
        PlumbingResult {
            successes: successes_post_deletion as u64,
            failures: failures as u64,
        }
    }
}

#[mockall::automock]
impl EnrichmentPlumbingImpl {
    pub fn new(
        faucet: Arc<dyn EnrichmentPipeFaucet + Send + Sync>,
        pipes: Vec<Box<dyn EnrichmentPipe + Send + Sync>>,
        sink: Arc<dyn EnrichmentPipeSink + Send + Sync>,
        sqs_client: Arc<aws_sdk_sqs::Client>,
        enrichment_queue_url: String,
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
    ) -> HashSet<ProductId> {
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
                    .map(|message_ref| message_ref.product_id)
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
            .queue_url(&self.enrichment_queue_url)
            .set_entries(Some(delete_message_batch_entries))
            .send()
            .await;

        match res {
            Ok(output) => {
                output.failed.into_iter().map(|failure| failure.id).filter_map(|message_id| {
                    match message_ids_message_refs.remove(&message_id) {
                        Some(message_ref) => Some(message_ref),
                        None => {
                            error!(messageId = message_id, "Failed re-collecting ProductId belonging to a failed messageId. This is a bug.");
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

#[cfg(test)]
mod tests {
    use crate::pipeline::{
        faucet::{MessageRef, MockEnrichmentPipeFaucet},
        pipe::{EnrichmentPipe, MockEnrichmentPipe, PipeProduct, PipeResult},
        plumbing::{EnrichmentPlumbing, EnrichmentPlumbingImpl},
        sink::MockEnrichmentPipeSink,
    };
    use aws_config::{BehaviorVersion, SdkConfig};
    use std::sync::Arc;

    #[tokio::test]
    async fn should_partially_fail_enrichment_in_pipe() {
        let mut faucet = MockEnrichmentPipeFaucet::default();
        let mut pipe = MockEnrichmentPipe::default();
        let mut sink = MockEnrichmentPipeSink::default();
        let sdk_config = SdkConfig::builder()
            .behavior_version(BehaviorVersion::latest())
            .build();
        let sqs_client = aws_sdk_sqs::Client::new(&sdk_config);

        let faked_poured = fake::vec![(PipeItem, MessageRef); 1000]
            .into_iter()
            .map(|(pipe_item, mut msg_ref)| {
                msg_ref.product_id = pipe_item.source.product_id;
                (pipe_item, msg_ref)
            })
            .collect();
        faucet
            .expect_pour()
            .return_once(move |_| Box::pin(async move { faked_poured }));
        pipe.expect_enrich().return_once(|items| PipeResult {
            failures: items
                .iter()
                .take(958)
                .map(|item| item.source.product_id)
                .collect(),
            successes: items.into_iter().skip(958).collect(),
        });
        sink.expect_drain_documents().return_once(|documents| {
            assert!(documents.len() <= 42); // of those succeeding some might not bring any changes
            Box::pin(async { documents.into_keys().collect() })
        });
        sink.expect_drain_records().return_once(|records| {
            assert!(records.len() <= 42);
            Box::pin(async { records.into_keys().collect() })
        });

        let pipes: Vec<Box<dyn EnrichmentPipe + Send + Sync>> = vec![Box::new(pipe)];
        let plumbing = EnrichmentPlumbingImpl::new(
            Arc::new(faucet),
            pipes,
            Arc::new(sink),
            Arc::new(sqs_client),
            "dummy".to_owned(),
        );
        let actual = plumbing.plumb(1000).await;
        assert_eq!(actual.successes, 0);
        assert_eq!(actual.failures, 1000);
    }
}
