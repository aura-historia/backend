use crate::{
    flow_in::{MessageRef, PipeFlowIn},
    flow_out::PipeFlowOut,
    process::PipeProcessor,
};
use aws_sdk_sqs::{
    Client,
    types::{ChangeMessageVisibilityBatchRequestEntry, DeleteMessageBatchRequestEntry},
};
use common::{batch::Batch, product_id::ProductId};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::{HashMap, HashSet};
use tracing::{error, info, warn};

#[async_trait::async_trait]
pub trait Pipe {
    async fn pipe(&self);
}

pub struct PipeImpl<'a, InData, In, Out, OutData> {
    sqs: &'a Client,
    source_queue: String,
    batch_in_count: u16,
    visibility_timeout: u16,
    flow_in: &'a (dyn PipeFlowIn<InData> + Send + Sync),
    processor: &'a (dyn PipeProcessor<In, Out> + Send + Sync),
    flow_out: &'a (dyn PipeFlowOut<'a, OutData> + Send + Sync),
}

impl<'a, InData, In, Out, OutData> PipeImpl<'a, InData, In, Out, OutData> {
    pub fn new(
        sqs: &'a Client,
        source_queue: String,
        batch_in_count: u16,
        visibility_timeout: u16,
        flow_in: &'a (dyn PipeFlowIn<InData> + Send + Sync),
        processor: &'a (dyn PipeProcessor<In, Out> + Send + Sync),
        flow_out: &'a (dyn PipeFlowOut<'a, OutData> + Send + Sync),
    ) -> Self {
        Self {
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
impl<'a, InData, In, Out, OutData> Pipe for PipeImpl<'a, InData, In, Out, OutData>
where
    InData: DeserializeOwned + Send + Sync,
    In: From<InData> + Send + Sync,
    Out: Send + Sync,
    OutData: From<Out> + Serialize + Send + Sync,
{
    async fn pipe(&self) {
        info!("Start piping...");
        loop {
            info!("Start piping iteration...");
            let in_res = self
                .flow_in
                .flow_in(self.batch_in_count, self.visibility_timeout)
                .await;
            if in_res.aborted {
                warn!(
                    batchInCount = self.batch_in_count,
                    count = in_res.data.len(),
                    "Aborted products flowing in."
                );
            }
            if in_res.data.is_empty() && !in_res.aborted {
                break;
            }
            info!(
                batchInCount = self.batch_in_count,
                count = in_res.data.len(),
                "Products have flown in."
            );

            let mut message_refs = in_res
                .data
                .keys()
                .cloned()
                .map(|message_ref| (message_ref.product_id, message_ref))
                .collect::<HashMap<_, _>>();
            let ins = in_res.data.into_values().map(In::from).collect();
            let processed = self.processor.process(ins);
            info!(
                successes = processed.successes.len(),
                failures = processed.failures.len(),
                "Processed products."
            );

            let outs = processed.successes.into_iter().map(OutData::from).collect();
            let out_res = self.flow_out.flow_out(outs).await;
            info!(
                successes = out_res.successes.len(),
                failures = out_res.failures.len(),
                "Products have flown out."
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

            // Set visibilityTimeout=0 for all failures
            let unrecoverable_failures = self
                .handle_reset_messages_with_retry(failed_message_refs)
                .await;
            if !unrecoverable_failures.is_empty() {
                warn!(
                    unrecoverable = unrecoverable_failures.len(),
                    "Finished piping iteration with unrecoverable failures."
                )
            } else {
                info!(
                    unrecoverable = unrecoverable_failures.len(),
                    "Finished piping iteration without any failures."
                );
            }
        }
        info!("Finished piping products.");
    }
}

impl<'a, InData, In, Out, OutData> PipeImpl<'a, InData, In, Out, OutData> {
    async fn handle_reset_messages_with_retry(
        &self,
        resets: HashSet<MessageRef>,
    ) -> HashSet<ProductId> {
        const MAX_RETRIES: u32 = 3;
        const BASE_DELAY_MS: u64 = 100;

        let mut message_refs = resets;
        let mut retry_count = 0;
        loop {
            let failed = self.handle_reset_messages(message_refs).await;
            if failed.is_empty() {
                return HashSet::default();
            }
            if retry_count >= MAX_RETRIES {
                warn!(
                    messageCount = failed.len(),
                    "Failed setting message-visibility to 0 after '{MAX_RETRIES}' retries."
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

    async fn handle_reset_messages(&self, successes: HashSet<MessageRef>) -> HashSet<MessageRef> {
        let mut failed_resets = HashSet::new();
        for batch in Batch::chunked_from(successes.into_iter()) {
            failed_resets.extend(self.handle_reset_messages_batch(batch).await);
        }

        failed_resets
    }

    async fn handle_reset_messages_batch(
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
                ChangeMessageVisibilityBatchRequestEntry::builder()
                    .id(message_ref.message_id)
                    .receipt_handle(message_ref.receipt_handle)
                    .visibility_timeout(0)
                    .build()
                    .expect("shouldn't fail because we explicitly set 'id' and 'receipt_handle'")
            })
            .collect();
        let res = self
            .sqs
            .change_message_visibility_batch()
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
                error!(error = ?err, "Failed changing visibility to 0 for entire MessageBatch.");
                message_ids_message_refs.into_values().collect()
            }
        }
    }

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
