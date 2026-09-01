#![allow(dead_code)]

use application::error::BoxError;
use geo::core::address::GeoAddress;
use localization::Language;
use money::Currency;
use serde_email::Email;
use user_core::measurement_unit::MeasurementUnit;
use user_core::stripe_customer_id::StripeCustomerId;
use user_core::user_id::UserId;
use user_core::{first_name::FirstName, last_name::LastName, role::UserRole, tier::UserTier};

#[derive(Debug, Clone, PartialEq)]
pub struct UserDetailsView {
    pub user_id: UserId,
    pub email: Email,
    pub first_name: Option<FirstName>,
    pub last_name: Option<LastName>,
    pub language: Option<Language>,
    pub currency: Option<Currency>,
    pub measurement_unit: Option<MeasurementUnit>,
    pub show_unassessed_or_sensitive_content: bool,
    pub tier: UserTier,
    pub role: UserRole,
    pub stripe_customer_id: Option<StripeCustomerId>,
    pub geo_address: Option<GeoAddress>,
}

#[derive(Debug, thiserror::Error)]
pub enum UserAccountReadError {
    #[error("temporary user account read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user account read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user account read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait UserAccountReader: Send {
    async fn find_by_id(
        &mut self,
        user_id: UserId,
    ) -> Result<Option<UserDetailsView>, UserAccountReadError>;
}

pub trait UserAccountReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl UserAccountReader + 'tx;
}
