#![allow(dead_code)]

use application::error::BoxError;
use serde_email::Email;
use user_core::stripe_customer_id::StripeCustomerId;
use user_core::user::User;
use user_core::user_id::UserId;

domain_primitives::version_newtype!(UserStorageVersion);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UserMarketingEmailConsentRevision(i64);

impl UserMarketingEmailConsentRevision {
    pub const INITIAL: Self = Self(0);

    pub fn into_inner(self) -> i64 {
        self.0
    }

    pub fn checked_next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

impl TryFrom<i64> for UserMarketingEmailConsentRevision {
    type Error = InvalidUserMarketingEmailConsentRevision;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if value < 0 {
            Err(InvalidUserMarketingEmailConsentRevision)
        } else {
            Ok(Self(value))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("marketing email consent revision must be nonnegative")]
pub struct InvalidUserMarketingEmailConsentRevision;

#[derive(Debug, Clone, PartialEq)]
pub struct VersionedUser {
    pub value: User,
    pub version: UserStorageVersion,
    pub marketing_email_consent_revision: UserMarketingEmailConsentRevision,
}

impl VersionedUser {
    pub fn new(
        value: User,
        version: UserStorageVersion,
        marketing_email_consent_revision: UserMarketingEmailConsentRevision,
    ) -> Self {
        Self {
            value,
            version,
            marketing_email_consent_revision,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum UserInsertOutcome {
    Created(VersionedUser),
    Existing(VersionedUser),
}

#[derive(Debug, thiserror::Error)]
pub enum UserRepositoryError {
    #[error("concurrent user update")]
    ConcurrencyConflict,
    #[error("user email conflict")]
    EmailConflict {
        #[source]
        source: BoxError,
    },
    #[error("user stripe customer conflict")]
    StripeCustomerConflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted user state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait UserRepository: Send {
    async fn find_by_id(
        &mut self,
        id: UserId,
    ) -> Result<Option<VersionedUser>, UserRepositoryError>;

    async fn find_by_email(
        &mut self,
        email: &Email,
    ) -> Result<Option<VersionedUser>, UserRepositoryError>;

    async fn find_by_stripe_customer_id(
        &mut self,
        stripe_customer_id: &StripeCustomerId,
    ) -> Result<Option<VersionedUser>, UserRepositoryError>;

    async fn insert(&mut self, user: &User) -> Result<VersionedUser, UserRepositoryError>;

    async fn insert_if_absent(
        &mut self,
        user: &User,
    ) -> Result<UserInsertOutcome, UserRepositoryError>;

    async fn update(
        &mut self,
        user: &User,
        expected_version: UserStorageVersion,
    ) -> Result<VersionedUser, UserRepositoryError>;

    /// Records one newly accepted consent decision and fences stale proof work.
    /// Each call advances both the root User version and the consent revision,
    /// even when the consent boolean is already true.
    async fn record_marketing_email_consent_decision(
        &mut self,
        user: &User,
        expected_version: UserStorageVersion,
        expected_consent_revision: UserMarketingEmailConsentRevision,
    ) -> Result<VersionedUser, UserRepositoryError>;

    async fn delete_by_id(&mut self, id: UserId) -> Result<bool, UserRepositoryError>;
}

pub trait UserRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl UserRepository + 'tx;
}
