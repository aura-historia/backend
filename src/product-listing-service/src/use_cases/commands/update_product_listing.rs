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
use application::patch_field::PatchField;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use indexmap::IndexSet;
use money::Price;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing::{
    ChangeListingAvailabilityError, ChangeProductListingError, ProductListing,
    ProductListingAddress, ProductListingAuction, ProductListingPricing,
};
use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
use product_listing_core::product_listing_image::ProductListingImage;
use url::Url;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateProductListingCommand {
    pub address: PatchField<ProductListingAddress>,
    pub pricing: PatchField<ProductListingPricing>,
    pub price: PatchField<Price>,
    pub price_estimate_min: PatchField<Price>,
    pub price_estimate_max: PatchField<Price>,
    pub availability: PatchField<ListingAvailability>,
    pub url: PatchField<Url>,
    pub images: PatchField<IndexSet<ProductListingImage>>,
    pub auction: PatchField<ProductListingAuction>,
    pub auction_start: PatchField<Option<time::OffsetDateTime>>,
    pub auction_end: PatchField<Option<time::OffsetDateTime>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateProductListingResult {
    pub product_listing_id: ProductListingId,
    pub event_id: Option<EventId>,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateProductListingError {
    #[error("authenticated actor required to update product listing")]
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
    #[error("product listing not found")]
    NotFound,
    #[error("product listing is withdrawn")]
    ListingWithdrawn,
    #[error("product listing persistence failed")]
    PersistenceFailed,
    #[error("product listing event storage failed")]
    EventStoreFailed,
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

pub struct UpdateProductListingHandler<U, R, E, A> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
}
impl<U, R, E, A> UpdateProductListingHandler<U, R, E, A> {
    pub fn new(unit_of_work: U, products: R, events: E, authorizer: A) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            authorizer,
        }
    }
}
enum UpdateTarget {
    Id(ProductListingId),
    Key(ProductListingKey),
}

impl<U, R, E, A> UpdateProductListingHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
{
    async fn update(
        &self,
        context: &OperationContext,
        target: UpdateTarget,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError> {
        context
            .require()
            .credential_capability(CredentialCapability::PartnerShopsWrite)
            .authorize::<UpdateProductListingError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateProductListingError::BeginTransactionFailed)?;
        let loaded = match target {
            UpdateTarget::Id(id) => {
                let loaded = self
                    .products
                    .in_transaction(&mut tx)
                    .find_by_id(id)
                    .await?
                    .ok_or(UpdateProductListingError::NotFound)?;
                if let Some(actor_id) = partner_actor(&context.principal) {
                    self.authorizer
                        .in_transaction(&mut tx)
                        .authorize(actor_id, loaded.value.shop_id())
                        .await?;
                }
                loaded
            }
            UpdateTarget::Key(key) => {
                if let Some(actor_id) = partner_actor(&context.principal) {
                    self.authorizer
                        .in_transaction(&mut tx)
                        .authorize(actor_id, key.shop_id)
                        .await?;
                }
                self.products
                    .in_transaction(&mut tx)
                    .find_by_key(&key)
                    .await?
                    .ok_or(UpdateProductListingError::NotFound)?
            }
        };
        let expected_event_id = loaded.version;
        let mut product = loaded.value;
        apply_command(&mut product, command)?;
        let events = stamp_product_listing_events(
            product.id(),
            time::OffsetDateTime::now_utc(),
            product.take_pending_event_payloads(),
        );
        let event_id = events.last().map(|event| event.event_id);
        if let Some(new_event_id) = event_id {
            product = self
                .products
                .in_transaction(&mut tx)
                .update(&product, expected_event_id, new_event_id)
                .await?
                .value;
            for event in &events {
                self.events.in_transaction(&mut tx).append(event).await?;
            }
        }
        tx.commit()
            .await
            .map_err(|_| UpdateProductListingError::CommitTransactionFailed)?;
        tracing::info!(event = "product_listing.updated", actor_type = context.principal.kind(), actor_id = %context.principal.label(), product_listing_id = %product.id(), event_id = ?event_id, outcome = "success");
        Ok(UpdateProductListingResult {
            product_listing_id: product.id(),
            event_id,
        })
    }
}

#[async_trait::async_trait]
impl<U, R, E, A> UpdateProductListingUseCase for UpdateProductListingHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
{
    #[tracing::instrument(name = "update_product_listing", skip_all, fields(product_listing_id = %product_listing_id, principal_type = context.principal.kind(), actor_id = tracing::field::Empty, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        product_listing_id: ProductListingId,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError> {
        self.update(context, UpdateTarget::Id(product_listing_id), command)
            .await
    }
    #[tracing::instrument(name = "update_product_listing_by_key", skip_all, fields(shop_id = %product_key.shop_id, shop_listing_id = %product_key.shop_listing_id, principal_type = context.principal.kind(), actor_id = tracing::field::Empty, request_id = %context.request_id, correlation_id = %context.correlation_id))]
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
) -> Result<(), UpdateProductListingError> {
    match command.address {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            product.replace_address(value)?;
        }
        PatchField::Clear => {
            product.replace_address(Default::default())?;
        }
    }
    match command.pricing {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            product.replace_pricing(value)?;
        }
        PatchField::Clear => {
            product.replace_pricing(Default::default())?;
        }
    }
    apply_price_patch(product, command.price, |pricing, value| {
        pricing.price = value
    })?;
    apply_price_patch(product, command.price_estimate_min, |pricing, value| {
        pricing.price_estimate_min = value
    })?;
    apply_price_patch(product, command.price_estimate_max, |pricing, value| {
        pricing.price_estimate_max = value
    })?;
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
        PatchField::Clear => {}
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
    match command.auction {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            product.replace_auction(value)?;
        }
        PatchField::Clear => {
            product.replace_auction(Default::default())?;
        }
    }
    apply_auction_patch(product, command.auction_start, |auction, value| {
        auction.start = value
    })?;
    apply_auction_patch(product, command.auction_end, |auction, value| {
        auction.end = value
    })?;
    Ok(())
}
fn apply_price_patch(
    product: &mut ProductListing,
    patch: PatchField<Price>,
    apply: impl FnOnce(&mut ProductListingPricing, Option<Price>),
) -> Result<(), UpdateProductListingError> {
    match patch {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            let mut pricing = product.pricing();
            apply(&mut pricing, Some(value));
            product.replace_pricing(pricing)?;
        }
        PatchField::Clear => {
            let mut pricing = product.pricing();
            apply(&mut pricing, None);
            product.replace_pricing(pricing)?;
        }
    };
    Ok(())
}
fn apply_auction_patch(
    product: &mut ProductListing,
    patch: PatchField<Option<time::OffsetDateTime>>,
    apply: impl FnOnce(&mut ProductListingAuction, Option<time::OffsetDateTime>),
) -> Result<(), UpdateProductListingError> {
    match patch {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            let mut auction = product.auction();
            apply(&mut auction, value);
            product.replace_auction(auction)?;
        }
        PatchField::Clear => {
            let mut auction = product.auction();
            apply(&mut auction, None);
            product.replace_auction(auction)?;
        }
    };
    Ok(())
}
fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => Some(*id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}
impl From<ChangeListingAvailabilityError> for UpdateProductListingError {
    fn from(_: ChangeListingAvailabilityError) -> Self {
        Self::ListingWithdrawn
    }
}
impl From<ChangeProductListingError> for UpdateProductListingError {
    fn from(_: ChangeProductListingError) -> Self {
        Self::ListingWithdrawn
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
impl From<ProductListingRepositoryError> for UpdateProductListingError {
    fn from(_: ProductListingRepositoryError) -> Self {
        Self::PersistenceFailed
    }
}
impl From<ProductListingEventStoreError> for UpdateProductListingError {
    fn from(_: ProductListingEventStoreError) -> Self {
        Self::EventStoreFailed
    }
}
