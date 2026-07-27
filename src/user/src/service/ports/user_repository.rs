#![allow(dead_code)]

use crate::core::user_aggregate::User;
use common::versioned::Versioned;
use common::{stripe_customer_id::StripeCustomerId, user_id::UserId};
use serde_email::Email;

common::version_newtype!(UserStorageVersion);

#[derive(Debug, thiserror::Error)]
pub enum UserRepositoryError {
    #[error("concurrent user update")]
    ConcurrencyConflict,
    #[error("user email conflict")]
    EmailConflict,
    #[error("user stripe customer conflict")]
    StripeCustomerConflict,
    #[error("temporary persistence failure")]
    TemporarilyUnavailable,
    #[error("invalid persisted user state")]
    InvalidPersistedState,
    #[error("internal persistence failure")]
    Internal,
}

#[async_trait::async_trait]
pub(crate) trait UserRepository {
    async fn find_by_id(
        &mut self,
        id: UserId,
    ) -> Result<Option<Versioned<User, UserStorageVersion>>, UserRepositoryError>;

    async fn find_by_email(
        &mut self,
        email: &Email,
    ) -> Result<Option<Versioned<User, UserStorageVersion>>, UserRepositoryError>;

    async fn find_by_stripe_customer_id(
        &mut self,
        stripe_customer_id: &StripeCustomerId,
    ) -> Result<Option<Versioned<User, UserStorageVersion>>, UserRepositoryError>;

    async fn insert(&mut self, user: &User) -> Result<(), UserRepositoryError>;

    async fn update(
        &mut self,
        user: &User,
        expected_version: UserStorageVersion,
    ) -> Result<(), UserRepositoryError>;
}
