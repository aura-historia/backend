use crate::core::{
    address::StructuredAddress, affiliate_configuration::AffiliateConfiguration,
    shop_type::ShopType, woocommerce_webhook_secret::WoocommerceWebhookSecret,
};
use common::currency::domain::Currency;
use common::domain::Domain;
use common::language::domain::Language;
use common::operation_context::OperationContext;
use common::patch_field::PatchField;
use common::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
use serde_email::Email;
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateShopCommand {
    pub shop_id: ShopId,
    pub shop_type: PatchField<ShopType>,
    pub domains: PatchField<HashSet<Domain>>,
    pub shopify_domain: PatchField<Domain>,
    pub shopify_currency: PatchField<Currency>,
    pub shopify_language: PatchField<Language>,
    pub woocommerce_webhook_secret: PatchField<WoocommerceWebhookSecret>,
    pub woocommerce_currency: PatchField<Currency>,
    pub woocommerce_language: PatchField<Language>,
    pub url: PatchField<Url>,
    pub image: PatchField<Url>,
    pub structured_address: PatchField<StructuredAddress>,
    pub phone: PatchField<String>,
    pub email: PatchField<Email>,
    pub affiliate_configuration: PatchField<AffiliateConfiguration>,
    pub idempotency_key: Option<String>,
}

impl UpdateShopCommand {
    pub fn is_empty(&self) -> bool {
        !self.shop_type.is_changed()
            && !self.domains.is_changed()
            && !self.shopify_domain.is_changed()
            && !self.shopify_currency.is_changed()
            && !self.shopify_language.is_changed()
            && !self.woocommerce_webhook_secret.is_changed()
            && !self.woocommerce_currency.is_changed()
            && !self.woocommerce_language.is_changed()
            && !self.url.is_changed()
            && !self.image.is_changed()
            && !self.structured_address.is_changed()
            && !self.phone.is_changed()
            && !self.email.is_changed()
            && !self.affiliate_configuration.is_changed()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateShopResult {
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateShopError {
    #[error("shop not found")]
    NotFound,
    #[error("concurrent shop update")]
    ConcurrencyConflict,
    #[error("operation not permitted")]
    Forbidden,
    #[error("invalid shop update")]
    InvalidUpdate,
    #[error("invalid shop address")]
    InvalidAddress,
    #[error("invalid shop integration change")]
    InvalidIntegrationChange,
    #[error("temporary persistence failure")]
    TemporarilyUnavailable,
    #[error("internal failure")]
    Internal,
}

#[async_trait::async_trait]
pub trait UpdateShopUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateShopCommand,
    ) -> Result<UpdateShopResult, UpdateShopError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_report_empty_update_when_all_fields_unchanged() {
        let command = UpdateShopCommand {
            shop_id: ShopId::new(),
            ..Default::default()
        };

        assert!(command.is_empty());
    }

    #[test]
    fn should_report_non_empty_update_when_field_set() {
        let command = UpdateShopCommand {
            shop_id: ShopId::new(),
            shop_type: PatchField::Set(ShopType::Marketplace),
            ..Default::default()
        };

        assert!(!command.is_empty());
    }

    #[test]
    fn should_report_non_empty_update_when_optional_field_cleared() {
        let command = UpdateShopCommand {
            shop_id: ShopId::new(),
            email: PatchField::Clear,
            ..Default::default()
        };

        assert!(!command.is_empty());
    }
}
