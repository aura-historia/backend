use common::product_id::ProductId;
use serde::Serialize;
use std::collections::HashSet;

#[derive(Debug, Clone, Default)]
pub struct FlowOutResult {
    pub successes: HashSet<ProductId>,
    pub failures: HashSet<ProductId>,
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait PipeFlowOut<OutData: Serialize> {
    async fn flow_out(&self, data: Vec<OutData>) -> FlowOutResult;
}
