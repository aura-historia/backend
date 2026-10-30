use super::{
    capture_product_listing_raw_observation::capture_in_transaction,
    create_product_listing::CreateProductListingApplyError,
    product_listing_ingestion::{
        ProductListingIngestionActor, ProductListingIngestionFingerprint,
        ProductListingIngestionIntent, ProductListingIngestionMessage,
    },
    update_product_listing::UpdateTarget,
    upsert_product_listing::UpsertProductListingApplyError,
    withdraw_product_listing::WithdrawTarget,
};
use crate::ports::product_listing_delivery::{
    ProductListingDeliveryReferencesFactory, UnavailableProductListingDeliveryReferences,
};
use crate::use_cases::{
    ChangeProductListingDeliveryHandler, ChangeProductListingDeliveryResult,
    ProductListingDeliveryError,
};
use crate::{
    ports::{
        PartnerProductListingAuthorizerFactory, ProductListingCommandCompletionCode,
        ProductListingCommandReceiptError, ProductListingCommandReceiptStore,
        ProductListingCommandReceiptStoreFactory, ProductListingCommandReceiptWrite,
        ProductListingEventAppenderFactory, ProductListingRawCaptureWriterFactory,
        ProductListingRepositoryFactory,
    },
    product_listing_title_slug_creation::MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS,
    use_cases::{
        CaptureProductListingRawObservationError, CaptureProductListingRawObservationResult,
        CreateProductListingError, CreateProductListingHandler, CreateProductListingResult,
        UpdateProductListingError, UpdateProductListingHandler, UpdateProductListingResult,
        UpsertProductListingError, UpsertProductListingHandler, UpsertProductListingResult,
        WithdrawProductListingError, WithdrawProductListingHandler, WithdrawProductListingResult,
    },
};
use application::{
    operation_context::{CredentialCapability, OperationContext, Principal},
    transaction::{Transaction, UnitOfWork},
};
use auction_service::ports::AuctionReferenceValidatorFactory;
use product_listing_core::product_listing_id::ProductListingId;
use std::collections::BTreeSet;

/// The verified fingerprint is supplied by the ingress codec, not calculated from transport data here.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingIngestionEnvelope {
    pub message: ProductListingIngestionMessage,
    pub fingerprint: ProductListingIngestionFingerprint,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingIngestionCompletion {
    Applied(ProductListingIngestionEffect),
    AlreadyCompleted,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingIngestionEffect {
    Created(CreateProductListingResult),
    Updated(UpdateProductListingResult),
    Upserted(UpsertProductListingResult),
    Withdrawn(WithdrawProductListingResult),
    RawCaptured(CaptureProductListingRawObservationResult),
    DeliveryChanged(ChangeProductListingDeliveryResult),
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingIngestionError {
    #[error("invalid ingestion command metadata")]
    InvalidMetadata,
    #[error(transparent)]
    Delivery(#[from] ProductListingDeliveryError),
    #[error("command identity was reused for different content")]
    FingerprintConflict,
    #[error("failed to begin ingestion transaction")]
    BeginTransactionFailed(#[source] application::error::BoxError),
    #[error("failed to commit ingestion transaction; completion is unconfirmed")]
    CommitTransactionFailed(#[source] application::error::BoxError),
    #[error("ingestion receipt failed")]
    Receipt(#[source] ProductListingCommandReceiptError),
    #[error(transparent)]
    Create(#[from] CreateProductListingError),
    #[error(transparent)]
    Update(#[from] UpdateProductListingError),
    #[error(transparent)]
    Upsert(#[from] UpsertProductListingError),
    #[error(transparent)]
    Withdraw(#[from] WithdrawProductListingError),
    #[error(transparent)]
    CaptureRaw(#[from] CaptureProductListingRawObservationError),
}

/// How the ordered command consumer settles a failed command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProductListingIngestionFailureClass {
    /// A valid command that current authoritative state or its own content deterministically
    /// rejects. Retrying cannot succeed, and a retained rejection would hold every later
    /// command of its FIFO group until it dead-letters, so the consumer completes it without a
    /// receipt.
    BusinessRejection,
    /// A command that cannot apply as delivered however often it is retried: invalid metadata,
    /// a command, provider-receipt or shipping key reused for different content, a source-key
    /// hash collision, or invalid persisted state that needs an operator repair. The consumer
    /// moves it to failure custody when it is delivered again instead of letting it hold its
    /// FIFO group for the whole retry budget.
    Unprocessable,
    /// Technical or temporary failures keep retry custody.
    Retryable,
    /// A concurrent writer won a uniqueness or version race. It keeps retry custody like
    /// `Retryable`, but no dependency failed: a later attempt sees the winner's result.
    Contended,
}

impl ProductListingIngestionError {
    /// Bounded, payload-free diagnostic code for the queue boundary. Keep matches exhaustive so
    /// newly introduced failure variants cannot silently collapse into a generic retry reason.
    pub fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::Delivery(e)=>e.diagnostic_code(),
            Self::InvalidMetadata => "INVALID_METADATA",
            Self::FingerprintConflict => "FINGERPRINT_CONFLICT",
            Self::BeginTransactionFailed(_) => "BEGIN_TRANSACTION_FAILED",
            Self::CommitTransactionFailed(_) => "COMMIT_UNCONFIRMED",
            Self::Receipt(error) => match error {
                ProductListingCommandReceiptError::InvalidIdentity => "RECEIPT_INVALID_IDENTITY",
                ProductListingCommandReceiptError::AlreadyExists => "RECEIPT_ALREADY_EXISTS",
                ProductListingCommandReceiptError::InvalidPersistedState => "RECEIPT_INVALID_PERSISTED_STATE",
                ProductListingCommandReceiptError::OperationFailed { .. } => "RECEIPT_OPERATION_FAILED",
            },
            Self::Create(error) => match error {
                CreateProductListingError::AuthenticatedActorRequired => "CREATE_AUTHENTICATED_ACTOR_REQUIRED",
                CreateProductListingError::Forbidden => "CREATE_FORBIDDEN",
                CreateProductListingError::ListingSourceNotFound => "CREATE_LISTING_SOURCE_NOT_FOUND",
                CreateProductListingError::PartnerAuthorizationTemporarilyUnavailable { .. } => "CREATE_PARTNER_AUTHORIZATION_TEMPORARILY_UNAVAILABLE",
                CreateProductListingError::PartnerAuthorizationInternal { .. } => "CREATE_PARTNER_AUTHORIZATION_INTERNAL",
                CreateProductListingError::AuctionNotFound => "CREATE_AUCTION_NOT_FOUND",
                CreateProductListingError::AuctionSourceMismatch => "CREATE_AUCTION_SOURCE_MISMATCH",
                CreateProductListingError::AuctionReferenceTemporarilyUnavailable { .. } => "CREATE_AUCTION_REFERENCE_TEMPORARILY_UNAVAILABLE",
                CreateProductListingError::SourceListingAlreadyExists => "CREATE_SOURCE_LISTING_ALREADY_EXISTS",
                CreateProductListingError::ProductListingTitleSlugAlreadyExists => "CREATE_TITLE_SLUG_ALREADY_EXISTS",
                CreateProductListingError::ProductListingTitleSlugGenerationExhausted => "CREATE_TITLE_SLUG_GENERATION_EXHAUSTED",
                CreateProductListingError::InvalidProductListing => "CREATE_INVALID_PRODUCT_LISTING",
                CreateProductListingError::CreatedEventMissing => "CREATE_CREATED_EVENT_MISSING",
                CreateProductListingError::PersistenceFailed { .. } => "CREATE_PERSISTENCE_FAILED",
                CreateProductListingError::ConcurrencyConflict => "CREATE_CONCURRENCY_CONFLICT",
                CreateProductListingError::InvalidDeliveryPersisted { .. } => {
                    "CREATE_INVALID_DELIVERY_PERSISTED"
                }
                CreateProductListingError::EventAppenderFailed { .. } => "CREATE_EVENT_APPENDER_FAILED",
                CreateProductListingError::BeginTransactionFailed(_) => "CREATE_BEGIN_TRANSACTION_FAILED",
                CreateProductListingError::CommitTransactionFailed(_) => "CREATE_COMMIT_TRANSACTION_FAILED",
            },
            Self::Update(error) => match error {
                UpdateProductListingError::AuthenticatedActorRequired => "UPDATE_AUTHENTICATED_ACTOR_REQUIRED",
                UpdateProductListingError::Forbidden => "UPDATE_FORBIDDEN",
                UpdateProductListingError::ListingSourceNotFound => "UPDATE_LISTING_SOURCE_NOT_FOUND",
                UpdateProductListingError::PartnerAuthorizationTemporarilyUnavailable { .. } => "UPDATE_PARTNER_AUTHORIZATION_TEMPORARILY_UNAVAILABLE",
                UpdateProductListingError::PartnerAuthorizationInternal { .. } => "UPDATE_PARTNER_AUTHORIZATION_INTERNAL",
                UpdateProductListingError::AuctionNotFound => "UPDATE_AUCTION_NOT_FOUND",
                UpdateProductListingError::AuctionSourceMismatch => "UPDATE_AUCTION_SOURCE_MISMATCH",
                UpdateProductListingError::AuctionReferenceTemporarilyUnavailable { .. } => "UPDATE_AUCTION_REFERENCE_TEMPORARILY_UNAVAILABLE",
                UpdateProductListingError::NotFound => "UPDATE_NOT_FOUND",
                UpdateProductListingError::ListingWithdrawn => "UPDATE_LISTING_WITHDRAWN",
                UpdateProductListingError::UrlRequired => "UPDATE_URL_REQUIRED",
                UpdateProductListingError::InvalidProductListing => "UPDATE_INVALID_PRODUCT_LISTING",
                UpdateProductListingError::PersistenceFailed { .. } => "UPDATE_PERSISTENCE_FAILED",
                UpdateProductListingError::ConcurrencyConflict => "UPDATE_CONCURRENCY_CONFLICT",
                UpdateProductListingError::InvalidDeliveryPersisted { .. } => {
                    "UPDATE_INVALID_DELIVERY_PERSISTED"
                }
                UpdateProductListingError::EventAppenderFailed { .. } => "UPDATE_EVENT_APPENDER_FAILED",
                UpdateProductListingError::BeginTransactionFailed(_) => "UPDATE_BEGIN_TRANSACTION_FAILED",
                UpdateProductListingError::CommitTransactionFailed(_) => "UPDATE_COMMIT_TRANSACTION_FAILED",
            },
            Self::Upsert(error) => match error {
                UpsertProductListingError::AuthenticatedActorRequired => "UPSERT_AUTHENTICATED_ACTOR_REQUIRED",
                UpsertProductListingError::Forbidden => "UPSERT_FORBIDDEN",
                UpsertProductListingError::ListingSourceNotFound => "UPSERT_LISTING_SOURCE_NOT_FOUND",
                UpsertProductListingError::PartnerAuthorizationTemporarilyUnavailable { .. } => "UPSERT_PARTNER_AUTHORIZATION_TEMPORARILY_UNAVAILABLE",
                UpsertProductListingError::PartnerAuthorizationInternal { .. } => "UPSERT_PARTNER_AUTHORIZATION_INTERNAL",
                UpsertProductListingError::AuctionNotFound => "UPSERT_AUCTION_NOT_FOUND",
                UpsertProductListingError::AuctionSourceMismatch => "UPSERT_AUCTION_SOURCE_MISMATCH",
                UpsertProductListingError::AuctionReferenceTemporarilyUnavailable { .. } => "UPSERT_AUCTION_REFERENCE_TEMPORARILY_UNAVAILABLE",
                UpsertProductListingError::ListingWithdrawn => "UPSERT_LISTING_WITHDRAWN",
                UpsertProductListingError::InvalidProductListing { .. } => "UPSERT_INVALID_PRODUCT_LISTING",
                UpsertProductListingError::ProductListingTitleSlugGenerationExhausted => "UPSERT_TITLE_SLUG_GENERATION_EXHAUSTED",
                UpsertProductListingError::PersistenceFailed { .. } => "UPSERT_PERSISTENCE_FAILED",
                UpsertProductListingError::ConcurrencyConflict => "UPSERT_CONCURRENCY_CONFLICT",
                UpsertProductListingError::InvalidDeliveryPersisted { .. } => {
                    "UPSERT_INVALID_DELIVERY_PERSISTED"
                }
                UpsertProductListingError::EventAppenderFailed { .. } => "UPSERT_EVENT_APPENDER_FAILED",
                UpsertProductListingError::BeginTransactionFailed(_) => "UPSERT_BEGIN_TRANSACTION_FAILED",
                UpsertProductListingError::CommitTransactionFailed(_) => "UPSERT_COMMIT_TRANSACTION_FAILED",
            },
            Self::Withdraw(error) => match error {
                WithdrawProductListingError::AuthenticatedActorRequired => "WITHDRAW_AUTHENTICATED_ACTOR_REQUIRED",
                WithdrawProductListingError::Forbidden => "WITHDRAW_FORBIDDEN",
                WithdrawProductListingError::ListingSourceNotFound => "WITHDRAW_LISTING_SOURCE_NOT_FOUND",
                WithdrawProductListingError::PartnerAuthorizationTemporarilyUnavailable { .. } => "WITHDRAW_PARTNER_AUTHORIZATION_TEMPORARILY_UNAVAILABLE",
                WithdrawProductListingError::PartnerAuthorizationInternal { .. } => "WITHDRAW_PARTNER_AUTHORIZATION_INTERNAL",
                WithdrawProductListingError::NotFound => "WITHDRAW_NOT_FOUND",
                WithdrawProductListingError::PersistenceFailed { .. } => "WITHDRAW_PERSISTENCE_FAILED",
                WithdrawProductListingError::ConcurrencyConflict => "WITHDRAW_CONCURRENCY_CONFLICT",
                WithdrawProductListingError::InvalidDeliveryPersisted { .. } => {
                    "WITHDRAW_INVALID_DELIVERY_PERSISTED"
                }
                WithdrawProductListingError::EventAppenderFailed { .. } => "WITHDRAW_EVENT_APPENDER_FAILED",
                WithdrawProductListingError::BeginTransactionFailed(_) => "WITHDRAW_BEGIN_TRANSACTION_FAILED",
                WithdrawProductListingError::CommitTransactionFailed(_) => "WITHDRAW_COMMIT_TRANSACTION_FAILED",
            },
            Self::CaptureRaw(error) => match error {
                CaptureProductListingRawObservationError::AuthenticatedActorRequired => "CAPTURE_RAW_AUTHENTICATED_ACTOR_REQUIRED",
                CaptureProductListingRawObservationError::Forbidden => "CAPTURE_RAW_FORBIDDEN",
                CaptureProductListingRawObservationError::SourceRecordKeyTooLong { .. } => "CAPTURE_RAW_SOURCE_RECORD_KEY_TOO_LONG",
                CaptureProductListingRawObservationError::SourceRecordKeyEmbeddedNul => "CAPTURE_RAW_SOURCE_RECORD_KEY_EMBEDDED_NUL",
                CaptureProductListingRawObservationError::InvalidInput { .. } => "CAPTURE_RAW_INVALID_INPUT",
                CaptureProductListingRawObservationError::ListingSourceNotFound => "CAPTURE_RAW_LISTING_SOURCE_NOT_FOUND",
                CaptureProductListingRawObservationError::PartnerAuthorizationTemporarilyUnavailable { .. } => "CAPTURE_RAW_PARTNER_AUTHORIZATION_TEMPORARILY_UNAVAILABLE",
                CaptureProductListingRawObservationError::PartnerAuthorizationInternal { .. } => "CAPTURE_RAW_PARTNER_AUTHORIZATION_INTERNAL",
                CaptureProductListingRawObservationError::SourceRecordKeyHashCollision => "CAPTURE_RAW_SOURCE_RECORD_KEY_HASH_COLLISION",
                CaptureProductListingRawObservationError::ProviderReceiptDigestConflict => "CAPTURE_RAW_PROVIDER_RECEIPT_DIGEST_CONFLICT",
                CaptureProductListingRawObservationError::ProviderSourceOrderAmbiguous => "CAPTURE_RAW_PROVIDER_SOURCE_ORDER_AMBIGUOUS",
                CaptureProductListingRawObservationError::BeginTransactionFailed(_) => "CAPTURE_RAW_BEGIN_TRANSACTION_FAILED",
                CaptureProductListingRawObservationError::CaptureFailed { .. } => "CAPTURE_RAW_CAPTURE_FAILED",
                CaptureProductListingRawObservationError::CommitTransactionFailed(_) => "CAPTURE_RAW_COMMIT_TRANSACTION_FAILED",
            },
        }
    }

    /// How the ordered command consumer settles this failure. Keep matches exhaustive so a new
    /// failure variant has to choose its class deliberately.
    pub fn failure_class(&self) -> ProductListingIngestionFailureClass {
        use ProductListingIngestionFailureClass as Class;
        match self {
            Self::Delivery(error) => match error {
                ProductListingDeliveryError::AuthenticationRequired
                | ProductListingDeliveryError::Forbidden
                | ProductListingDeliveryError::SourceNotFound
                | ProductListingDeliveryError::NotFound
                | ProductListingDeliveryError::StaleRevision
                | ProductListingDeliveryError::ListingWithdrawn
                | ProductListingDeliveryError::IntegrationMismatch
                | ProductListingDeliveryError::ReferenceIneligible
                | ProductListingDeliveryError::InvalidInput { .. } => Class::BusinessRejection,
                ProductListingDeliveryError::Import(error) => shipping_import_failure_class(error),
                ProductListingDeliveryError::Unavailable
                | ProductListingDeliveryError::OperationFailed { .. }
                | ProductListingDeliveryError::Transaction(_) => Class::Retryable,
            },
            Self::InvalidMetadata | Self::FingerprintConflict => Class::Unprocessable,
            Self::BeginTransactionFailed(_) | Self::CommitTransactionFailed(_) => Class::Retryable,
            Self::Receipt(error) => match error {
                ProductListingCommandReceiptError::InvalidIdentity
                | ProductListingCommandReceiptError::InvalidPersistedState => Class::Unprocessable,
                ProductListingCommandReceiptError::AlreadyExists => Class::Contended,
                ProductListingCommandReceiptError::OperationFailed { .. } => Class::Retryable,
            },
            Self::Create(error) => match error {
                CreateProductListingError::AuthenticatedActorRequired
                | CreateProductListingError::Forbidden
                | CreateProductListingError::ListingSourceNotFound
                | CreateProductListingError::AuctionNotFound
                | CreateProductListingError::AuctionSourceMismatch
                | CreateProductListingError::SourceListingAlreadyExists
                | CreateProductListingError::InvalidProductListing => Class::BusinessRejection,
                CreateProductListingError::InvalidDeliveryPersisted { .. } => Class::Unprocessable,
                CreateProductListingError::PartnerAuthorizationTemporarilyUnavailable { .. }
                | CreateProductListingError::PartnerAuthorizationInternal { .. }
                | CreateProductListingError::AuctionReferenceTemporarilyUnavailable { .. }
                | CreateProductListingError::CreatedEventMissing
                | CreateProductListingError::PersistenceFailed { .. }
                | CreateProductListingError::EventAppenderFailed { .. }
                | CreateProductListingError::BeginTransactionFailed(_)
                | CreateProductListingError::CommitTransactionFailed(_) => Class::Retryable,
                CreateProductListingError::ProductListingTitleSlugAlreadyExists
                | CreateProductListingError::ProductListingTitleSlugGenerationExhausted
                | CreateProductListingError::ConcurrencyConflict => Class::Contended,
            },
            Self::Update(error) => match error {
                UpdateProductListingError::AuthenticatedActorRequired
                | UpdateProductListingError::Forbidden
                | UpdateProductListingError::ListingSourceNotFound
                | UpdateProductListingError::AuctionNotFound
                | UpdateProductListingError::AuctionSourceMismatch
                | UpdateProductListingError::NotFound
                | UpdateProductListingError::ListingWithdrawn
                | UpdateProductListingError::UrlRequired
                | UpdateProductListingError::InvalidProductListing => Class::BusinessRejection,
                UpdateProductListingError::InvalidDeliveryPersisted { .. } => Class::Unprocessable,
                UpdateProductListingError::PartnerAuthorizationTemporarilyUnavailable { .. }
                | UpdateProductListingError::PartnerAuthorizationInternal { .. }
                | UpdateProductListingError::AuctionReferenceTemporarilyUnavailable { .. }
                | UpdateProductListingError::PersistenceFailed { .. }
                | UpdateProductListingError::EventAppenderFailed { .. }
                | UpdateProductListingError::BeginTransactionFailed(_)
                | UpdateProductListingError::CommitTransactionFailed(_) => Class::Retryable,
                UpdateProductListingError::ConcurrencyConflict => Class::Contended,
            },
            Self::Upsert(error) => match error {
                UpsertProductListingError::AuthenticatedActorRequired
                | UpsertProductListingError::Forbidden
                | UpsertProductListingError::ListingSourceNotFound
                | UpsertProductListingError::AuctionNotFound
                | UpsertProductListingError::AuctionSourceMismatch
                | UpsertProductListingError::ListingWithdrawn
                | UpsertProductListingError::InvalidProductListing { .. } => Class::BusinessRejection,
                UpsertProductListingError::InvalidDeliveryPersisted { .. } => Class::Unprocessable,
                UpsertProductListingError::PartnerAuthorizationTemporarilyUnavailable { .. }
                | UpsertProductListingError::PartnerAuthorizationInternal { .. }
                | UpsertProductListingError::AuctionReferenceTemporarilyUnavailable { .. }
                | UpsertProductListingError::PersistenceFailed { .. }
                | UpsertProductListingError::EventAppenderFailed { .. }
                | UpsertProductListingError::BeginTransactionFailed(_)
                | UpsertProductListingError::CommitTransactionFailed(_) => Class::Retryable,
                UpsertProductListingError::ProductListingTitleSlugGenerationExhausted
                | UpsertProductListingError::ConcurrencyConflict => Class::Contended,
            },
            Self::Withdraw(error) => match error {
                WithdrawProductListingError::AuthenticatedActorRequired
                | WithdrawProductListingError::Forbidden
                | WithdrawProductListingError::ListingSourceNotFound
                | WithdrawProductListingError::NotFound => Class::BusinessRejection,
                WithdrawProductListingError::InvalidDeliveryPersisted { .. } => Class::Unprocessable,
                WithdrawProductListingError::PartnerAuthorizationTemporarilyUnavailable { .. }
                | WithdrawProductListingError::PartnerAuthorizationInternal { .. }
                | WithdrawProductListingError::PersistenceFailed { .. }
                | WithdrawProductListingError::EventAppenderFailed { .. }
                | WithdrawProductListingError::BeginTransactionFailed(_)
                | WithdrawProductListingError::CommitTransactionFailed(_) => Class::Retryable,
                WithdrawProductListingError::ConcurrencyConflict => Class::Contended,
            },
            Self::CaptureRaw(error) => match error {
                CaptureProductListingRawObservationError::AuthenticatedActorRequired
                | CaptureProductListingRawObservationError::Forbidden
                | CaptureProductListingRawObservationError::SourceRecordKeyTooLong { .. }
                | CaptureProductListingRawObservationError::SourceRecordKeyEmbeddedNul
                | CaptureProductListingRawObservationError::InvalidInput { .. }
                | CaptureProductListingRawObservationError::ListingSourceNotFound
                | CaptureProductListingRawObservationError::ProviderSourceOrderAmbiguous => Class::BusinessRejection,
                CaptureProductListingRawObservationError::SourceRecordKeyHashCollision
                | CaptureProductListingRawObservationError::ProviderReceiptDigestConflict => {
                    Class::Unprocessable
                }
                CaptureProductListingRawObservationError::PartnerAuthorizationTemporarilyUnavailable { .. }
                | CaptureProductListingRawObservationError::PartnerAuthorizationInternal { .. }
                | CaptureProductListingRawObservationError::BeginTransactionFailed(_)
                | CaptureProductListingRawObservationError::CaptureFailed { .. }
                | CaptureProductListingRawObservationError::CommitTransactionFailed(_) => Class::Retryable,
            },
        }
    }
}

/// A shipping import that current shipping state deterministically rejects is a business
/// rejection. Key reuse with different content and corrupt shipping state are unprocessable; a
/// catalog dependency that has not arrived yet and storage or transaction failures keep retry
/// custody.
fn shipping_import_failure_class(
    error: &shipping_service::ShippingError,
) -> ProductListingIngestionFailureClass {
    use ProductListingIngestionFailureClass as Class;
    use shipping_service::ShippingError;
    match error {
        ShippingError::AuthenticationRequired
        | ShippingError::SourceAccessDenied
        | ShippingError::SourceNotFound
        | ShippingError::ProfileNotFound
        | ShippingError::AttributedPartyNotFound { .. }
        | ShippingError::LocationNotFound
        | ShippingError::LocationRetired
        | ShippingError::StaleRevision
        | ShippingError::InvalidRevision
        | ShippingError::ImportAuthorityStale
        | ShippingError::ImportOwnershipConflict
        | ShippingError::ImportOutOfOrder
        | ShippingError::ImportIdentityExists
        | ShippingError::InvalidInput
        | ShippingError::InvalidTerms(_)
        | ShippingError::Profile(_) => Class::BusinessRejection,
        // A key reused with different content is an identity conflict, like a command
        // fingerprint conflict, and moves to failure custody for investigation.
        ShippingError::CreationKeyConflict
        | ShippingError::ImportRequestConflict
        | ShippingError::CorruptShippingState { .. } => Class::Unprocessable,
        ShippingError::ImportDependencyMissing
        | ShippingError::RevisionExhausted
        | ShippingError::PersistenceUnavailable { .. }
        | ShippingError::PersistenceFailed { .. }
        | ShippingError::Transaction(_) => Class::Retryable,
    }
}

#[async_trait::async_trait]
pub trait ProcessProductListingIngestionUseCase: Send + Sync {
    async fn execute(
        &self,
        envelope: ProductListingIngestionEnvelope,
    ) -> Result<ProductListingIngestionCompletion, ProductListingIngestionError>;
}

pub struct ProcessProductListingIngestionHandler<
    U,
    R,
    E,
    A,
    V,
    W,
    C,
    D = UnavailableProductListingDeliveryReferences,
> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
    auction_references: V,
    raw_writer: W,
    receipts: C,
    delivery_references: D,
}

impl<U, R, E, A, V, W, C> ProcessProductListingIngestionHandler<U, R, E, A, V, W, C> {
    pub fn new(
        unit_of_work: U,
        products: R,
        events: E,
        authorizer: A,
        auction_references: V,
        raw_writer: W,
        receipts: C,
    ) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            authorizer,
            auction_references,
            raw_writer,
            receipts,
            delivery_references: UnavailableProductListingDeliveryReferences,
        }
    }
}

impl<U, R, E, A, V, W, C, D> ProcessProductListingIngestionHandler<U, R, E, A, V, W, C, D> {
    pub fn with_delivery_references<D2>(
        self,
        delivery_references: D2,
    ) -> ProcessProductListingIngestionHandler<U, R, E, A, V, W, C, D2> {
        ProcessProductListingIngestionHandler {
            unit_of_work: self.unit_of_work,
            products: self.products,
            events: self.events,
            authorizer: self.authorizer,
            auction_references: self.auction_references,
            raw_writer: self.raw_writer,
            receipts: self.receipts,
            delivery_references,
        }
    }
}
#[async_trait::async_trait]
impl<U, R, E, A, V, W, C, D> ProcessProductListingIngestionUseCase
    for ProcessProductListingIngestionHandler<U, R, E, A, V, W, C, D>
where
    U: UnitOfWork + Clone,
    R: ProductListingRepositoryFactory<U::Tx> + Clone,
    E: ProductListingEventAppenderFactory<U::Tx> + Clone,
    A: PartnerProductListingAuthorizerFactory<U::Tx> + Clone,
    V: AuctionReferenceValidatorFactory<U::Tx> + Clone,
    W: ProductListingRawCaptureWriterFactory<U::Tx>,
    C: ProductListingCommandReceiptStoreFactory<U::Tx>,
    D: ProductListingDeliveryReferencesFactory<U::Tx> + Clone,
{
    async fn execute(
        &self,
        envelope: ProductListingIngestionEnvelope,
    ) -> Result<ProductListingIngestionCompletion, ProductListingIngestionError> {
        let metadata = &envelope.message.metadata;
        let intent = &envelope.message.intent;
        if metadata.operation != intent.operation()
            || metadata.listing_source_id != intent.listing_source_id()
            || metadata.input_count == 0
            || metadata.index >= metadata.input_count
        {
            return Err(ProductListingIngestionError::InvalidMetadata);
        }
        // Admission has already authenticated the actor. A delegated user's accepted write
        // capability is restored without transporting the bearer credential.
        let principal = match &metadata.actor {
            ProductListingIngestionActor::User(id) => Principal::User(*id),
            ProductListingIngestionActor::DelegatedUser(id) => Principal::DelegatedUser {
                user_id: *id,
                capabilities: BTreeSet::from([CredentialCapability::ProductListingsWrite]),
            },
            ProductListingIngestionActor::Service(id) if !id.is_empty() && !id.contains('\0') => {
                Principal::Service(id.clone())
            }
            ProductListingIngestionActor::System => Principal::System,
            ProductListingIngestionActor::Service(_) => {
                return Err(ProductListingIngestionError::InvalidMetadata);
            }
        };
        let context = OperationContext {
            principal,
            request_id: metadata.request_id.clone(),
            correlation_id: metadata.correlation_id.clone(),
        };

        let create = CreateProductListingHandler::new(
            self.unit_of_work.clone(),
            self.products.clone(),
            self.events.clone(),
            self.authorizer.clone(),
            self.auction_references.clone(),
        );
        let update = UpdateProductListingHandler::new(
            self.unit_of_work.clone(),
            self.products.clone(),
            self.events.clone(),
            self.authorizer.clone(),
            self.auction_references.clone(),
        );
        let upsert = UpsertProductListingHandler::new(
            self.unit_of_work.clone(),
            self.products.clone(),
            self.events.clone(),
            self.authorizer.clone(),
            self.auction_references.clone(),
        );
        let withdraw = WithdrawProductListingHandler::new(
            self.unit_of_work.clone(),
            self.products.clone(),
            self.events.clone(),
            self.authorizer.clone(),
        );
        let delivery = ChangeProductListingDeliveryHandler::new(
            self.unit_of_work.clone(),
            self.products.clone(),
            self.events.clone(),
            self.delivery_references.clone(),
        );
        let listing_id = ProductListingId::new();
        let mut slug_attempts = 0;
        let mut source_race_retried = false;
        loop {
            let mut tx = self.unit_of_work.begin().await.map_err(|source| {
                ProductListingIngestionError::BeginTransactionFailed(Box::new(source))
            })?;
            let receipt = self
                .receipts
                .in_transaction(&mut tx)
                .lock_and_find(metadata.command_id.as_str())
                .await
                .map_err(ProductListingIngestionError::Receipt)?;
            if let Some(receipt) = receipt {
                if receipt.fingerprint != envelope.fingerprint
                    || receipt.operation != metadata.operation
                    || receipt.listing_source_id != metadata.listing_source_id
                {
                    return Err(ProductListingIngestionError::FingerprintConflict);
                }
                if receipt.completion_code != ProductListingCommandCompletionCode::Applied {
                    return Err(ProductListingIngestionError::InvalidMetadata);
                }
                if let ProductListingIngestionIntent::ChangeDelivery(command) = intent
                    && command.import.is_some()
                {
                    // The outer queue receipt is not an authority grant. Recheck current
                    // source/grant/connection fences even for already committed imports.
                    delivery
                        .apply_in_transaction(&mut tx, &context, command)
                        .await?;
                }
                tx.commit().await.map_err(|source| {
                    ProductListingIngestionError::CommitTransactionFailed(Box::new(source))
                })?;
                return Ok(ProductListingIngestionCompletion::AlreadyCompleted);
            }

            // No separately committing use case may participate here. These operations only
            // borrow the handler-owned transaction; a race drops it before the next lookup.
            let effect = match intent {
                ProductListingIngestionIntent::ChangeDelivery(command) => {
                    ProductListingIngestionEffect::DeliveryChanged(
                        delivery
                            .apply_in_transaction(&mut tx, &context, command)
                            .await?,
                    )
                }
                ProductListingIngestionIntent::Create(command) => {
                    let slug = create.generate_title_slug_id(command)?;
                    match create
                        .apply_in_transaction(&mut tx, &context, command, listing_id, slug)
                        .await
                    {
                        Ok(result) => ProductListingIngestionEffect::Created(result),
                        Err(CreateProductListingApplyError::SlugCollision) => {
                            slug_attempts += 1;
                            if slug_attempts < MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS {
                                continue;
                            }
                            return Err(CreateProductListingError::ProductListingTitleSlugGenerationExhausted.into());
                        }
                        Err(CreateProductListingApplyError::Failed(error)) => {
                            return Err(error.into());
                        }
                    }
                }
                ProductListingIngestionIntent::Update {
                    product_key,
                    command,
                } => ProductListingIngestionEffect::Updated(
                    update
                        .apply_in_tx(
                            &mut tx,
                            &context,
                            UpdateTarget::Key(product_key.clone()),
                            command.clone(),
                        )
                        .await?,
                ),
                ProductListingIngestionIntent::Upsert(command) => {
                    match upsert
                        .apply_in_transaction(&mut tx, &context, command, listing_id)
                        .await
                    {
                        Ok(result) => ProductListingIngestionEffect::Upserted(result),
                        Err(UpsertProductListingApplyError::SourceRace) if !source_race_retried => {
                            source_race_retried = true;
                            continue;
                        }
                        Err(UpsertProductListingApplyError::SourceRace) => {
                            return Err(UpsertProductListingError::ConcurrencyConflict.into());
                        }
                        Err(UpsertProductListingApplyError::SlugCollision) => {
                            slug_attempts += 1;
                            if slug_attempts < MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS {
                                continue;
                            }
                            return Err(UpsertProductListingError::ProductListingTitleSlugGenerationExhausted.into());
                        }
                        Err(UpsertProductListingApplyError::Failed(error)) => {
                            return Err(error.into());
                        }
                    }
                }
                ProductListingIngestionIntent::Withdraw(key) => {
                    let (result, _) = withdraw
                        .apply_in_tx(&mut tx, &context, WithdrawTarget::Key(key.clone()))
                        .await?;
                    ProductListingIngestionEffect::Withdrawn(result)
                }
                ProductListingIngestionIntent::CaptureRaw(command) => {
                    ProductListingIngestionEffect::RawCaptured(
                        capture_in_transaction(
                            &context,
                            command.clone(),
                            &mut tx,
                            &self.raw_writer,
                            &self.authorizer,
                        )
                        .await?,
                    )
                }
            };
            self.receipts
                .in_transaction(&mut tx)
                .insert(&ProductListingCommandReceiptWrite {
                    command_id: metadata.command_id.as_str().to_owned(),
                    submission_id: metadata.submission_id.as_str().to_owned(),
                    listing_source_id: metadata.listing_source_id,
                    operation: metadata.operation,
                    fingerprint: envelope.fingerprint,
                })
                .await
                .map_err(ProductListingIngestionError::Receipt)?;
            tx.commit().await.map_err(|source| {
                ProductListingIngestionError::CommitTransactionFailed(Box::new(source))
            })?;
            return Ok(ProductListingIngestionCompletion::Applied(effect));
        }
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    use application::error::{StaticError, box_error};

    #[test]
    fn should_classify_each_failure_by_how_the_ordered_consumer_settles_it() {
        let rejections = [
            ProductListingIngestionError::Create(
                CreateProductListingError::SourceListingAlreadyExists,
            ),
            ProductListingIngestionError::Update(UpdateProductListingError::NotFound),
            ProductListingIngestionError::Update(UpdateProductListingError::ListingWithdrawn),
            ProductListingIngestionError::Upsert(UpsertProductListingError::Forbidden),
            ProductListingIngestionError::Withdraw(WithdrawProductListingError::NotFound),
            ProductListingIngestionError::Delivery(ProductListingDeliveryError::StaleRevision),
            ProductListingIngestionError::Delivery(ProductListingDeliveryError::Import(
                shipping_service::ShippingError::ImportOwnershipConflict,
            )),
            ProductListingIngestionError::CaptureRaw(
                CaptureProductListingRawObservationError::ProviderSourceOrderAmbiguous,
            ),
            ProductListingIngestionError::CaptureRaw(
                CaptureProductListingRawObservationError::ListingSourceNotFound,
            ),
        ];
        let unprocessable = [
            ProductListingIngestionError::InvalidMetadata,
            ProductListingIngestionError::FingerprintConflict,
            ProductListingIngestionError::Receipt(
                ProductListingCommandReceiptError::InvalidIdentity,
            ),
            ProductListingIngestionError::Receipt(
                ProductListingCommandReceiptError::InvalidPersistedState,
            ),
            ProductListingIngestionError::Update(
                UpdateProductListingError::InvalidDeliveryPersisted {
                    source: box_error(StaticError("undecodable delivery")),
                },
            ),
            ProductListingIngestionError::CaptureRaw(
                CaptureProductListingRawObservationError::SourceRecordKeyHashCollision,
            ),
            ProductListingIngestionError::CaptureRaw(
                CaptureProductListingRawObservationError::ProviderReceiptDigestConflict,
            ),
            ProductListingIngestionError::Delivery(ProductListingDeliveryError::Import(
                shipping_service::ShippingError::ImportRequestConflict,
            )),
            ProductListingIngestionError::Delivery(ProductListingDeliveryError::Import(
                shipping_service::ShippingError::CreationKeyConflict,
            )),
            ProductListingIngestionError::Delivery(ProductListingDeliveryError::Import(
                shipping_service::ShippingError::CorruptShippingState {
                    source: box_error(StaticError("undecodable shipping state")),
                },
            )),
        ];
        let retryable = [
            ProductListingIngestionError::CommitTransactionFailed(
                application::error::static_error("test transaction failure"),
            ),
            ProductListingIngestionError::Receipt(
                ProductListingCommandReceiptError::OperationFailed {
                    source: box_error(StaticError("unavailable")),
                },
            ),
            ProductListingIngestionError::Create(CreateProductListingError::PersistenceFailed {
                source: box_error(StaticError("unavailable")),
            }),
            ProductListingIngestionError::Update(
                UpdateProductListingError::PartnerAuthorizationTemporarilyUnavailable {
                    source: box_error(StaticError("unavailable")),
                },
            ),
            ProductListingIngestionError::Delivery(ProductListingDeliveryError::Import(
                shipping_service::ShippingError::ImportDependencyMissing,
            )),
        ];

        let contended = [
            ProductListingIngestionError::Receipt(ProductListingCommandReceiptError::AlreadyExists),
            ProductListingIngestionError::Create(
                CreateProductListingError::ProductListingTitleSlugAlreadyExists,
            ),
            ProductListingIngestionError::Update(UpdateProductListingError::ConcurrencyConflict),
            ProductListingIngestionError::Upsert(
                UpsertProductListingError::ProductListingTitleSlugGenerationExhausted,
            ),
            ProductListingIngestionError::Withdraw(
                WithdrawProductListingError::ConcurrencyConflict,
            ),
        ];

        for (class, errors) in [
            (
                ProductListingIngestionFailureClass::BusinessRejection,
                &rejections[..],
            ),
            (
                ProductListingIngestionFailureClass::Unprocessable,
                &unprocessable[..],
            ),
            (
                ProductListingIngestionFailureClass::Retryable,
                &retryable[..],
            ),
            (
                ProductListingIngestionFailureClass::Contended,
                &contended[..],
            ),
        ] {
            for error in errors {
                assert_eq!(error.failure_class(), class, "{}", error.diagnostic_code());
            }
        }
    }

    #[test]
    fn ingestion_diagnostic_codes_distinguish_receipt_transaction_and_business_failures() {
        let cases = [
            (
                ProductListingIngestionError::InvalidMetadata,
                "INVALID_METADATA",
            ),
            (
                ProductListingIngestionError::FingerprintConflict,
                "FINGERPRINT_CONFLICT",
            ),
            (
                ProductListingIngestionError::BeginTransactionFailed(
                    application::error::static_error("test transaction failure"),
                ),
                "BEGIN_TRANSACTION_FAILED",
            ),
            (
                ProductListingIngestionError::CommitTransactionFailed(
                    application::error::static_error("test transaction failure"),
                ),
                "COMMIT_UNCONFIRMED",
            ),
            (
                ProductListingIngestionError::Receipt(
                    ProductListingCommandReceiptError::InvalidIdentity,
                ),
                "RECEIPT_INVALID_IDENTITY",
            ),
            (
                ProductListingIngestionError::Receipt(
                    ProductListingCommandReceiptError::AlreadyExists,
                ),
                "RECEIPT_ALREADY_EXISTS",
            ),
            (
                ProductListingIngestionError::Receipt(
                    ProductListingCommandReceiptError::InvalidPersistedState,
                ),
                "RECEIPT_INVALID_PERSISTED_STATE",
            ),
            (
                ProductListingIngestionError::Receipt(
                    ProductListingCommandReceiptError::OperationFailed {
                        source: box_error(StaticError("secret")),
                    },
                ),
                "RECEIPT_OPERATION_FAILED",
            ),
            (
                ProductListingIngestionError::Create(
                    CreateProductListingError::SourceListingAlreadyExists,
                ),
                "CREATE_SOURCE_LISTING_ALREADY_EXISTS",
            ),
            (
                ProductListingIngestionError::Update(UpdateProductListingError::NotFound),
                "UPDATE_NOT_FOUND",
            ),
            (
                ProductListingIngestionError::Update(UpdateProductListingError::Forbidden),
                "UPDATE_FORBIDDEN",
            ),
            (
                ProductListingIngestionError::Upsert(
                    UpsertProductListingError::InvalidProductListing {
                        source: box_error(StaticError("secret")),
                    },
                ),
                "UPSERT_INVALID_PRODUCT_LISTING",
            ),
            (
                ProductListingIngestionError::Withdraw(WithdrawProductListingError::Forbidden),
                "WITHDRAW_FORBIDDEN",
            ),
            (
                ProductListingIngestionError::CaptureRaw(
                    CaptureProductListingRawObservationError::ProviderReceiptDigestConflict,
                ),
                "CAPTURE_RAW_PROVIDER_RECEIPT_DIGEST_CONFLICT",
            ),
        ];
        for (error, expected) in cases {
            let code = error.diagnostic_code();
            assert_eq!(code, expected);
            assert!(!code.contains("secret"));
            assert!(
                code.bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
            );
        }
    }
}
