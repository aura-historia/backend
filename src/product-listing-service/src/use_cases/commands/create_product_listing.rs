use crate::{
    ports::{
        PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
        PartnerProductListingAuthorizerFactory, ProductListingEventAppendError,
        ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
        ProductListingRepositoryError, ProductListingRepositoryFactory,
        stamp_product_listing_event,
    },
    product_listing_auction_patch::{
        ProductListingAuctionPatch, compose_product_listing_auction_patch,
        validate_product_listing_auction_patch,
    },
    product_listing_title_slug_creation::{
        MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS, ProductListingTitleSlugGenerator,
        RandomProductListingTitleSlugGenerator, TitleSlugCollisionRetry,
        title_slug_collision_retry,
    },
};
use application::{
    error::BoxError,
    operation_context::{
        CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
    },
    transaction::{Transaction, UnitOfWork},
};
use auction_service::ports::{
    AuctionReferenceValidationError, AuctionReferenceValidator, AuctionReferenceValidatorFactory,
};

use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use product_listing_core::{
    description::Description,
    listing_availability::ListingAvailability,
    product_listing::{
        NewProductListing, ProductListing, ProductListingAuction, ProductListingPricing,
        RehydrateProductListingError,
    },
    product_listing_id::ProductListingId,
    product_listing_image::ProductListingImage,
    product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId,
    title::Title,
};
use url::Url;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateProductListingCommand {
    pub listing_source_id: ListingSourceId,
    pub source_listing_id: SourceListingId,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductListingPricing,
    pub availability: Option<ListingAvailability>,
    pub url: Url,
    pub images: IndexSet<ProductListingImage>,
    pub auction: Option<ProductListingAuctionPatch>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateProductListingResult {
    pub product_listing_id: ProductListingId,
    pub product_listing_title_slug_id: ProductListingSlugId,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateProductListingError {
    #[error("authenticated actor required to create product listing")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("listing source not found")]
    ListingSourceNotFound,
    #[error("partner product listing authorization is temporarily unavailable")]
    PartnerAuthorizationTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("partner product listing authorization failed internally")]
    PartnerAuthorizationInternal {
        #[source]
        source: BoxError,
    },
    #[error("auction not found")]
    AuctionNotFound,
    #[error("auction belongs to another listing source")]
    AuctionSourceMismatch,
    #[error("auction reference validation is temporarily unavailable")]
    AuctionReferenceTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("product listing already exists for source listing identity")]
    SourceListingAlreadyExists,
    #[error("product listing title slug already exists")]
    ProductListingTitleSlugAlreadyExists,
    #[error("product listing title slug generation was exhausted")]
    ProductListingTitleSlugGenerationExhausted,
    #[error("new product listing is invalid")]
    InvalidProductListing,
    #[error("created product listing did not record a domain event")]
    CreatedEventMissing,
    #[error("product listing persistence failed")]
    PersistenceFailed,
    #[error("product listing event append failed")]
    EventAppenderFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin create product listing transaction")]
    BeginTransactionFailed,
    #[error("failed to commit create product listing transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait CreateProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateProductListingCommand,
    ) -> Result<CreateProductListingResult, CreateProductListingError>;
}

pub struct CreateProductListingHandler<U, R, E, A, V, G = RandomProductListingTitleSlugGenerator> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
    auction_references: V,
    title_slug_generator: G,
}

impl<U, R, E, A, V> CreateProductListingHandler<U, R, E, A, V> {
    pub fn new(
        unit_of_work: U,
        products: R,
        events: E,
        authorizer: A,
        auction_references: V,
    ) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            authorizer,
            auction_references,
            title_slug_generator: RandomProductListingTitleSlugGenerator,
        }
    }
}

impl<U, R, E, A, V, G> CreateProductListingHandler<U, R, E, A, V, G>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    V: AuctionReferenceValidatorFactory<U::Tx>,
    G: ProductListingTitleSlugGenerator,
{
    async fn persist_attempt(
        &self,
        context: &OperationContext,
        command: &CreateProductListingCommand,
        product_listing_id: ProductListingId,
        title_slug_id: ProductListingSlugId,
    ) -> Result<CreateProductListingResult, CreateProductListingError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CreateProductListingError::BeginTransactionFailed)?;
        if let Some(actor_id) = partner_actor(&context.principal) {
            self.authorizer
                .in_transaction(&mut tx)
                .authorize(actor_id, command.listing_source_id)
                .await?;
        }
        let auction = command
            .auction
            .as_ref()
            .map(|patch| {
                validate_product_listing_auction_patch(None, patch)
                    .and_then(|_| compose_product_listing_auction_patch(None, patch))
            })
            .transpose()
            .map_err(|_| CreateProductListingError::InvalidProductListing)?
            .flatten();
        if let Some(auction_id) = auction.as_ref().and_then(ProductListingAuction::auction_id) {
            self.auction_references
                .in_transaction(&mut tx)
                .validate(auction_id, command.listing_source_id)
                .await?;
        }
        let mut product = ProductListing::create(NewProductListing {
            id: product_listing_id,
            title_slug_id,
            listing_source_id: command.listing_source_id,
            source_listing_id: command.source_listing_id.clone(),
            title: command.title.clone(),
            description: command.description.clone(),
            pricing: command.pricing,
            availability: command.availability,
            url: command.url.clone(),
            images: command.images.clone(),
            auction,
        })?;
        let event = stamp_product_listing_event(
            product.id(),
            time::OffsetDateTime::now_utc(),
            product
                .take_pending_event_payload()
                .ok_or(CreateProductListingError::CreatedEventMissing)?,
        );
        let event_id = event.event_id;
        let persisted = self
            .products
            .in_transaction(&mut tx)
            .insert(&product, event_id)
            .await?;
        self.events.in_transaction(&mut tx).append(&event).await?;
        tx.commit()
            .await
            .map_err(|_| CreateProductListingError::CommitTransactionFailed)?;
        Ok(CreateProductListingResult {
            product_listing_id: persisted.value.id(),
            product_listing_title_slug_id: persisted.value.title_slug_id().clone(),
        })
    }
}

#[async_trait::async_trait]
impl<U, R, E, A, V, G> CreateProductListingUseCase for CreateProductListingHandler<U, R, E, A, V, G>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    V: AuctionReferenceValidatorFactory<U::Tx>,
    G: ProductListingTitleSlugGenerator,
{
    #[tracing::instrument(name = "create_product_listing", skip_all, fields(listing_source_id = %command.listing_source_id, source_listing_id = %command.source_listing_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateProductListingCommand,
    ) -> Result<CreateProductListingResult, CreateProductListingError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductListingsWrite)
            .authorize::<CreateProductListingError>()?;
        let product_listing_id = ProductListingId::new();
        for attempt in 1..=MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS {
            let title_slug_id = self
                .title_slug_generator
                .generate(
                    command
                        .title
                        .as_ref()
                        .map_or("", |title| title.payload.as_ref()),
                )
                .map_err(|_| CreateProductListingError::InvalidProductListing)?;
            match self
                .persist_attempt(context, &command, product_listing_id, title_slug_id)
                .await
            {
                Ok(result) => return Ok(result),
                Err(CreateProductListingError::ProductListingTitleSlugAlreadyExists)
                    if title_slug_collision_retry(attempt, true)
                        == TitleSlugCollisionRetry::Retry =>
                {
                    continue;
                }
                Err(CreateProductListingError::ProductListingTitleSlugAlreadyExists) => {
                    return Err(
                        CreateProductListingError::ProductListingTitleSlugGenerationExhausted,
                    );
                }
                Err(error) => return Err(error),
            }
        }
        Err(CreateProductListingError::ProductListingTitleSlugGenerationExhausted)
    }
}

fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => Some(*id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}
impl From<OperationAuthorizationError> for CreateProductListingError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}
impl From<PartnerProductListingAuthorizationError> for CreateProductListingError {
    fn from(error: PartnerProductListingAuthorizationError) -> Self {
        match error {
            PartnerProductListingAuthorizationError::ListingSourceNotFound => {
                Self::ListingSourceNotFound
            }
            PartnerProductListingAuthorizationError::Forbidden => Self::Forbidden,
            PartnerProductListingAuthorizationError::TemporarilyUnavailable { source } => {
                Self::PartnerAuthorizationTemporarilyUnavailable { source }
            }
            PartnerProductListingAuthorizationError::Internal { source } => {
                Self::PartnerAuthorizationInternal { source }
            }
        }
    }
}
impl From<AuctionReferenceValidationError> for CreateProductListingError {
    fn from(error: AuctionReferenceValidationError) -> Self {
        match error {
            AuctionReferenceValidationError::NotFound => Self::AuctionNotFound,
            AuctionReferenceValidationError::ListingSourceMismatch => Self::AuctionSourceMismatch,
            AuctionReferenceValidationError::TemporarilyUnavailable { source } => {
                Self::AuctionReferenceTemporarilyUnavailable { source }
            }
            AuctionReferenceValidationError::InvalidPersistedState { .. } => {
                Self::PersistenceFailed
            }
        }
    }
}
impl From<RehydrateProductListingError> for CreateProductListingError {
    fn from(_: RehydrateProductListingError) -> Self {
        Self::InvalidProductListing
    }
}
impl From<ProductListingRepositoryError> for CreateProductListingError {
    fn from(error: ProductListingRepositoryError) -> Self {
        match error {
            ProductListingRepositoryError::SourceListingAlreadyExists => {
                Self::SourceListingAlreadyExists
            }
            ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists => {
                Self::ProductListingTitleSlugAlreadyExists
            }
            _ => Self::PersistenceFailed,
        }
    }
}
impl From<ProductListingEventAppendError> for CreateProductListingError {
    fn from(error: ProductListingEventAppendError) -> Self {
        Self::EventAppenderFailed {
            source: Box::new(error),
        }
    }
}
