use application::error::BoxError;
use localization::Language;
use money::Currency;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop_id::ShopId;

#[derive(Debug, Clone, PartialEq)]
pub struct WoocommerceWebhookShop {
    pub shop_id: ShopId,
    pub partner_status: ShopPartnerStatus,
    pub currency: Option<Currency>,
    pub language: Option<Language>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WoocommerceWebhookSignatureVerification {
    Valid,
    Invalid,
    SecretNotConfigured,
}

#[derive(Debug, thiserror::Error)]
pub enum WoocommerceWebhookShopReadError {
    #[error("temporary WooCommerce webhook shop read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid WooCommerce webhook shop read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait WoocommerceWebhookShopReader: Send {
    async fn find_for_webhook(
        &mut self,
        shop_id: ShopId,
    ) -> Result<Option<WoocommerceWebhookShop>, WoocommerceWebhookShopReadError>;
}

pub trait WoocommerceWebhookShopReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl WoocommerceWebhookShopReader + 'tx;
}

#[async_trait::async_trait]
pub trait WoocommerceWebhookSignatureVerifier: Send {
    async fn verify(
        &mut self,
        shop_id: ShopId,
        body: &[u8],
        signature: &[u8],
    ) -> Result<WoocommerceWebhookSignatureVerification, WoocommerceWebhookShopReadError>;
}

pub trait WoocommerceWebhookSignatureVerifierFactory<Tx>: Send + Sync {
    fn verifier_in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl WoocommerceWebhookSignatureVerifier + 'tx;
}
