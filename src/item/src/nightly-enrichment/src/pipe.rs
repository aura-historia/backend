use item_opensearch::item_document::ItemDocument;
use std::error::Error;

pub trait EnrichmentPipe {
    fn enrich(&self, items: Vec<ItemDocument>) -> Result<Vec<ItemDocument>, Box<dyn Error>>;
}
