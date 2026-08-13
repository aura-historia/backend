use common::error::boxed::BoxError;

#[derive(Debug, thiserror::Error)]
pub enum StripeBillingError {
    #[error("Stripe is temporarily unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("Stripe rejected the billing request")]
    Rejected {
        #[source]
        source: BoxError,
    },
    #[error("Stripe returned an invalid billing response")]
    InvalidResponse {
        #[source]
        source: BoxError,
    },
}
