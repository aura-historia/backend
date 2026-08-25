use crate::ports::{
    PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
    PartnerProductListingAuthorizerFactory, ProductListingEventStore,
    ProductListingEventStoreError, ProductListingEventStoreFactory, ProductListingRepository,
    ProductListingRepositoryError, ProductListingRepositoryFactory, stamp_product_listing_events,
};
use application::error::BoxError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use indexmap::IndexSet;
use localization::{Language, Localized};
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing::{
    NewProductListing, ProductListing, ProductListingAddress, ProductListingAuction,
    ProductListingPricing, RehydrateProductListingError,
};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_core::shop_listing_id::ShopListingId;
use product_listing_core::title::Title;
use shop_core::shop_id::ShopId;
use url::Url;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateProductListingCommand {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shop_listing_id: ShopListingId,
    pub address: ProductListingAddress,
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
    pub product_listing_slug_id: ProductListingSlugId,
    pub event_id: EventId,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateProductListingError {
    #[error("authenticated actor required to create product listing")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("shop not found")]
    ShopNotFound,
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
    #[error("product listing already exists for shop listing identity")]
    ShopListingAlreadyExists,
    #[error("product listing slug already exists")]
    ProductListingSlugAlreadyExists,
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

#[async_trait::async_trait]
impl<U, R, E, A> CreateProductListingUseCase for CreateProductListingHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
{
    #[tracing::instrument(name = "create_product_listing", skip_all, fields(shop_id = %command.shop_id, shop_listing_id = %command.shop_listing_id, principal_type = context.principal.kind(), actor_id = tracing::field::Empty, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateProductListingCommand,
    ) -> Result<CreateProductListingResult, CreateProductListingError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductsWrite)
            .authorize::<CreateProductListingError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CreateProductListingError::BeginTransactionFailed)?;
        if let Some(actor_id) = partner_actor(&context.principal) {
            self.authorizer
                .in_transaction(&mut tx)
                .authorize(actor_id, command.shop_id)
                .await?;
        }

        let mut product =
            ProductListing::create(command.into_new_product(ProductListingId::new()))?;
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
            product_listing_slug_id: persisted.value.slug_id().clone(),
            event_id,
        })
    }
}

impl CreateProductListingCommand {
    fn into_new_product(self, id: ProductListingId) -> NewProductListing {
        NewProductListing {
            id,
            shop_id: self.shop_id,
            seller_id: self.seller_id,
            shop_listing_id: self.shop_listing_id,
            address: self.address,
            title: self.title,
            description: self.description,
            pricing: self.pricing,
            sale_valuation: None,
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
            PartnerProductListingAuthorizationError::ShopNotFound => Self::ShopNotFound,
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
            ProductListingRepositoryError::ShopListingAlreadyExists => {
                Self::ShopListingAlreadyExists
            }
            ProductListingRepositoryError::ProductListingSlugAlreadyExists => {
                Self::ProductListingSlugAlreadyExists
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
