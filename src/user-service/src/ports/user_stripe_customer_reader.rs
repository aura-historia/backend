#![allow(dead_code)]

use crate::use_cases::queries::find_user_by_stripe_customer_id::{
    FindUserByStripeCustomerIdRequest, UserStripeLookupView,
};
use application::error::BoxError;

#[derive(Debug, thiserror::Error)]
pub enum UserStripeCustomerReadError {
    #[error("temporary user stripe customer lookup failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user stripe customer read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user stripe customer lookup failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait UserStripeCustomerReader: Send {
    async fn find_by_stripe_customer_id(
        &mut self,
        request: &FindUserByStripeCustomerIdRequest,
    ) -> Result<Option<UserStripeLookupView>, UserStripeCustomerReadError>;
}

pub trait UserStripeCustomerReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl UserStripeCustomerReader + 'tx;
}
