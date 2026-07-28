use crate::ports::{ShopDetailsReadError, ShopDetailsReader};
use common::currency::domain::Currency;
use common::domain::Domain;
use common::language::domain::Language;
use common::operation_context::OperationContext;
use common::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
use serde_email::Email;
use shop_core::{
    address::{GeoAddress, StructuredAddress},
    affiliate_configuration::AffiliateConfiguration,
    partner_status::ShopPartnerStatus,
    shop_type::ShopType,
};
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub enum GetShopRequest {
    ById(ShopId),
    BySlug(ShopSlugId),
    ByShopifyDomain(Domain),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShopDetailsView {
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,
    pub shop_type: ShopType,
    pub domains: HashSet<Domain>,
    pub shopify_domain: Option<Domain>,
    pub shopify_currency: Option<Currency>,
    pub shopify_language: Option<Language>,
    pub url: Option<Url>,
    pub view_url: Option<Url>,
    pub image: Option<Url>,
    pub structured_address: Option<StructuredAddress>,
    pub geo_address: Option<GeoAddress>,
    pub phone: Option<String>,
    pub email: Option<Email>,
    pub partner_status: ShopPartnerStatus,
    pub affiliate_configuration: Option<AffiliateConfiguration>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum GetShopError {
    #[error("shop not found")]
    NotFound,
    #[error("temporary shop details read failure")]
    TemporarilyUnavailable,
    #[error("invalid shop details read model")]
    InvalidReadModel,
    #[error("internal shop details read failure")]
    Internal,
}

#[async_trait::async_trait]
pub trait GetShopUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetShopRequest,
    ) -> Result<ShopDetailsView, GetShopError>;
}

pub struct GetShopHandler<R> {
    reader: R,
}

impl<R> GetShopHandler<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }
}

#[async_trait::async_trait]
impl<R> GetShopUseCase for GetShopHandler<R>
where
    R: ShopDetailsReader,
{
    #[tracing::instrument(
        name = "get_shop",
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
        request: GetShopRequest,
    ) -> Result<ShopDetailsView, GetShopError> {
        self.reader
            .find_details(&request)
            .await?
            .ok_or(GetShopError::NotFound)
    }
}

impl From<ShopDetailsReadError> for GetShopError {
    fn from(error: ShopDetailsReadError) -> Self {
        match error {
            ShopDetailsReadError::TemporarilyUnavailable => Self::TemporarilyUnavailable,
            ShopDetailsReadError::InvalidReadModel => Self::InvalidReadModel,
            ShopDetailsReadError::Internal => Self::Internal,
        }
    }
}
