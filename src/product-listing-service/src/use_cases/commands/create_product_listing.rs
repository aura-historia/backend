use crate::ports::{
    PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
    PartnerProductListingAuthorizerFactory, ProductListingEventStore,
    ProductListingEventStoreError, ProductListingEventStoreFactory, ProductListingRepository,
    ProductListingRepositoryError, ProductListingRepositoryFactory, stamp_product_listing_events,
};
use crate::product_listing_title_slug_creation::{
    MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS, next_product_listing_title_slug,
};
use application::error::BoxError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing::{
    NewProductListing, ProductListing, ProductListingAuction, ProductListingPricing,
    RehydrateProductListingError,
};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_core::title::Title;
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
    pub auction: ProductListingAuction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateProductListingResult {
    pub product_listing_id: ProductListingId,
    pub product_listing_title_slug_id: ProductListingSlugId,
    pub event_id: EventId,
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
    #[error("product listing event storage failed")]
    EventStoreFailed,
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

pub struct CreateProductListingHandler<U, R, E, A> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
}

impl<U, R, E, A> CreateProductListingHandler<U, R, E, A> {
    pub fn new(unit_of_work: U, products: R, events: E, authorizer: A) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            authorizer,
        }
    }
}

impl<U, R, E, A> CreateProductListingHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
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

        let mut product = ProductListing::create_with_title_slug_id(
            command.clone().into_new_product(product_listing_id),
            title_slug_id,
        )?;
        let events = stamp_product_listing_events(
            product.id(),
            time::OffsetDateTime::now_utc(),
            product.take_pending_event_payloads(),
        );
        let event_id = events
            .last()
            .map(|event| event.event_id)
            .ok_or(CreateProductListingError::CreatedEventMissing)?;
        let persisted = self
            .products
            .in_transaction(&mut tx)
            .insert(&product, event_id)
            .await?;
        for event in &events {
            self.events.in_transaction(&mut tx).append(event).await?;
        }
        tx.commit()
            .await
            .map_err(|_| CreateProductListingError::CommitTransactionFailed)?;

        tracing::info!(event = "product_listing.created", actor_type = context.principal.kind(), actor_id = %context.principal.label(), product_listing_id = %persisted.value.id(), event_id = %event_id, outcome = "success");
        Ok(CreateProductListingResult {
            product_listing_id: persisted.value.id(),
            product_listing_title_slug_id: persisted.value.title_slug_id().clone(),
            event_id,
        })
    }
}

#[async_trait::async_trait]
impl<U, R, E, A> CreateProductListingUseCase for CreateProductListingHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
{
    #[tracing::instrument(name = "create_product_listing", skip_all, fields(listing_source_id = %command.listing_source_id, source_listing_id = %command.source_listing_id, principal_type = context.principal.kind(), actor_id = tracing::field::Empty, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateProductListingCommand,
    ) -> Result<CreateProductListingResult, CreateProductListingError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductListingsWrite)
            .authorize::<CreateProductListingError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let product_listing_id = ProductListingId::new();
        for attempt in 1..=MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS {
            let title_slug_id = next_product_listing_title_slug(command.title.as_ref())
                .map_err(|_| CreateProductListingError::InvalidProductListing)?;
            match self
                .persist_attempt(context, &command, product_listing_id, title_slug_id)
                .await
            {
                Ok(result) => return Ok(result),
                Err(CreateProductListingError::ProductListingTitleSlugAlreadyExists)
                    if attempt < MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS =>
                {
                    tracing::warn!(
                        product_listing_id = %product_listing_id,
                        attempt,
                        constraint_name = "product_listings_title_slug_unique",
                        "product listing title slug collision; regenerating"
                    );
                }
                Err(CreateProductListingError::ProductListingTitleSlugAlreadyExists) => {
                    tracing::error!(
                        product_listing_id = %product_listing_id,
                        attempt,
                        constraint_name = "product_listings_title_slug_unique",
                        "product listing title slug generation exhausted"
                    );
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

impl CreateProductListingCommand {
    fn into_new_product(self, id: ProductListingId) -> NewProductListing {
        NewProductListing {
            id,
            listing_source_id: self.listing_source_id,
            source_listing_id: self.source_listing_id,
            title: self.title,
            description: self.description,
            pricing: self.pricing,
            availability: self.availability,
            url: self.url,
            images: self.images,
            auction: self.auction,
        }
    }
}

fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}

impl From<RehydrateProductListingError> for CreateProductListingError {
    fn from(_: RehydrateProductListingError) -> Self {
        Self::InvalidProductListing
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

impl From<ProductListingEventStoreError> for CreateProductListingError {
    fn from(_: ProductListingEventStoreError) -> Self {
        Self::EventStoreFailed
    }
}
