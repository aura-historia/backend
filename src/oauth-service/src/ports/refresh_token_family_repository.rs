use super::OAuthConnectionRepositoryError;
use credential_core::oauth_connection_id::OAuthConnectionId;
use oauth_core::refresh_token::{
    HashedRawRefreshToken, PresentedRefreshToken, RefreshTokenFamily, RefreshTokenFamilyId,
};

/// Persists refresh-token families together with every token they issued.
#[async_trait::async_trait]
pub trait RefreshTokenFamilyRepository: Send {
    /// Stores a new family with its first token.
    async fn insert(
        &mut self,
        family: &RefreshTokenFamily,
    ) -> Result<(), OAuthConnectionRepositoryError>;

    /// The connection of the family that issued `token`, read without locking, so the caller
    /// can lock the connection first.
    async fn find_connection_id(
        &mut self,
        token: &HashedRawRefreshToken,
    ) -> Result<Option<OAuthConnectionId>, OAuthConnectionRepositoryError>;

    /// Locks the family that issued `token` and reads it with the presented token's state.
    async fn find_by_token_for_update(
        &mut self,
        token: &HashedRawRefreshToken,
    ) -> Result<Option<(RefreshTokenFamily, PresentedRefreshToken)>, OAuthConnectionRepositoryError>;

    /// Stores a rotated family: `replaced` stops being current at the new token's issue time.
    async fn record_rotation(
        &mut self,
        family: &RefreshTokenFamily,
        replaced: &HashedRawRefreshToken,
    ) -> Result<(), OAuthConnectionRepositoryError>;

    /// Ends the family and every token it issued.
    async fn delete(
        &mut self,
        family_id: RefreshTokenFamilyId,
    ) -> Result<(), OAuthConnectionRepositoryError>;
}

pub trait RefreshTokenFamilyRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl RefreshTokenFamilyRepository + 'tx;
}
