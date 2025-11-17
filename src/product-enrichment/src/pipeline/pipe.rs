use common::product_id::ProductId;
use product::core::product_event::ProductCreatedEventPayload;
use product::dynamodb::product_update_record::ProductRecordUpdate;
use product::opensearch::product_update_document::ProductUpdateDocument;
use std::collections::HashSet;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone)]
pub struct PipeProduct {
    pub source: PipeProductSource,
    pub update: PipeProductUpdate,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone)]
pub struct PipeProductSource {
    pub product_id: ProductId,
    pub payload: ProductCreatedEventPayload,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Default)]
pub struct PipeProductUpdate {
    pub document: Option<ProductUpdateDocument>,
    pub record: Option<ProductRecordUpdate>,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Default)]
pub struct PipeResult {
    pub successes: Vec<PipeProduct>,
    pub failures: HashSet<ProductId>,
}

impl PipeResult {
    pub fn get_pipe_product_mut(&mut self, product_id: &ProductId) -> Option<&mut PipeProduct> {
        self.successes
            .iter()
            .position(|el| &el.source.product_id == product_id)
            .and_then(|i| self.successes.get_mut(i))
    }
}

#[mockall::automock]
pub trait EnrichmentPipe {
    fn enrich(&self, products: Vec<PipeProduct>) -> PipeResult;
}
