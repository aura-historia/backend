use std::collections::HashSet;

use common::product_id::ProductId;
use product::core::product_event::ItemCreatedEventPayload;
use product::dynamodb::product_update_record::ProductRecordUpdate;
use product::opensearch::product_update_document::ProductUpdateDocument;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone)]
pub struct PipeItem {
    pub source: PipeItemSource,
    pub update: PipeItemUpdate,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone)]
pub struct PipeItemSource {
    pub product_id: ProductId,
    pub payload: ItemCreatedEventPayload,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Default)]
pub struct PipeItemUpdate {
    pub document: Option<ProductUpdateDocument>,
    pub record: Option<ProductRecordUpdate>,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Default)]
pub struct PipeResult {
    pub successes: Vec<PipeItem>,
    pub failures: HashSet<ProductId>,
}

#[mockall::automock]
pub trait EnrichmentPipe {
    fn enrich(&self, items: Vec<PipeItem>) -> PipeResult;
}
