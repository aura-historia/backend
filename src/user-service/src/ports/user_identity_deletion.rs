use super::CognitoIdentity;
use application::error::BoxError;
use user_core::user_id::UserId;

/// A failed identity deletion port call. Each variant names the system that failed; the cause
/// stays in the source chain.
#[derive(Debug, thiserror::Error)]
pub enum UserIdentityDeletionError {
    #[error("user identity deletion storage failed")]
    Storage(#[source] BoxError),
    #[error("persisted user identity deletion state is invalid")]
    InvalidPersistedState(#[source] BoxError),
    /// Another attempt took over the claim's lease before this write.
    #[error("user identity deletion claim is no longer held")]
    ClaimLost,
    #[error("identity provider request failed")]
    IdentityProvider {
        failure: IdentityProviderFailure,
        http_status: Option<u16>,
        #[source]
        source: BoxError,
    },
}

/// Static, safe classes of an identity provider failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityProviderFailure {
    Timeout,
    Throttled,
    Unavailable,
    Authentication,
    Configuration,
    /// The provider answered with an identity that does not match the deletion target.
    InvalidResponse,
}

impl IdentityProviderFailure {
    /// The lower-case class used by worker failure diagnostics.
    pub const fn diagnostic_category(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Throttled => "throttled",
            Self::Unavailable => "unavailable",
            Self::Authentication => "authentication",
            Self::Configuration => "configuration",
            Self::InvalidResponse => "response_invalid",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid identity username")]
pub struct InvalidUserIdentityUsername;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserIdentityUsername(String);
impl UserIdentityUsername {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for UserIdentityUsername {
    type Error = InvalidUserIdentityUsername;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.chars().count() > 128
            || value.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(InvalidUserIdentityUsername);
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
