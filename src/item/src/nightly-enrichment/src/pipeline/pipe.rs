use std::collections::HashSet;

use common::item_id::ItemId;
use item::opensearch::item_update_document::ItemUpdateDocument;
use item_core::item_event::ItemCreatedEventPayload;
use item_dynamodb::item_update_record::ItemRecordUpdate;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone)]
pub struct PipeItem {
    pub source: PipeItemSource,
    pub update: PipeItemUpdate,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone)]
pub struct PipeItemSource {
    pub item_id: ItemId,
    pub payload: ItemCreatedEventPayload,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Default)]
pub struct PipeItemUpdate {
    pub document: Option<ItemUpdateDocument>,
    pub record: Option<ItemRecordUpdate>,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Default)]
pub struct PipeResult {
    pub successes: Vec<PipeItem>,
    pub failures: HashSet<ItemId>,
}

#[mockall::automock]
pub trait EnrichmentPipe {
    fn enrich(&self, items: Vec<PipeItem>) -> PipeResult;
}
