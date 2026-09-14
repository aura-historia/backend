use crate::{
    ports::{
        PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
        PartnerProductListingAuthorizerFactory, ProductListingEventAppendError,
        ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
        ProductListingRepositoryError, ProductListingRepositoryFactory, ProductListingWriteEffects,
        stamp_product_listing_event,
    },
    product_listing_auction_patch::{
        ProductListingAuctionPatch, compose_product_listing_auction_patch,
        validate_product_listing_auction_patch,
    },
};
use application::{
    error::BoxError,
    operation_context::{
        CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
    },
    patch_field::PatchField,
    transaction::{Transaction, UnitOfWork},
};
use auction_service::ports::{
    AuctionReferenceValidationError, AuctionReferenceValidator, AuctionReferenceValidatorFactory,
};
use domain_primitives::change_outcome::ChangeOutcome;
use indexmap::IndexSet;
use money::Price;
use product_listing_core::{
    listing_availability::ListingAvailability,
    product_listing::{ChangeListingAvailabilityError, ChangeProductListingError, ProductListing},
    product_listing_id::{ProductListingId, ProductListingKey},
    product_listing_image::ProductListingImage,
    product_listing_price::ProductListingPrice,
};
use url::Url;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateProductListingCommand {
    pub price: PatchField<ProductListingPrice>,
    pub price_estimate_min: PatchField<Price>,
    pub price_estimate_max: PatchField<Price>,
    pub availability: PatchField<ListingAvailability>,
    pub url: PatchField<Url>,
    pub images: PatchField<IndexSet<ProductListingImage>>,
    /// Nested patches preserve omitted lot facts. `auctionId: null` clears membership only.
    pub auction: PatchField<ProductListingAuctionPatch>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateProductListingResult {
    pub product_listing_id: ProductListingId,
    pub outcome: ChangeOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateProductListingError {
    #[error("authenticated actor required to update product listing")]
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
    #[error("product listing not found")]
    NotFound,
    #[error("product listing is withdrawn")]
    ListingWithdrawn,
    #[error("product listing URL is required")]
    UrlRequired,
    #[error("product listing is invalid")]
    InvalidProductListing,
    #[error("product listing persistence failed")]
    PersistenceFailed,
    #[error("product listing event storage failed")]
    EventAppenderFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin update product listing transaction")]
    BeginTransactionFailed,
    #[error("failed to commit update product listing transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpdateProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        product_listing_id: ProductListingId,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError>;
    async fn execute_by_key(
        &self,
        context: &OperationContext,
        product_key: ProductListingKey,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError>;
}

pub struct UpdateProductListingHandler<U, R, E, A, V> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
    auction_references: V,
}
impl<U, R, E, A, V> UpdateProductListingHandler<U, R, E, A, V> {
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
        }
    }
}
enum UpdateTarget {
    Id(ProductListingId),
    Key(ProductListingKey),
}

impl<U, R, E, A, V> UpdateProductListingHandler<U, R, E, A, V>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    V: AuctionReferenceValidatorFactory<U::Tx>,
{
    async fn update(
        &self,
        context: &OperationContext,
        target: UpdateTarget,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductListingsWrite)
            .authorize::<UpdateProductListingError>()?;
        if matches!(command.url, PatchField::Clear) {
            return Err(UpdateProductListingError::UrlRequired);
        }
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateProductListingError::BeginTransactionFailed)?;
        let loaded = match target {
            UpdateTarget::Id(id) => self
                .products
                .in_transaction(&mut tx)
                .find_by_id(id)
                .await?
                .ok_or(UpdateProductListingError::NotFound)?,
            UpdateTarget::Key(key) => self
                .products
                .in_transaction(&mut tx)
                .find_by_key(&key)
                .await?
                .ok_or(UpdateProductListingError::NotFound)?,
        };
        if let Some(actor_id) = partner_actor(&context.principal) {
            self.authorizer
                .in_transaction(&mut tx)
                .authorize(actor_id, loaded.value.listing_source_id())
                .await?;
        }
        let mut product = loaded.value;
        let auction = match &command.auction {
            PatchField::Unchanged | PatchField::Clear => None,
            PatchField::Set(patch) => {
                validate_product_listing_auction_patch(product.auction(), patch)
                    .map_err(|_| UpdateProductListingError::InvalidProductListing)?;
                let auction = compose_product_listing_auction_patch(product.auction(), patch)
                    .map_err(|_| UpdateProductListingError::InvalidProductListing)?;
                if let Some(auction_id) = auction
                    .membership()
                    .map(|membership| membership.auction_id())
                {
                    self.auction_references
                        .in_transaction(&mut tx)
                        .validate(auction_id, product.listing_source_id())
                        .await?;
                }
                Some(auction)
            }
        };
        apply_command(&mut product, command, auction)?;
        let event = product.take_pending_event_payload().map(|payload| {
            stamp_product_listing_event(product.id(), time::OffsetDateTime::now_utc(), payload)
        });
        let outcome = if let Some(event) = event {
            let effects = ProductListingWriteEffects::from(&event.payload);
            self.products
                .in_transaction(&mut tx)
                .update(&product, loaded.version, event.event_id, effects)
                .await?;
            self.events.in_transaction(&mut tx).append(&event).await?;
            ChangeOutcome::Changed
        } else {
            ChangeOutcome::Unchanged
        };
        let id = product.id();
        tx.commit()
            .await
            .map_err(|_| UpdateProductListingError::CommitTransactionFailed)?;
        Ok(UpdateProductListingResult {
            product_listing_id: id,
            outcome,
        })
    }
}

#[async_trait::async_trait]
impl<U, R, E, A, V> UpdateProductListingUseCase for UpdateProductListingHandler<U, R, E, A, V>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    V: AuctionReferenceValidatorFactory<U::Tx>,
{
    async fn execute(
        &self,
        context: &OperationContext,
        product_listing_id: ProductListingId,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError> {
        self.update(context, UpdateTarget::Id(product_listing_id), command)
            .await
    }
    async fn execute_by_key(
        &self,
        context: &OperationContext,
        product_key: ProductListingKey,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError> {
        self.update(context, UpdateTarget::Key(product_key), command)
            .await
    }
}

fn apply_command(
    product: &mut ProductListing,
    command: UpdateProductListingCommand,
    auction: Option<product_listing_core::product_listing::ProductListingAuction>,
) -> Result<(), UpdateProductListingError> {
    let mut pricing = product.pricing();
    apply_optional_patch(&mut pricing.price, command.price);
    apply_optional_patch(&mut pricing.price_estimate_min, command.price_estimate_min);
    apply_optional_patch(&mut pricing.price_estimate_max, command.price_estimate_max);
    product.replace_pricing(pricing)?;
    match command.availability {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            product.set_availability(value)?;
        }
        PatchField::Clear => {
            product.clear_availability()?;
        }
    }
    match command.url {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            product.change_url(value)?;
        }
        PatchField::Clear => return Err(UpdateProductListingError::UrlRequired),
    }
    match command.images {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            product.replace_images(value)?;
        }
        PatchField::Clear => {
            product.replace_images(Default::default())?;
        }
    }
    if let Some(auction) = auction {
        product.replace_auction(Some(auction))?;
    }
    Ok(())
}
fn apply_optional_patch<T>(field: &mut Option<T>, patch: PatchField<T>) {
    match patch {
        PatchField::Unchanged => {}
        PatchField::Set(value) => *field = Some(value),
        PatchField::Clear => *field = None,
    }
}
fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => Some(*id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}
impl From<OperationAuthorizationError> for UpdateProductListingError {
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
impl From<PartnerProductListingAuthorizationError> for UpdateProductListingError {
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
impl From<AuctionReferenceValidationError> for UpdateProductListingError {
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
impl From<ChangeListingAvailabilityError> for UpdateProductListingError {
    fn from(_: ChangeListingAvailabilityError) -> Self {
        Self::ListingWithdrawn
    }
}
impl From<ChangeProductListingError> for UpdateProductListingError {
    fn from(error: ChangeProductListingError) -> Self {
        match error {
            ChangeProductListingError::ListingWithdrawn => Self::ListingWithdrawn,
            _ => Self::InvalidProductListing,
        }
    }
}
impl From<ProductListingRepositoryError> for UpdateProductListingError {
    fn from(_: ProductListingRepositoryError) -> Self {
        Self::PersistenceFailed
    }
}
impl From<ProductListingEventAppendError> for UpdateProductListingError {
    fn from(error: ProductListingEventAppendError) -> Self {
        Self::EventAppenderFailed {
            source: Box::new(error),
        }
    }
}
