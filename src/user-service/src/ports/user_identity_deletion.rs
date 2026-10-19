use super::CognitoIdentity;
use application::error::BoxError;
use user_core::user_id::UserId;

#[derive(Debug, thiserror::Error)]
#[error("user identity deletion failed")]
pub struct UserIdentityDeletionError(#[source] pub BoxError);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserIdentityUsername(String);
impl UserIdentityUsername {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for UserIdentityUsername {
    type Error = UserIdentityDeletionError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.chars().count() > 128
            || value.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(UserIdentityDeletionError(application::error::static_error(
                "invalid identity username",
            )));
        }
        Ok(Self(value))
    }
}
#[derive(Debug, Clone)]
pub struct UserIdentityDeletionClaim {
    pub user_id: UserId,
    pub identity: CognitoIdentity,
    pub username: Option<UserIdentityUsername>,
    pub version: u64,
}
#[derive(Debug, Clone)]
pub enum UserIdentityDeletionClaimOutcome {
    Claimed(UserIdentityDeletionClaim),
    Completed,
    Deferred,
}
#[async_trait::async_trait]
pub trait UserIdentityDeletionStore: Send {
    /// Retain the identity and append its CDC request before the binding cascades away.
    async fn request_for_user(&mut self, user_id: UserId) -> Result<(), UserIdentityDeletionError>;
    async fn claim(
        &mut self,
        user_id: UserId,
    ) -> Result<UserIdentityDeletionClaimOutcome, UserIdentityDeletionError>;
    /// Commit the provider target before attempting its irreversible removal.
    async fn save_username(
        &mut self,
        claim: &UserIdentityDeletionClaim,
        username: &UserIdentityUsername,
    ) -> Result<(), UserIdentityDeletionError>;
    async fn complete(
        &mut self,
        claim: &UserIdentityDeletionClaim,
    ) -> Result<(), UserIdentityDeletionError>;
}
pub trait UserIdentityDeletionStoreFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl UserIdentityDeletionStore + 'tx;
}
#[async_trait::async_trait]
pub trait UserIdentityDeleter: Send + Sync {
    /// An empty eventually consistent lookup cannot prove deletion complete.
    async fn resolve_username(
        &self,
        identity: &CognitoIdentity,
    ) -> Result<Option<UserIdentityUsername>, UserIdentityDeletionError>;
    /// Verify the exact subject, then remove it or confirm the saved target is absent.
    async fn delete_identity(
        &self,
        identity: &CognitoIdentity,
        username: &UserIdentityUsername,
    ) -> Result<(), UserIdentityDeletionError>;
}
