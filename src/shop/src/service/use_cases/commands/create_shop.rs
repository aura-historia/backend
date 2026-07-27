use crate::core::{
    address::{GeoAddress, StructuredAddress},
    affiliate_configuration::AffiliateConfiguration,
    shop_aggregate::Shop,
    shop_type::ShopType,
    woocommerce_webhook_secret::WoocommerceWebhookSecret,
};
use common::currency::domain::Currency;
use common::domain::Domain;
use common::language::domain::Language;
use common::operation_context::OperationContext;
use common::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
use serde_email::Email;
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateShopCommand {
    pub name: ShopName,
    pub shop_type: ShopType,
    pub domains: HashSet<Domain>,
    pub shopify_domain: Option<Domain>,
    pub shopify_currency: Option<Currency>,
    pub shopify_language: Option<Language>,
    pub woocommerce_webhook_secret: Option<WoocommerceWebhookSecret>,
    pub woocommerce_currency: Option<Currency>,
    pub woocommerce_language: Option<Language>,
    pub url: Option<Url>,
    pub image: Option<Url>,
    pub structured_address: Option<StructuredAddress>,
    pub phone: Option<String>,
    pub email: Option<Email>,
    pub affiliate_configuration: Option<AffiliateConfiguration>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateShopResult {
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateShopError {
    #[error("shop slug already exists")]
    SlugConflict,
    #[error("operation not permitted")]
    Forbidden,
    #[error("invalid shop address")]
    InvalidAddress,
    #[error("temporary persistence failure")]
    TemporarilyUnavailable,
    #[error("internal failure")]
    Internal,
}

#[async_trait::async_trait]
pub trait CreateShopUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateShopCommand,
    ) -> Result<CreateShopResult, CreateShopError>;
}

impl From<&Shop> for CreateShopResult {
    fn from(shop: &Shop) -> Self {
        Self {
            shop_id: shop.id(),
            shop_slug_id: shop.slug_id().clone(),
            name: shop.name().clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GeocodedShopAddress {
    pub structured_address: Option<StructuredAddress>,
    pub geo_address: Option<GeoAddress>,
}
