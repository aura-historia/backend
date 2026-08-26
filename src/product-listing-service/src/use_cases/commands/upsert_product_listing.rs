use crate::ports::{
    PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
    PartnerProductListingAuthorizerFactory, ProductListingEventStore,
    ProductListingEventStoreError, ProductListingEventStoreFactory, ProductListingRepository,
    ProductListingRepositoryError, ProductListingRepositoryFactory, stamp_product_listing_events,
};
use crate::use_cases::{CreateProductListingResult, UpdateProductListingResult};
use application::error::{BoxError, box_error};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use application::patch_field::PatchField;
use application::transaction::{Transaction, UnitOfWork};

use indexmap::IndexSet;
use localization::{Language, Localized};
use money::Price;
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing::{
    ChangeListingAvailabilityError, ChangeProductListingError, NewProductListing, ProductListing,
    ProductListingAddress, ProductListingAuction, ProductListingPricing,
    RehydrateProductListingError,
};
use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::shop_listing_id::ShopListingId;
use product_listing_core::title::Title;
use shop_core::shop_id::ShopId;
use url::Url;
use user_core::user_id::UserId;

const MISSING_PRODUCT_URL: &str = "https://not-provided.invalid";
#[derive(Debug, Clone, PartialEq)]
pub struct UpsertProductListingCommand {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shop_listing_id: ShopListingId,
    pub address: ProductListingAddress,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub price: Option<Price>,
    pub price_estimate_min: Option<Price>,
    pub price_estimate_max: Option<Price>,
    pub availability: PatchField<ListingAvailability>,
    pub url: Option<Url>,
    pub images: IndexSet<ProductListingImage>,
    pub auction_start: Option<time::OffsetDateTime>,
    pub auction_end: Option<time::OffsetDateTime>,
}
#[derive(Debug, Clone, PartialEq)]
pub enum UpsertProductListingResult {
    Created(CreateProductListingResult),
    Updated(UpdateProductListingResult),
}
#[derive(Debug, thiserror::Error)]
pub enum UpsertProductListingError {
    #[error("authenticated actor required to upsert product listing")]
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
    #[error("product listing is withdrawn")]
    ListingWithdrawn,
    #[error("product listing is invalid")]
    InvalidProductListing {
        #[source]
        source: BoxError,
    },
    #[error("product listing persistence failed")]
    PersistenceFailed,
    #[error("product listing event storage failed")]
    EventStoreFailed,
    #[error("failed to begin upsert product listing transaction")]
    BeginTransactionFailed,
    #[error("failed to commit upsert product listing transaction")]
    CommitTransactionFailed,
}
#[async_trait::async_trait]
pub trait UpsertProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpsertProductListingCommand,
    ) -> Result<UpsertProductListingResult, UpsertProductListingError>;
}
pub struct UpsertProductListingHandler<U, R, E, A> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
}

enum UpsertAttemptError {
    ShopListingInsertRace,
    Failed(UpsertProductListingError),
}

impl From<UpsertProductListingError> for UpsertAttemptError {
    fn from(error: UpsertProductListingError) -> Self {
        Self::Failed(error)
    }
}

impl From<ProductListingRepositoryError> for UpsertAttemptError {
    fn from(error: ProductListingRepositoryError) -> Self {
        Self::Failed(error.into())
    }
}

impl From<ProductListingEventStoreError> for UpsertAttemptError {
    fn from(error: ProductListingEventStoreError) -> Self {
        Self::Failed(error.into())
    }
}

impl From<PartnerProductListingAuthorizationError> for UpsertAttemptError {
    fn from(error: PartnerProductListingAuthorizationError) -> Self {
        Self::Failed(error.into())
    }
}

impl From<ChangeListingAvailabilityError> for UpsertAttemptError {
    fn from(error: ChangeListingAvailabilityError) -> Self {
        Self::Failed(error.into())
    }
}

impl From<ChangeProductListingError> for UpsertAttemptError {
    fn from(error: ChangeProductListingError) -> Self {
        Self::Failed(error.into())
    }
}

impl From<RehydrateProductListingError> for UpsertAttemptError {
    fn from(error: RehydrateProductListingError) -> Self {
        Self::Failed(error.into())
    }
}
impl<U, R, E, A> UpsertProductListingHandler<U, R, E, A> {
    pub fn new(unit_of_work: U, products: R, events: E, authorizer: A) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            authorizer,
        }
    }
}
impl<U, R, E, A> UpsertProductListingHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
{
    async fn persist(
        &self,
        tx: &mut U::Tx,
        context: &OperationContext,
        command: UpsertProductListingCommand,
    ) -> Result<UpsertProductListingResult, UpsertAttemptError> {
        if let Some(actor_id) = partner_actor(&context.principal) {
            self.authorizer
                .in_transaction(tx)
                .authorize(actor_id, command.shop_id)
                .await?;
        }
        let key = ProductListingKey::new(command.shop_id, command.shop_listing_id.clone());
        let existing = self.products.in_transaction(tx).find_by_key(&key).await?;
        match existing {
            Some(loaded) => {
                let expected_event_id = loaded.version;
                let mut product = loaded.value;
                product.restore();
                apply_update(&mut product, &command)?;
                let events = stamp_product_listing_events(
                    product.id(),
                    time::OffsetDateTime::now_utc(),
                    product.take_pending_event_payloads(),
                );
                let event_id = events.last().map(|event| event.event_id);
                if let Some(new_event_id) = event_id {
                    product = self
                        .products
                        .in_transaction(tx)
                        .update(&product, expected_event_id, new_event_id)
                        .await?
                        .value;
                    for event in &events {
                        self.events.in_transaction(tx).append(event).await?;
                    }
                }
                Ok(UpsertProductListingResult::Updated(
                    UpdateProductListingResult {
                        product_listing_id: product.id(),
                        event_id,
                    },
                ))
            }
            None => {
                let mut product =
                    ProductListing::create(command.into_new_product(ProductListingId::new())?)?;
                let events = stamp_product_listing_events(
                    product.id(),
                    time::OffsetDateTime::now_utc(),
                    product.take_pending_event_payloads(),
                );
                let event_id = events.last().map(|event| event.event_id).ok_or_else(|| {
                    UpsertProductListingError::InvalidProductListing {
                        source: box_error(std::io::Error::other("created listing has no event")),
                    }
                })?;
                let persisted = self
                    .products
                    .in_transaction(tx)
                    .insert(&product, event_id)
                    .await
                    .map_err(|error| match error {
                        ProductListingRepositoryError::ShopListingAlreadyExists => {
                            UpsertAttemptError::ShopListingInsertRace
                        }
                        error => UpsertAttemptError::Failed(error.into()),
                    })?;
                for event in &events {
                    self.events.in_transaction(tx).append(event).await?;
                }
                Ok(UpsertProductListingResult::Created(
                    CreateProductListingResult {
                        product_listing_id: persisted.value.id(),
                        product_listing_slug_id: persisted.value.slug_id().clone(),
                        event_id,
                    },
                ))
            }
        }
    }

    async fn execute_attempt(
        &self,
        context: &OperationContext,
        command: UpsertProductListingCommand,
    ) -> Result<UpsertProductListingResult, UpsertAttemptError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductListingsWrite)
            .authorize::<UpsertProductListingError>()?;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpsertProductListingError::BeginTransactionFailed)?;
        let result = self.persist(&mut tx, context, command).await?;
        tx.commit()
            .await
            .map_err(|_| UpsertProductListingError::CommitTransactionFailed)?;
        Ok(result)
    }
}
#[async_trait::async_trait]
impl<U, R, E, A> UpsertProductListingUseCase for UpsertProductListingHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
{
    #[tracing::instrument(name = "upsert_product_listing", skip_all, fields(shop_id = %command.shop_id, shop_listing_id = %command.shop_listing_id, principal_type = context.principal.kind(), actor_id = tracing::field::Empty, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpsertProductListingCommand,
    ) -> Result<UpsertProductListingResult, UpsertProductListingError> {
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );
        let result = match self.execute_attempt(context, command.clone()).await {
            Ok(result) => result,
            Err(UpsertAttemptError::ShopListingInsertRace) => {
                match self.execute_attempt(context, command).await {
                    Ok(result) => result,
                    Err(UpsertAttemptError::ShopListingInsertRace) => {
                        return Err(UpsertProductListingError::PersistenceFailed);
                    }
                    Err(UpsertAttemptError::Failed(error)) => return Err(error),
                }
            }
            Err(UpsertAttemptError::Failed(error)) => return Err(error),
        };
        let product_listing_id = match &result {
            UpsertProductListingResult::Created(value) => value.product_listing_id,
            UpsertProductListingResult::Updated(value) => value.product_listing_id,
        };
        tracing::info!(event = "product_listing.upserted", actor_type = context.principal.kind(), actor_id = %context.principal.label(), product_listing_id = %product_listing_id, outcome = "success");
        Ok(result)
    }
}
impl UpsertProductListingCommand {
    fn into_new_product(
        self,
        id: ProductListingId,
    ) -> Result<NewProductListing, UpsertProductListingError> {
        let url = match self.url {
            Some(url) => url,
            None => Url::parse(MISSING_PRODUCT_URL).map_err(|error| {
                UpsertProductListingError::InvalidProductListing {
                    source: box_error(error),
                }
            })?,
        };
        Ok(NewProductListing {
            id,
            shop_id: self.shop_id,
            seller_id: self.seller_id,
            shop_listing_id: self.shop_listing_id,
            address: self.address,
            title: self
                .title
                .or_else(|| Some(Localized::new(Language::En, Title::from("")))),
            description: self.description,
            pricing: ProductListingPricing {
                price: self.price,
                price_estimate_min: self.price_estimate_min,
                price_estimate_max: self.price_estimate_max,
            },
            availability: match self.availability {
                PatchField::Unchanged | PatchField::Clear => None,
                PatchField::Set(availability) => Some(availability),
            },
            url,
            images: self.images,
            auction: ProductListingAuction {
                start: self.auction_start,
                end: self.auction_end,
            },
        })
    }
}
fn apply_update(
    product: &mut ProductListing,
    command: &UpsertProductListingCommand,
) -> Result<(), UpsertProductListingError> {
    let pricing = ProductListingPricing {
        price: command.price.or(product.pricing().price),
        price_estimate_min: command
            .price_estimate_min
            .or(product.pricing().price_estimate_min),
        price_estimate_max: command
            .price_estimate_max
            .or(product.pricing().price_estimate_max),
    };
    product.replace_pricing(pricing)?;
    match command.availability {
        PatchField::Unchanged => {}
        PatchField::Set(availability) => {
            product.set_availability(availability)?;
        }
        PatchField::Clear => {
            product.clear_availability()?;
        }
    }
    if let Some(url) = &command.url {
        product.change_url(url.clone())?;
    }
    product.replace_images(command.images.clone())?;
    if command.auction_start.is_some() || command.auction_end.is_some() {
        let mut auction = product.auction();
        if let Some(value) = command.auction_start {
            auction.start = Some(value);
        }
        if let Some(value) = command.auction_end {
            auction.end = Some(value);
        }
        product.replace_auction(auction)?;
    }
    Ok(())
}
fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => Some(*id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}
impl From<OperationAuthorizationError> for UpsertProductListingError {
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
impl From<PartnerProductListingAuthorizationError> for UpsertProductListingError {
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
impl From<ChangeListingAvailabilityError> for UpsertProductListingError {
    fn from(_: ChangeListingAvailabilityError) -> Self {
        Self::ListingWithdrawn
    }
}
impl From<ChangeProductListingError> for UpsertProductListingError {
    fn from(error: ChangeProductListingError) -> Self {
        match error {
            ChangeProductListingError::ListingWithdrawn => Self::ListingWithdrawn,
            ChangeProductListingError::GeoLatitudeOutOfRange
            | ChangeProductListingError::GeoLongitudeOutOfRange
            | ChangeProductListingError::AuctionStartAfterEnd => Self::InvalidProductListing {
                source: box_error(error),
            },
        }
    }
}
impl From<RehydrateProductListingError> for UpsertProductListingError {
    fn from(error: RehydrateProductListingError) -> Self {
        Self::InvalidProductListing {
            source: box_error(error),
        }
    }
}
impl From<ProductListingRepositoryError> for UpsertProductListingError {
    fn from(_: ProductListingRepositoryError) -> Self {
        Self::PersistenceFailed
    }
}
impl From<ProductListingEventStoreError> for UpsertProductListingError {
    fn from(_: ProductListingEventStoreError) -> Self {
        Self::EventStoreFailed
    }
}
