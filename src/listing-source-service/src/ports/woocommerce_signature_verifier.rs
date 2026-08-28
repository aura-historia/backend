use listing_source_core::ListingSourceId;

use super::ListingSourceReadError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WoocommerceSignatureVerification {
    Valid,
    Invalid,
    SecretNotConfigured,
}

#[async_trait::async_trait]
pub trait WoocommerceSignatureVerifier: Send + Sync {
    async fn verify(
        &self,
        id: ListingSourceId,
        body: &[u8],
        signature: &[u8],
    ) -> Result<WoocommerceSignatureVerification, ListingSourceReadError>;
}
