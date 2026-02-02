use common::{batch::Batch, product_id::ProductId};
use product::dynamodb::{
    product_event_record::ProductEventRecord, product_record::ProductRecordSerdeField,
    repository::ProductDynamoDbRepository,
};
use std::collections::HashSet;
use tracing::error;

#[derive(Debug, Clone, Default)]
pub struct FlowOutResult {
    pub successes: HashSet<ProductId>,
    pub failures: HashSet<ProductId>,
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait PipeFlowOut {
    async fn flow_out(&self, data: Vec<ProductEventRecord>) -> FlowOutResult;
}

pub struct PipeFlowOutImpl<'a> {
    product_dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Send + Sync),
}

impl<'a> PipeFlowOutImpl<'a> {
    pub fn new(
        product_dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Send + Sync),
    ) -> Self {
        Self {
            product_dynamodb_repository,
        }
    }
}

#[async_trait::async_trait]
impl<'a> PipeFlowOut for PipeFlowOutImpl<'a> {
    async fn flow_out(&self, event_records: Vec<ProductEventRecord>) -> FlowOutResult {
        let mut successes = HashSet::with_capacity(event_records.len());
        let mut failures = HashSet::new();
        for batch in Batch::chunked_from(event_records.into_iter()) {
            let mut batch_product_ids: HashSet<ProductId> = batch
                .iter()
                .map(ProductEventRecord::product_id)
                .copied()
                .collect();
            let batch_res = self
                .product_dynamodb_repository
                .put_product_event_records(batch)
                .await;
            match batch_res {
                Ok(output) => {
                    let default_vec = vec![];
                    let batch_failure_product_ids = output
                        .unprocessed_items
                        .unwrap_or_default()
                        .get(self.product_dynamodb_repository.table())
                        .unwrap_or(&default_vec)
                        .iter()
                        .filter_map(|write_request| match write_request.put_request {
                            Some(ref put_request) => put_request
                                .item
                                .get(ProductRecordSerdeField::ProductId.as_str()),
                            None => None,
                        })
                        .filter_map(|attribute_value| attribute_value.as_s().ok())
                        .filter_map(|product_id_str| {
                            ProductId::try_from(product_id_str.as_str()).ok()
                        })
                        .collect::<HashSet<_>>();
                    for batch_failure_product_id in batch_failure_product_ids {
                        batch_product_ids.remove(&batch_failure_product_id);
                        failures.insert(batch_failure_product_id);
                    }
                    // leaves behind successes
                    successes.extend(&mut batch_product_ids.iter());
                }
                Err(err) => {
                    error!(error = ?err, "Failed writing batch of ProductEventRecords.");
                    failures.extend(&mut batch_product_ids.into_iter());
                }
            }
        }

        FlowOutResult {
            successes,
            failures,
        }
    }
}
