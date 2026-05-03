use crate::{
    flow_in::{MessageRef, PipeFlowIn},
    flow_out::PipeFlowOut,
    process::PipeProcessor,
};
use aws_sdk_sqs::{Client, types::DeleteMessageBatchRequestEntry};
use common::{batch::Batch, product_id::ProductId};
use product::{
    dynamodb::product_event_record::ProductEventRecord, service::get_service::GetProductService,
};
use std::collections::{HashMap, HashSet};
use tracing::{error, info, warn};

#[async_trait::async_trait]
pub trait Pipe {
    async fn pipe(&self);
}

pub struct PipeImpl<'a> {
    get_product_service: &'a (dyn GetProductService + Send + Sync),
    sqs: &'a Client,
    source_queue: String,
    batch_in_count: u16,
    visibility_timeout: u16,
    flow_in: &'a (dyn PipeFlowIn + Send + Sync),
    processor: &'a (dyn PipeProcessor + Send + Sync),
    flow_out: &'a (dyn PipeFlowOut + Send + Sync),
}

impl<'a> PipeImpl<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        get_product_service: &'a (dyn GetProductService + Send + Sync),
        sqs: &'a Client,
        source_queue: String,
        batch_in_count: u16,
        visibility_timeout: u16,
        flow_in: &'a (dyn PipeFlowIn + Send + Sync),
        processor: &'a (dyn PipeProcessor + Send + Sync),
        flow_out: &'a (dyn PipeFlowOut + Send + Sync),
    ) -> Self {
        Self {
            get_product_service,
            sqs,
            source_queue,
            batch_in_count,
            visibility_timeout,
            flow_in,
            processor,
            flow_out,
        }
    }
}

#[async_trait::async_trait]
impl<'a> Pipe for PipeImpl<'a> {
    async fn pipe(&self) {
        info!("Start piping...");
        loop {
            info!("Start piping iteration...");
            let in_res = self
                .flow_in
                .flow_in(self.batch_in_count, self.visibility_timeout)
                .await;
            info!(
                batchInCount = self.batch_in_count,
                count = in_res.data.len(),
                "Products have flown in."
            );
            if in_res.data.is_empty() {
                break;
            }

            let mut message_refs = in_res
                .data
                .keys()
                .cloned()
                .map(|message_ref| (message_ref.product_id, message_ref))
                .collect::<HashMap<_, _>>();
            let get_products_res = self
                .get_product_service
                .find_products(in_res.data.into_values().collect())
                .await;
            let products = match get_products_res {
                Ok(products) => products,
                Err(err) => {
                    error!(error = %err, "Failed finding products. Continuing with no products");
                    vec![]
                }
            };

            let processed = self.processor.process(products).await;
            info!(
                successes = processed
                    .successes
                    .iter()
                    .map(|success| success.aggregate_id)
                    .collect::<HashSet<_>>()
                    .len(),
                failures = processed.failures.len(),
                "Processed products."
            );

            let outs = processed
                .successes
                .into_iter()
                .map(ProductEventRecord::from)
                .collect();
            let out_res = self.flow_out.flow_out(outs).await;
            info!(
                successes = out_res.successes.len(),
                failures = out_res.failures.len(),
                "Product-Events have flown out."
            );

            // Remove successes from message_refs - leaving beahind failures
            let mut successful_message_refs = HashSet::with_capacity(message_refs.len());
            for success in out_res.successes {
                let message_ref_opt = message_refs.remove(&success);
                match message_ref_opt {
                    None => {
                        error!(
                            productId = %success,
                            "Failed re-collecting MessageRef belonging to a failed productId. This is a bug."
                        );
                    }
                    Some(message_ref) => {
                        successful_message_refs.insert(message_ref);
                    }
                }
            }
            let mut failed_message_refs: HashSet<MessageRef> =
                message_refs.values().cloned().collect();

            // Delete successes from source-queue
            let delete_failures = self
                .handle_delete_messages_with_retry(successful_message_refs)
                .await;
            for delete_failure in delete_failures {
                let message_ref_opt = message_refs.remove(&delete_failure);
                match message_ref_opt {
                    None => {
                        error!(
                            productId = %delete_failure,
                            "Failed re-collecting MessageRef belonging to a delete-failed productId. This is a bug."
                        );
                    }
                    Some(message_ref) => {
                        failed_message_refs.insert(message_ref);
                    }
                }
            }
            if !failed_message_refs.is_empty() {
                warn!(
                    failures = failed_message_refs.len(),
                    "Finished piping iteration with failures."
                )
            } else {
                info!("Finished piping iteration without any failures.");
            }
        }
        info!("Finished piping.");
    }
}

impl<'a> PipeImpl<'a> {
    async fn handle_delete_messages_with_retry(
        &self,
        deletes: HashSet<MessageRef>,
    ) -> HashSet<ProductId> {
        const MAX_RETRIES: u32 = 5;
        const BASE_DELAY_MS: u64 = 100;

        let mut message_refs = deletes;
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
            .sqs
            .delete_message_batch()
            .queue_url(&self.source_queue)
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
