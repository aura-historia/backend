use crate::OperationError;
use aura_historia_contracts::partner_product_listings::ProductListingBatchFailureCode as Code;
use product_listing_service::use_cases::{
    CreateProductListingError, UpdateProductListingError, UpsertProductListingError,
    WithdrawProductListingError,
    commands::partner_product_listing_batches::ProductListingBatchInputError,
};
pub(super) fn input(error: ProductListingBatchInputError) -> OperationError {
    match error {
        ProductListingBatchInputError::AuthenticationRequired => OperationError::Forbidden,
        error @ ProductListingBatchInputError::TooManyItems => {
            OperationError::InvalidArguments(Box::new(error))
        }
    }
}
pub(super) fn create(error: CreateProductListingError) -> Code {
    use CreateProductListingError::*;
    match error {
        AuthenticatedActorRequired | Forbidden => Code::Forbidden,
        ListingSourceNotFound | AuctionNotFound => Code::NotFound,
        AuctionSourceMismatch | SourceListingAlreadyExists => Code::Conflict,
        InvalidProductListing => Code::InvalidInput,
        PartnerAuthorizationInternal { .. } | CreatedEventMissing => Code::InternalError,
        ProductListingTitleSlugAlreadyExists
        | ProductListingTitleSlugGenerationExhausted
        | AuctionReferenceTemporarilyUnavailable { .. }
        | PartnerAuthorizationTemporarilyUnavailable { .. }
        | PersistenceFailed
        | InvalidDeliveryPersisted { .. }
        | EventAppenderFailed { .. }
        | BeginTransactionFailed(_)
        | CommitTransactionFailed(_) => Code::TemporarilyUnavailable,
    }
}
pub(super) fn update(error: UpdateProductListingError) -> Code {
    use UpdateProductListingError::*;
    match error {
        AuthenticatedActorRequired | Forbidden => Code::Forbidden,
        ListingSourceNotFound | NotFound | AuctionNotFound => Code::NotFound,
        AuctionSourceMismatch | ListingWithdrawn => Code::Conflict,
        UrlRequired | InvalidProductListing => Code::InvalidInput,
        PartnerAuthorizationInternal { .. } => Code::InternalError,
        AuctionReferenceTemporarilyUnavailable { .. }
        | PartnerAuthorizationTemporarilyUnavailable { .. }
        | PersistenceFailed
        | InvalidDeliveryPersisted { .. }
        | EventAppenderFailed { .. }
        | BeginTransactionFailed(_)
        | CommitTransactionFailed(_) => Code::TemporarilyUnavailable,
    }
}
pub(super) fn upsert(error: UpsertProductListingError) -> Code {
    use UpsertProductListingError::*;
    match error {
        AuthenticatedActorRequired | Forbidden => Code::Forbidden,
        ListingSourceNotFound | AuctionNotFound => Code::NotFound,
        AuctionSourceMismatch | ListingWithdrawn => Code::Conflict,
        InvalidProductListing { .. } => Code::InvalidInput,
        PartnerAuthorizationInternal { .. } => Code::InternalError,
        ProductListingTitleSlugGenerationExhausted
        | AuctionReferenceTemporarilyUnavailable { .. }
        | PartnerAuthorizationTemporarilyUnavailable { .. }
        | PersistenceFailed
        | InvalidDeliveryPersisted { .. }
        | EventAppenderFailed { .. }
        | BeginTransactionFailed(_)
        | CommitTransactionFailed(_) => Code::TemporarilyUnavailable,
    }
}
pub(super) fn withdraw(error: WithdrawProductListingError) -> Code {
    use WithdrawProductListingError::*;
    match error {
        AuthenticatedActorRequired | Forbidden => Code::Forbidden,
        ListingSourceNotFound | NotFound => Code::NotFound,
        PartnerAuthorizationInternal { .. } => Code::InternalError,
        PartnerAuthorizationTemporarilyUnavailable { .. }
        | PersistenceFailed
        | InvalidDeliveryPersisted { .. }
        | EventAppenderFailed { .. }
        | BeginTransactionFailed(_)
        | CommitTransactionFailed(_) => Code::TemporarilyUnavailable,
    }
}
