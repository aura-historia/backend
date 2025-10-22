use item_opensearch::item_document::ItemDocument;

pub trait EnrichmentPipe {
    type Error;

    fn enrich(&self, items: Vec<ItemDocument>) -> Result<Vec<ItemDocument>, Self::Error>;
}
