use crate::core::{
    partner_status::ShopPartnerStatus, shop_search::ShopSearch, shop_type::ShopType,
};
use common::domain::Domain;
use common::operation_context::OperationContext;
use common::pagination::cursor::Cursor;
use common::sort::Sort;
use common::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
use serde_json::Value;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchShopsRequest {
    pub search: ShopSearch,
    pub sort: Option<Sort<crate::core::sort_shop_field::SortShopField>>,
    pub cursor: Option<Cursor<Value>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShopSummary {
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,
    pub shop_type: ShopType,
    pub partner_status: ShopPartnerStatus,
    pub domains: Vec<Domain>,
    pub image: Option<Url>,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchShopsResult {
    pub items: Vec<ShopSummary>,
    pub cursor: Cursor<Value>,
    pub total: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchShopsError {
    #[error("temporary search failure")]
    TemporarilyUnavailable,
    #[error("internal failure")]
    Internal,
}

#[async_trait::async_trait]
pub trait SearchShopsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchShopsRequest,
    ) -> Result<SearchShopsResult, SearchShopsError>;
}
