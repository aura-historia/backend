use common::item_id::{ItemId, ItemKey};
use item_core::item_event::ItemCreatedEventPayload;
use item_dynamodb::item_update_record::ItemRecordUpdate;
use item_opensearch::item_update_document::ItemUpdateDocument;
use std::collections::HashMap;

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
#[derive(Debug, Clone)]
pub struct PipeItemUpdate {
    pub document: Option<ItemUpdateDocument>,
    pub record: Option<ItemRecordUpdate>,
}

pub trait EnrichmentPipe {
    type Error;

    fn enrich(&self, items: Vec<PipeItem>) -> Result<Vec<PipeItem>, Self::Error>;
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait EnrichmentPipeSink {
    async fn drain_documents(&self, documents: HashMap<ItemId, ItemUpdateDocument>);

    async fn drain_records(&self, documents: HashMap<ItemKey, ItemRecordUpdate>);
}
