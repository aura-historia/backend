use crate::ports::{ShopSearchReadError, ShopSearchReader};
use common::domain::Domain;
use common::operation_context::OperationContext;
use common::pagination::cursor::Cursor;
use common::sort::Sort;
use common::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
use serde_json::Value;
use shop_core::{partner_status::ShopPartnerStatus, shop_search::ShopSearch, shop_type::ShopType};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchShopsRequest {
    pub search: ShopSearch,
    pub sort: Option<Sort<shop_core::sort_shop_field::SortShopField>>,
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
    #[error("temporary shop search failure")]
    TemporarilyUnavailable,
    #[error("invalid shop search read model")]
    InvalidReadModel,
    #[error("internal shop search failure")]
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

pub struct SearchShopsHandler<R> {
    reader: R,
}

impl<R> SearchShopsHandler<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }
}

#[async_trait::async_trait]
impl<R> SearchShopsUseCase for SearchShopsHandler<R>
where
    R: ShopSearchReader,
{
    #[tracing::instrument(
        name = "search_shops",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchShopsRequest,
    ) -> Result<SearchShopsResult, SearchShopsError> {
        self.reader.search(&request).await.map_err(Into::into)
    }
}

impl From<ShopSearchReadError> for SearchShopsError {
    fn from(error: ShopSearchReadError) -> Self {
        match error {
            ShopSearchReadError::TemporarilyUnavailable => Self::TemporarilyUnavailable,
            ShopSearchReadError::InvalidReadModel => Self::InvalidReadModel,
            ShopSearchReadError::Internal => Self::Internal,
        }
    }
}
