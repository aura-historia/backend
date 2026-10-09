use application::{
    error::BoxError,
    pagination::{Cursor, CursoredResult},
};
use auction_core::{
    AuctionFormat, AuctionId, AuctionKey, AuctionName, AuctionReportedStatus, AuctionSchedule,
    ReportedCatalogueLotCount, SourceAuctionId,
};
use domain_primitives::sort::SortOrder;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use time::OffsetDateTime;
use url::Url;

use super::AuctionStorageVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdminAuctionSort {
    Name,
    Created,
    #[default]
    Updated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminAuctionSearchScope {
    pub query: Option<String>,
    pub listing_source_id: Option<ListingSourceId>,
    pub source_auction_id: Option<SourceAuctionId>,
    pub format: Option<AuctionFormat>,
    pub reported_status: Option<AuctionReportedStatus>,
    pub sort: AdminAuctionSort,
    pub order: SortOrder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminAuctionSearchCursor {
    pub auction_id: AuctionId,
    pub sort_name: Option<String>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
    pub scope: AdminAuctionSearchScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminAuctionSearchRequest {
    pub scope: AdminAuctionSearchScope,
    pub cursor: Cursor<AdminAuctionSearchCursor>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdminAuctionSearchItem {
    pub auction_id: AuctionId,
    pub key: AuctionKey,
    pub name: Option<Localized<Language, AuctionName>>,
    pub catalogue_url: Option<Url>,
    pub format: Option<AuctionFormat>,
    pub schedule: AuctionSchedule,
    pub reported_status: Option<AuctionReportedStatus>,
    pub reported_lot_count: Option<ReportedCatalogueLotCount>,
    pub version: AuctionStorageVersion,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

pub type AdminAuctionSearchReadResult =
    CursoredResult<AdminAuctionSearchItem, AdminAuctionSearchCursor>;

#[derive(Debug, thiserror::Error)]
pub enum AdminAuctionSearchReadError {
    #[error("Auction admin search query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("Auction admin search read model is invalid")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AdminAuctionSearchReader: Send + Sync {
    async fn search(
        &self,
        request: &AdminAuctionSearchRequest,
    ) -> Result<AdminAuctionSearchReadResult, AdminAuctionSearchReadError>;
}
