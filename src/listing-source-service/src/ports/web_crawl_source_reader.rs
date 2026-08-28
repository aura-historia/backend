use listing_source_core::ListingSourceId;
use party_core::party_id::PartyId;

use super::ListingSourceReadError;

#[derive(Debug, Clone, PartialEq)]
pub struct WebCrawlSource {
    pub listing_source_id: ListingSourceId,
    pub operator_party_id: PartyId,
    pub url: url::Url,
}

#[async_trait::async_trait]
pub trait WebCrawlSourceReader: Send + Sync {
    async fn list_sources(&self) -> Result<Vec<WebCrawlSource>, ListingSourceReadError>;
}
