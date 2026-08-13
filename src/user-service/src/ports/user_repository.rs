#![allow(dead_code)]

use common::error::boxed::BoxError;
use common::versioned::Versioned;
use common::{stripe_customer_id::StripeCustomerId, user_id::UserId};
use serde_email::Email;
use user_core::user::User;

common::version_newtype!(UserStorageVersion);

pub type VersionedUser = Versioned<User, UserStorageVersion>;

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

    async fn delete_by_id(&mut self, id: UserId) -> Result<bool, UserRepositoryError>;
}

pub trait UserRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl UserRepository + 'tx;
}
