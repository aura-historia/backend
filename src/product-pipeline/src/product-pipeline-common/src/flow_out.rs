use crate::types::HasProductId;
use aws_sdk_sqs::Client;
use common::{batch::Batch, product_id::ProductId};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use tracing::{error, warn};

#[derive(Debug, Clone, Default)]
pub struct FlowOutResult {
    pub successes: HashSet<ProductId>,
    pub failures: HashSet<ProductId>,
}

#[async_trait::async_trait]
pub trait PipeFlowOut<'a, OutData: 'a + Serialize> {
    async fn flow_out(&'a self, data: Vec<OutData>) -> FlowOutResult;
}

#[derive(Debug, Clone)]
pub struct PipeFlowOutImpl<'a> {
    sqs: &'a Client,
    target_queue: String,
}

impl<'a> PipeFlowOutImpl<'a> {
    pub fn new(sqs: &'a Client, target_queue: impl Into<String>) -> PipeFlowOutImpl<'a> {
        Self {
            sqs,
            target_queue: target_queue.into(),
        }
    }
}

#[async_trait::async_trait]
impl<'a, OutData: 'a + Serialize + HasProductId + Clone + Send + Sync> PipeFlowOut<'a, OutData>
    for PipeFlowOutImpl<'a>
{
    async fn flow_out(&'a self, data: Vec<OutData>) -> FlowOutResult {
        const MAX_RETRIES: u32 = 5;
        const BASE_DELAY_MS: u64 = 100;

        let all_products = data
            .into_iter()
            .map(|datum| (datum.product_id(), datum))
            .collect::<HashMap<_, _>>();
        let mut result = FlowOutResult {
            successes: HashSet::with_capacity(all_products.len()),
            failures: HashSet::new(),
        };

        let mut products = all_products.values().cloned().collect();
        let mut retry_count = 0;
        loop {
            let res = self.handle_send_messages(products).await;
            result.successes.extend(res.successes);
            if res.failures.is_empty() {
                return result;
            }
            if retry_count >= MAX_RETRIES {
                warn!(
                    messageCount = res.failures.len(),
                    "Failed sending messages after '{MAX_RETRIES}' retries."
                );
                result.failures.extend(res.failures);
                return result;
            }

            retry_count += 1;
            let delay_ms = BASE_DELAY_MS * 2_u64.pow(retry_count - 1);
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;

            products = res.failures.into_iter().filter_map(|failure_id| {
                match all_products.get(&failure_id) {
                    Some(failure) => Some(failure.clone()),
                    None => {
                        error!(productId = %failure_id, "Failed re-collecting OutData-Payload belonging to a failed productId. This is a bug.");
                        None
                    },
                }
            }).collect();
        }
    }
}

impl<'a> PipeFlowOutImpl<'a> {
    async fn handle_send_messages<OutData: 'a + Serialize + HasProductId + Send + Sync>(
        &self,
        data: Vec<OutData>,
    ) -> FlowOutResult {
        let mut successes = HashSet::with_capacity(data.len());
        let mut failures = HashSet::new();

        for batch in Batch::chunked_from(data.into_iter()) {
            let res = self.handle_send_messages_batch(batch).await;
            successes.extend(res.successes);
            failures.extend(res.failures);
        }

        FlowOutResult {
            successes,
            failures,
        }
    }

    async fn handle_send_messages_batch<OutData: 'a + Serialize + HasProductId + Send + Sync>(
        &self,
        batch: Batch<OutData, 10>,
    ) -> FlowOutResult {
        let mut msgid_productid: HashMap<String, ProductId> = batch
            .iter()
            .enumerate()
            .map(|(i, data)| (i.to_string(), data.product_id()))
            .collect::<HashMap<_, _>>();
        let mut successes = msgid_productid.values().copied().collect::<HashSet<_>>();
        let mut failures = HashSet::new();

        let res = self
            .sqs
            .send_message_batch()
            .queue_url(&self.target_queue)
            .set_entries(Some(batch.into_sqs_message_entries()))
            .send()
            .await;

        match res {
            Ok(output) => {
                for failed in output.failed {
                    match msgid_productid.remove(failed.id()) {
                        Some(failed_productid) => {
                            failures.insert(failed_productid);
                        }
                        None => error!(
                            payload = ?failed,
                            "Couldn't find ProductId for unprocessed message. This is a bug. Not retrying."
                        ),
                    }
                }
            }
            Err(err) => {
                error!(error = ?err, "Failed writing entire OutData-Batch due to SdkError.");
                failures.extend(msgid_productid.into_values());
            }
        }

        successes.retain(|productid| !failures.contains(productid));
        FlowOutResult {
            successes,
            failures,
        }
    }
}
