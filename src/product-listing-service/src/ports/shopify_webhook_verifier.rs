/// Why a Shopify webhook delivery was not authenticated. Carries no signing material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ShopifyWebhookVerificationError {
    #[error("Shopify webhook signature is missing, repeated or malformed")]
    MalformedSignature,
    #[error("Shopify webhook signature does not match a configured secret")]
    InvalidSignature,
    #[error("Shopify webhook signature could not be computed")]
    CryptographicFailure,
}

/// Authenticates a Shopify webhook's exact body bytes before anything in it is read.
pub trait ShopifyWebhookVerifier: Send + Sync {
    /// `signatures` are every received `X-Shopify-Hmac-Sha256` value, in order.
    fn verify(
        &self,
        signatures: &[&[u8]],
        raw_body: &[u8],
    ) -> Result<(), ShopifyWebhookVerificationError>;
}
