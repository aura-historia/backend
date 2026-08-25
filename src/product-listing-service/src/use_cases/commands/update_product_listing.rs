use crate::ports::{
    PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
    PartnerProductListingAuthorizerFactory, ProductListingEventStore,
    ProductListingEventStoreError, ProductListingEventStoreFactory, ProductListingRepository,
    ProductListingRepositoryError, ProductListingRepositoryFactory,
};
use application::error::BoxError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use application::patch_field::PatchField;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use indexmap::IndexSet;
use money::Price;
use product_listing_core::product_listing::{
    ProductListingAddress, ProductListingAuction, ProductListingPricing, ProductSaleValuation,
    ProductStateTransitionError,
};
use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_state::ProductState;
use url::Url;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateProductListingCommand {
    pub address: PatchField<ProductListingAddress>,
    pub pricing: PatchField<ProductListingPricing>,
    pub price: PatchField<Price>,
    pub price_estimate_min: PatchField<Price>,
    pub price_estimate_max: PatchField<Price>,
    pub state: PatchField<ProductState>,
    pub url: PatchField<Url>,
    pub images: PatchField<IndexSet<ProductListingImage>>,
    pub auction: PatchField<ProductListingAuction>,
    pub auction_start: PatchField<Option<time::OffsetDateTime>>,
    pub auction_end: PatchField<Option<time::OffsetDateTime>>,
}

impl UpdateProductListingCommand {
    pub fn is_empty(&self) -> bool {
        !self.address.is_changed()
            && !self.pricing.is_changed()
            && !self.price.is_changed()
            && !self.price_estimate_min.is_changed()
            && !self.price_estimate_max.is_changed()
            && !self.state.is_changed()
            && !self.url.is_changed()
            && !self.images.is_changed()
            && !self.auction.is_changed()
            && !self.auction_start.is_changed()
            && !self.auction_end.is_changed()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateProductListingResult {
    pub product_listing_id: ProductListingId,
    pub event_id: Option<EventId>,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateProductListingError {
    #[error("authenticated actor required to update product")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("shop not found")]
    ShopNotFound,
    #[error("partner product authorization is temporarily unavailable")]
    PartnerProductListingAuthorizationTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("partner product authorization failed internally")]
    PartnerProductListingAuthorizationInternal {
        #[source]
        source: BoxError,
    },
    #[error("product not found")]
    ProductListingNotFound,
    #[error("product current event id did not match expected event id")]
    ProductListingCurrentEventIdConflict,
    #[error("product update cleared required state")]
    StateRequired,
    #[error("product update cleared required url")]
    UrlRequired,
    #[error("product state is invalid")]
    InvalidProductState,
    #[error("no persisted FX snapshot is available for product sale")]
    SaleFxSnapshotMissing,
    #[error("persisted FX snapshot is invalid for product sale")]
    SaleFxSnapshotInvalid {
        #[source]
        source: BoxError,
    },
    #[error("FX snapshot lookup is temporarily unavailable for product sale")]
    SaleFxSnapshotUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("generic update cannot reopen a sold product")]
    SoldProductReopenRequiresExplicitOperation,
    #[error("product already exists for shop product identity")]
    ShopListingAlreadyExists,
    #[error("product slug already exists")]
    ProductListingSlugAlreadyExists,
    #[error("product lookup by id failed")]
    ProductListingLookupByIdFailed,
    #[error("product lookup by shop product identity failed")]
    ProductListingLookupByKeyFailed {
        #[source]
        source: BoxError,
    },
    #[error("product insert failed")]
    ProductListingInsertFailed,
    #[error("product update failed")]
    ProductListingUpdateFailed,
    #[error("persisted product slug is invalid")]
    InvalidProductListingSlugPersisted,
    #[error("persisted title is incomplete")]
    IncompleteTitlePersisted,
    #[error("persisted title language is invalid")]
    InvalidTitleLanguagePersisted,
    #[error("persisted description is incomplete")]
    IncompleteDescriptionPersisted,
    #[error("persisted description language is invalid")]
    InvalidDescriptionLanguagePersisted,
    #[error("persisted price is incomplete")]
    IncompletePricePersisted,
    #[error("persisted price amount is negative")]
    NegativePriceAmountPersisted,
    #[error("persisted price currency is invalid")]
    InvalidPriceCurrencyPersisted,
    #[error("persisted product state is invalid")]
    InvalidProductStatePersisted,
    #[error("persisted product lifecycle is invalid")]
    InvalidProductLifecyclePersisted,
    #[error("persisted product URL is invalid")]
    InvalidProductListingUrlPersisted,
    #[error("persisted product images value is invalid")]
    InvalidProductListingImagesPersisted,
    #[error("persisted product image URL is invalid")]
    InvalidProductListingImageUrlPersisted,
    #[error("persisted product image prohibited-content value is invalid")]
    InvalidProductListingImageProhibitedContentPersisted,
    #[error("persisted aggregate state is invalid")]
    InvalidAggregateStatePersisted,
    #[error("product event already exists")]
    ProductListingEventAlreadyExists,
    #[error("product event append failed")]
    ProductListingEventAppendFailed,
    #[error("current product event lookup failed")]
    CurrentProductListingEventLookupFailed,
    #[error("failed to begin update product transaction")]
    BeginTransactionFailed,
    #[error("failed to commit update product transaction")]
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

pub struct UpdateProductListingHandler<U, R, E, A, F> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
    fx_rates: F,
}

impl<U, R, E, A, F> UpdateProductListingHandler<U, R, E, A, F> {
    pub fn new_with_fx_rates(
        unit_of_work: U,
        products: R,
        events: E,
        authorizer: A,
        fx_rates: F,
    ) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            authorizer,
            fx_rates,
        }
    }
}

enum UpdateProductListingTarget {
    Id(ProductListingId),
    Key(ProductListingKey),
}

impl<U, R, E, A, F> UpdateProductListingHandler<U, R, E, A, F>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    async fn persist_for_target(
        &self,
        tx: &mut U::Tx,
        context: &OperationContext,
        target: UpdateProductListingTarget,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductsWrite)
            .authorize::<UpdateProductListingError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let loaded = match target {
            UpdateProductListingTarget::Id(product_listing_id) => {
                let loaded = self
                    .products
                    .in_transaction(tx)
                    .find_by_id(product_listing_id)
                    .await?
                    .ok_or(UpdateProductListingError::ProductListingNotFound)?;

                if let Some(actor_id) = partner_actor(&context.principal) {
                    self.authorizer
                        .in_transaction(tx)
                        .authorize(actor_id, loaded.value.shop_id())
                        .await?;
                }

                loaded
            }
            UpdateProductListingTarget::Key(product_key) => {
                if let Some(actor_id) = partner_actor(&context.principal) {
                    self.authorizer
                        .in_transaction(tx)
                        .authorize(actor_id, product_key.shop_id)
                        .await?;
                }
                self.products
                    .in_transaction(tx)
                    .find_by_key(&product_key)
                    .await?
                    .ok_or(UpdateProductListingError::ProductListingNotFound)?
            }
        };
        let expected_event_id = loaded.version;
        let mut product = loaded.value;
        let sale_valuation = if matches!(command.state, PatchField::Set(ProductState::Sold))
            && product.state() != ProductState::Sold
        {
            let sold_at = time::OffsetDateTime::now_utc();
            Some(sale_valuation(&self.fx_rates, tx, sold_at).await?)
        } else {
            None
        };

        apply_command(&mut product, command, sale_valuation)?;
        let events = product.take_pending_events();
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

        Ok(UpdateProductListingResult {
            product_listing_id: product.id(),
            event_id,
        })
    }
}

#[async_trait::async_trait]
impl<U, R, E, A, F> UpdateProductListingUseCase for UpdateProductListingHandler<U, R, E, A, F>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "update_product",
        skip_all,
        fields(
            product_listing_id = %product_listing_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        product_listing_id: ProductListingId,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateProductListingError::BeginTransactionFailed)?;
        let result = self
            .persist_for_target(
                &mut tx,
                context,
                UpdateProductListingTarget::Id(product_listing_id),
                command,
            )
            .await?;
        tx.commit()
            .await
            .map_err(|_| UpdateProductListingError::CommitTransactionFailed)?;
        Ok(result)
    }

    #[tracing::instrument(
        name = "update_product_by_key",
        skip_all,
        fields(
            shop_id = %product_key.shop_id,
            shop_listing_id = %product_key.shop_listing_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute_by_key(
        &self,
        context: &OperationContext,
        product_key: ProductListingKey,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateProductListingError::BeginTransactionFailed)?;
        let result = self
            .persist_for_target(
                &mut tx,
                context,
                UpdateProductListingTarget::Key(product_key),
                command,
            )
            .await?;
        tx.commit()
            .await
            .map_err(|_| UpdateProductListingError::CommitTransactionFailed)?;
        Ok(result)
    }
}

fn apply_command(
    product: &mut product_listing_core::product_listing::ProductListing,
    command: UpdateProductListingCommand,
    sale_valuation: Option<ProductSaleValuation>,
) -> Result<(), UpdateProductListingError> {
    match command.address {
        PatchField::Unchanged => {}
        PatchField::Set(address) => {
            product.replace_address(address);
        }
        PatchField::Clear => {
            product.replace_address(Default::default());
        }
    }
    match command.pricing {
        PatchField::Unchanged => {}
        PatchField::Set(pricing) => {
            product.replace_pricing(pricing);
        }
        PatchField::Clear => {
            product.replace_pricing(Default::default());
        }
    }
    apply_price_patch(product, command.price, |pricing, price| {
        pricing.price = price;
    });
    apply_price_patch(product, command.price_estimate_min, |pricing, price| {
        pricing.price_estimate_min = price;
    });
    apply_price_patch(product, command.price_estimate_max, |pricing, price| {
        pricing.price_estimate_max = price;
    });
    match command.state {
        PatchField::Unchanged => {}
        PatchField::Set(state) => apply_state(product, state, sale_valuation)?,
        PatchField::Clear => return Err(UpdateProductListingError::StateRequired),
    }
    match command.url {
        PatchField::Unchanged => {}
        PatchField::Set(url) => {
            product.change_url(url);
        }
        PatchField::Clear => return Err(UpdateProductListingError::UrlRequired),
    }
    match command.images {
        PatchField::Unchanged => {}
        PatchField::Set(images) => {
            product.replace_images(images);
        }
        PatchField::Clear => {
            product.replace_images(Default::default());
        }
    }
    match command.auction {
        PatchField::Unchanged => {}
        PatchField::Set(auction) => {
            product.replace_auction(auction);
        }
        PatchField::Clear => {
            product.replace_auction(Default::default());
        }
    }
    apply_auction_patch(product, command.auction_start, |auction, value| {
        auction.start = value;
    });
    apply_auction_patch(product, command.auction_end, |auction, value| {
        auction.end = value;
    });

    Ok(())
}

fn apply_state(
    product: &mut product_listing_core::product_listing::ProductListing,
    state: ProductState,
    sale_valuation: Option<ProductSaleValuation>,
) -> Result<(), UpdateProductListingError> {
    if product.state() == state {
        return Ok(());
    }
    match state {
        ProductState::Listed => product.mark_listed()?,
        ProductState::Available => product.mark_available()?,
        ProductState::Reserved => product.mark_reserved()?,
        ProductState::Sold => product
            .mark_sold(sale_valuation.ok_or(UpdateProductListingError::SaleFxSnapshotMissing)?)?,
        ProductState::Removed => product.mark_removed()?,
        ProductState::Unknown => product.mark_unknown()?,
    };
    Ok(())
}

fn apply_price_patch(
    product: &mut product_listing_core::product_listing::ProductListing,
    patch: PatchField<Price>,
    apply: impl FnOnce(&mut ProductListingPricing, Option<Price>),
) {
    match patch {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            let mut pricing = product.pricing();
            apply(&mut pricing, Some(value));
            product.replace_pricing(pricing);
        }
        PatchField::Clear => {
            let mut pricing = product.pricing();
            apply(&mut pricing, None);
            product.replace_pricing(pricing);
        }
    }
}

fn apply_auction_patch(
    product: &mut product_listing_core::product_listing::ProductListing,
    patch: PatchField<Option<time::OffsetDateTime>>,
    apply: impl FnOnce(&mut ProductListingAuction, Option<time::OffsetDateTime>),
) {
    match patch {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            let mut auction = product.auction();
            apply(&mut auction, value);
            product.replace_auction(auction);
        }
        PatchField::Clear => {
            let mut auction = product.auction();
            apply(&mut auction, None);
            product.replace_auction(auction);
        }
    }
}

async fn sale_valuation<Tx, F>(
    fx_rates: &F,
    tx: &mut Tx,
    sold_at: time::OffsetDateTime,
) -> Result<ProductSaleValuation, UpdateProductListingError>
where
    F: FxRateSnapshotRepositoryFactory<Tx>,
{
    let mut repository = fx_rates.in_transaction(tx);
    let snapshot = repository
        .find_latest_at_or_before(sold_at)
        .await
        .map_err(UpdateProductListingError::from)?
        .ok_or(UpdateProductListingError::SaleFxSnapshotMissing)?;
    Ok(ProductSaleValuation {
        sold_at,
        fx_rate_id: snapshot.id(),
    })
}

impl From<ProductStateTransitionError> for UpdateProductListingError {
    fn from(error: ProductStateTransitionError) -> Self {
        match error {
            ProductStateTransitionError::SoldProductReopenRequiresExplicitOperation => {
                Self::SoldProductReopenRequiresExplicitOperation
            }
        }
    }
}

impl From<FxRateSnapshotRepositoryError> for UpdateProductListingError {
    fn from(error: FxRateSnapshotRepositoryError) -> Self {
        match error {
            FxRateSnapshotRepositoryError::InsertFailed { source }
            | FxRateSnapshotRepositoryError::ReadFailed { source } => {
                Self::SaleFxSnapshotUnavailable { source }
            }
            FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
                Self::SaleFxSnapshotInvalid { source }
            }
            FxRateSnapshotRepositoryError::CapturedAtNotMonotonic => Self::SaleFxSnapshotMissing,
        }
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

fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}

impl From<PartnerProductListingAuthorizationError> for UpdateProductListingError {
    fn from(error: PartnerProductListingAuthorizationError) -> Self {
        match error {
            PartnerProductListingAuthorizationError::ShopNotFound => Self::ShopNotFound,
            PartnerProductListingAuthorizationError::Forbidden => Self::Forbidden,
            PartnerProductListingAuthorizationError::TemporarilyUnavailable { source } => {
                Self::PartnerProductListingAuthorizationTemporarilyUnavailable { source }
            }
            PartnerProductListingAuthorizationError::Internal { source } => {
                Self::PartnerProductListingAuthorizationInternal { source }
            }
        }
    }
}

impl From<ProductListingRepositoryError> for UpdateProductListingError {
    fn from(error: ProductListingRepositoryError) -> Self {
        match error {
            ProductListingRepositoryError::ProductListingCurrentEventIdConflict => {
                Self::ProductListingCurrentEventIdConflict
            }
            ProductListingRepositoryError::ShopListingAlreadyExists => {
                Self::ShopListingAlreadyExists
            }
            ProductListingRepositoryError::ProductListingSlugAlreadyExists => {
                Self::ProductListingSlugAlreadyExists
            }
            ProductListingRepositoryError::ProductListingLookupByIdFailed => {
                Self::ProductListingLookupByIdFailed
            }
            ProductListingRepositoryError::ProductListingLookupByKeyFailed { source } => {
                Self::ProductListingLookupByKeyFailed { source }
            }
            ProductListingRepositoryError::ProductListingInsertFailed => {
                Self::ProductListingInsertFailed
            }
            ProductListingRepositoryError::ProductListingUpdateFailed => {
                Self::ProductListingUpdateFailed
            }
            ProductListingRepositoryError::InvalidProductListingSlugPersisted => {
                Self::InvalidProductListingSlugPersisted
            }
            ProductListingRepositoryError::IncompleteTitlePersisted => {
                Self::IncompleteTitlePersisted
            }
            ProductListingRepositoryError::InvalidTitleLanguagePersisted => {
                Self::InvalidTitleLanguagePersisted
            }
            ProductListingRepositoryError::IncompleteDescriptionPersisted => {
                Self::IncompleteDescriptionPersisted
            }
            ProductListingRepositoryError::InvalidDescriptionLanguagePersisted => {
                Self::InvalidDescriptionLanguagePersisted
            }
            ProductListingRepositoryError::IncompletePricePersisted => {
                Self::IncompletePricePersisted
            }
            ProductListingRepositoryError::NegativePriceAmountPersisted => {
                Self::NegativePriceAmountPersisted
            }
            ProductListingRepositoryError::InvalidPriceCurrencyPersisted => {
                Self::InvalidPriceCurrencyPersisted
            }
            ProductListingRepositoryError::InvalidProductStatePersisted => {
                Self::InvalidProductStatePersisted
            }
            ProductListingRepositoryError::InvalidProductLifecyclePersisted => {
                Self::InvalidProductLifecyclePersisted
            }
            ProductListingRepositoryError::InvalidProductListingUrlPersisted => {
                Self::InvalidProductListingUrlPersisted
            }
            ProductListingRepositoryError::InvalidProductListingImagesPersisted => {
                Self::InvalidProductListingImagesPersisted
            }
            ProductListingRepositoryError::InvalidProductListingImageUrlPersisted => {
                Self::InvalidProductListingImageUrlPersisted
            }
            ProductListingRepositoryError::InvalidProductListingImageProhibitedContentPersisted => {
                Self::InvalidProductListingImageProhibitedContentPersisted
            }
            ProductListingRepositoryError::InvalidAggregateStatePersisted => {
                Self::InvalidAggregateStatePersisted
            }
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct MissingFxRateSnapshotFactory;

#[cfg(test)]
struct MissingFxRateSnapshotRepository;

#[cfg(test)]
impl<Tx> FxRateSnapshotRepositoryFactory<Tx> for MissingFxRateSnapshotFactory {
    fn in_transaction<'tx>(&'tx self, _tx: &'tx mut Tx) -> impl FxRateSnapshotRepository + 'tx {
        MissingFxRateSnapshotRepository
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl FxRateSnapshotRepository for MissingFxRateSnapshotRepository {
    async fn find_latest(
        &mut self,
    ) -> Result<Option<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
        Ok(None)
    }

    async fn find_latest_at_or_before(
        &mut self,
        _timestamp: time::OffsetDateTime,
    ) -> Result<Option<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
        Ok(None)
    }

    async fn find_by_id(
        &mut self,
        _id: fxrate_core::FxRateId,
    ) -> Result<Option<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
        Ok(None)
    }

    async fn find_by_ids(
        &mut self,
        _ids: &[fxrate_core::FxRateId],
    ) -> Result<Vec<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
        Ok(Vec::new())
    }

    async fn insert(
        &mut self,
        _snapshot: &fxrate_core::NewFxRateSnapshot,
        _source_event_id: &str,
    ) -> Result<fxrate_service::ports::FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError>
    {
        Ok(fxrate_service::ports::FxRateSnapshotInsertOutcome::Duplicate)
    }
}

#[cfg(test)]
impl<U, R, E, A> UpdateProductListingHandler<U, R, E, A, MissingFxRateSnapshotFactory> {
    fn new(unit_of_work: U, products: R, events: E, authorizer: A) -> Self {
        Self::new_with_fx_rates(
            unit_of_work,
            products,
            events,
            authorizer,
            MissingFxRateSnapshotFactory,
        )
    }
}

impl From<ProductListingEventStoreError> for UpdateProductListingError {
    fn from(error: ProductListingEventStoreError) -> Self {
        match error {
            ProductListingEventStoreError::ProductListingEventAlreadyExists => {
                Self::ProductListingEventAlreadyExists
            }
            ProductListingEventStoreError::ProductListingEventAppendFailed => {
                Self::ProductListingEventAppendFailed
            }
            ProductListingEventStoreError::CurrentProductListingEventLookupFailed => {
                Self::CurrentProductListingEventLookupFailed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use localization::Language;
    use localization::Localized;
    use money::Currency;
    use money::{MonetaryAmount, Price};

    use application::transaction::TransactionError;
    use domain_primitives::versioned::Versioned;
    use product_listing_core::description::Description;
    use product_listing_core::product_listing::{
        NewProductListing, ProductListing, ProductListingDomainEvent,
    };
    use product_listing_core::shop_listing_id::ShopListingId;
    use product_listing_core::title::Title;
    use shop_core::shop_id::ShopId;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Debug, Default)]
    struct FakeState {
        begin_error: bool,
        commit_error: bool,
        find_by_id_result: Option<
            Result<Option<Versioned<ProductListing, EventId>>, ProductListingRepositoryError>,
        >,
        find_by_key_result: Option<
            Result<Option<Versioned<ProductListing, EventId>>, ProductListingRepositoryError>,
        >,
        update_result: Option<Result<(), ProductListingRepositoryError>>,
        append_result: Option<Result<(), ProductListingEventStoreError>>,
        update_count: usize,
        append_count: usize,
        commit_count: usize,
    }

    type SharedState = Arc<Mutex<FakeState>>;

    #[derive(Clone)]
    struct FakeUnitOfWork {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeRepositoryFactory {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeEventStoreFactory {
        state: SharedState,
    }

    struct FakeTx {
        state: SharedState,
    }

    struct FakeRepository {
        state: SharedState,
    }

    struct FakeEventStore {
        state: SharedState,
    }

    #[derive(Clone, Copy)]
    struct AllowPartnerProductListingAuthorizer;

    struct AllowPartnerProductListingAuthorizerTx;

    #[derive(Clone, Copy)]
    struct DenyPartnerProductListingAuthorizer;

    struct DenyPartnerProductListingAuthorizerTx;

    impl PartnerProductListingAuthorizerFactory<FakeTx> for AllowPartnerProductListingAuthorizer {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl PartnerProductListingAuthorizer + 'tx {
            AllowPartnerProductListingAuthorizerTx
        }
    }

    #[async_trait::async_trait]
    impl PartnerProductListingAuthorizer for AllowPartnerProductListingAuthorizerTx {
        async fn authorize(
            &mut self,
            _actor_id: UserId,
            _shop_id: ShopId,
        ) -> Result<(), PartnerProductListingAuthorizationError> {
            Ok(())
        }
    }

    impl PartnerProductListingAuthorizerFactory<FakeTx> for DenyPartnerProductListingAuthorizer {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl PartnerProductListingAuthorizer + 'tx {
            DenyPartnerProductListingAuthorizerTx
        }
    }

    #[async_trait::async_trait]
    impl PartnerProductListingAuthorizer for DenyPartnerProductListingAuthorizerTx {
        async fn authorize(
            &mut self,
            _actor_id: UserId,
            _shop_id: ShopId,
        ) -> Result<(), PartnerProductListingAuthorizationError> {
            Err(PartnerProductListingAuthorizationError::Forbidden)
        }
    }

    fn state() -> SharedState {
        Arc::new(Mutex::new(FakeState::default()))
    }

    fn lock_state(state: &SharedState) -> MutexGuard<'_, FakeState> {
        match state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn uow(state: &SharedState) -> FakeUnitOfWork {
        FakeUnitOfWork {
            state: Arc::clone(state),
        }
    }

    fn repository_factory(state: &SharedState) -> FakeRepositoryFactory {
        FakeRepositoryFactory {
            state: Arc::clone(state),
        }
    }

    fn event_store_factory(state: &SharedState) -> FakeEventStoreFactory {
        FakeEventStoreFactory {
            state: Arc::clone(state),
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let state = lock_state(&self.state);
            if state.begin_error {
                Err(TransactionError::BeginFailed)
            } else {
                Ok(FakeTx {
                    state: Arc::clone(&self.state),
                })
            }
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock_state(&self.state);
            state.commit_count += 1;
            if state.commit_error {
                Err(TransactionError::CommitFailed)
            } else {
                Ok(())
            }
        }
    }

    impl ProductListingRepositoryFactory<FakeTx> for FakeRepositoryFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl ProductListingRepository + 'tx {
            FakeRepository {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductListingRepository for FakeRepository {
        async fn find_by_id(
            &mut self,
            _id: ProductListingId,
        ) -> Result<Option<Versioned<ProductListing, EventId>>, ProductListingRepositoryError>
        {
            let mut state = lock_state(&self.state);
            match state.find_by_id_result.take() {
                Some(result) => result,
                None => Ok(None),
            }
        }

        async fn find_by_key(
            &mut self,
            _key: &ProductListingKey,
        ) -> Result<Option<Versioned<ProductListing, EventId>>, ProductListingRepositoryError>
        {
            match lock_state(&self.state).find_by_key_result.take() {
                Some(result) => result,
                None => Ok(None),
            }
        }

        async fn insert(
            &mut self,
            product: &ProductListing,
            current_event_id: EventId,
        ) -> Result<Versioned<ProductListing, EventId>, ProductListingRepositoryError> {
            Ok(Versioned::new(product.clone(), current_event_id))
        }

        async fn update(
            &mut self,
            product: &ProductListing,
            _expected_event_id: EventId,
            new_event_id: EventId,
        ) -> Result<Versioned<ProductListing, EventId>, ProductListingRepositoryError> {
            let mut state = lock_state(&self.state);
            state.update_count += 1;
            match state.update_result.take() {
                Some(Err(error)) => Err(error),
                Some(Ok(())) | None => Ok(Versioned::new(product.clone(), new_event_id)),
            }
        }
    }

    impl ProductListingEventStoreFactory<FakeTx> for FakeEventStoreFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl ProductListingEventStore + 'tx {
            FakeEventStore {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductListingEventStore for FakeEventStore {
        async fn append(
            &mut self,
            _event: &ProductListingDomainEvent,
        ) -> Result<(), ProductListingEventStoreError> {
            let mut state = lock_state(&self.state);
            state.append_count += 1;
            match state.append_result.take() {
                Some(result) => result,
                None => Ok(()),
            }
        }

        async fn find_current_event_id(
            &mut self,
            _product_listing_id: ProductListingId,
        ) -> Result<Option<EventId>, ProductListingEventStoreError> {
            Ok(None)
        }
    }

    fn handler(
        state: &SharedState,
    ) -> UpdateProductListingHandler<
        FakeUnitOfWork,
        FakeRepositoryFactory,
        FakeEventStoreFactory,
        AllowPartnerProductListingAuthorizer,
        MissingFxRateSnapshotFactory,
    > {
        UpdateProductListingHandler::new(
            uow(state),
            repository_factory(state),
            event_store_factory(state),
            AllowPartnerProductListingAuthorizer,
        )
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn partner_context() -> OperationContext {
        OperationContext {
            principal: Principal::User(UserId::new()),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn url(value: &str) -> Result<Url, url::ParseError> {
        Url::parse(value)
    }

    fn product() -> Result<ProductListing, url::ParseError> {
        let mut product = ProductListing::create(new_product(ProductListingId::new())?)
            .map_err(|_| url::ParseError::EmptyHost)?;
        let _events = product.take_pending_events();
        Ok(product)
    }

    fn new_product(
        product_listing_id: ProductListingId,
    ) -> Result<NewProductListing, url::ParseError> {
        Ok(NewProductListing {
            id: product_listing_id,
            shop_id: ShopId::new(),
            seller_id: ShopId::new(),
            shop_listing_id: ShopListingId::new(),
            address: ProductListingAddress::default(),
            title: Some(Localized {
                localization: Language::En,
                payload: Title::from("Cabinet"),
            }),
            description: Some(Localized {
                localization: Language::En,
                payload: Description::from("Old cabinet"),
            }),
            pricing: ProductListingPricing {
                price: Some(Price::new(MonetaryAmount::from(100_u64), Currency::Eur)),
                ..Default::default()
            },
            sale_valuation: None,
            state: ProductState::Listed,
            url: url("https://shop.example/products/1")?,
            images: IndexSet::new(),
            auction: ProductListingAuction::default(),
        })
    }

    fn versioned_product(product: ProductListing) -> Versioned<ProductListing, EventId> {
        Versioned::new(product, EventId::new())
    }

    #[test]
    fn should_report_empty_update_when_all_fields_unchanged() {
        let command = UpdateProductListingCommand {
            ..Default::default()
        };

        assert!(command.is_empty());
    }

    #[tokio::test]
    async fn should_not_persist_or_commit_when_sold_transition_has_no_persisted_fx_snapshot()
    -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_listing_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductListingCommand {
            state: PatchField::Set(ProductState::Sold),
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_listing_id, command)
            .await;

        assert!(matches!(
            result,
            Err(UpdateProductListingError::SaleFxSnapshotMissing)
        ));
        let state = lock_state(&state);
        assert_eq!(0, state.update_count);
        assert_eq!(0, state.append_count);
        assert_eq!(0, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_update_product_when_field_set() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_listing_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductListingCommand {
            state: PatchField::Set(ProductState::Available),
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_listing_id, command)
            .await;

        assert!(matches!(
            result,
            Ok(UpdateProductListingResult {
                event_id: Some(_),
                ..
            })
        ));
        let state = lock_state(&state);
        assert_eq!(1, state.update_count);
        assert_eq!(1, state.append_count);
        assert_eq!(1, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_partner_when_updating_product_by_id_without_shop_access()
    -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_listing_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductListingCommand {
            state: PatchField::Set(ProductState::Available),
            ..Default::default()
        };
        let handler = UpdateProductListingHandler::new(
            uow(&state),
            repository_factory(&state),
            event_store_factory(&state),
            DenyPartnerProductListingAuthorizer,
        );

        let result = handler
            .execute(&partner_context(), product_listing_id, command)
            .await;

        assert!(matches!(result, Err(UpdateProductListingError::Forbidden)));
        let state = lock_state(&state);
        assert_eq!(0, state.update_count);
        assert_eq!(0, state.append_count);
        assert_eq!(0, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_update_product_by_partner_key() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let key = ProductListingKey::new(product.shop_id(), product.shop_listing_id().clone());
        lock_state(&state).find_by_key_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductListingCommand {
            state: PatchField::Set(ProductState::Available),
            ..Default::default()
        };

        let result = handler(&state)
            .execute_by_key(&context(), key, command)
            .await;

        assert!(matches!(
            result,
            Ok(UpdateProductListingResult {
                event_id: Some(_),
                ..
            })
        ));
        assert_eq!(1, lock_state(&state).update_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_commit_no_op_when_update_empty() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_listing_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductListingCommand {
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_listing_id, command)
            .await;

        assert!(matches!(
            result,
            Ok(UpdateProductListingResult { event_id: None, .. })
        ));
        let state = lock_state(&state);
        assert_eq!(0, state.update_count);
        assert_eq!(0, state.append_count);
        assert_eq!(1, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_not_found_when_update_product_missing() {
        let state = state();
        let product_listing_id = ProductListingId::new();
        let command = UpdateProductListingCommand {
            state: PatchField::Set(ProductState::Available),
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_listing_id, command)
            .await;

        assert!(matches!(
            result,
            Err(UpdateProductListingError::ProductListingNotFound)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_map_begin_error_when_update_begin_fails() {
        let state = state();
        let product_listing_id = ProductListingId::new();
        lock_state(&state).begin_error = true;
        let command = UpdateProductListingCommand {
            state: PatchField::Set(ProductState::Available),
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_listing_id, command)
            .await;

        assert!(matches!(
            result,
            Err(UpdateProductListingError::BeginTransactionFailed)
        ));
    }

    #[tokio::test]
    async fn should_map_commit_error_when_update_commit_fails() -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).commit_error = true;
        let product = product()?;
        let product_listing_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductListingCommand {
            state: PatchField::Set(ProductState::Available),
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_listing_id, command)
            .await;

        assert!(matches!(
            result,
            Err(UpdateProductListingError::CommitTransactionFailed)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_when_update_find_fails() {
        let state = state();
        let product_listing_id = ProductListingId::new();
        lock_state(&state).find_by_id_result = Some(Err(
            ProductListingRepositoryError::ProductListingLookupByIdFailed,
        ));
        let command = UpdateProductListingCommand {
            state: PatchField::Set(ProductState::Available),
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_listing_id, command)
            .await;

        assert!(matches!(
            result,
            Err(UpdateProductListingError::ProductListingLookupByIdFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_not_commit_when_update_repository_fails() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_listing_id = product.id();
        {
            let mut state = lock_state(&state);
            state.find_by_id_result = Some(Ok(Some(versioned_product(product))));
            state.update_result = Some(Err(
                ProductListingRepositoryError::ProductListingUpdateFailed,
            ));
        }
        let command = UpdateProductListingCommand {
            state: PatchField::Set(ProductState::Available),
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_listing_id, command)
            .await;

        assert!(matches!(
            result,
            Err(UpdateProductListingError::ProductListingUpdateFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_when_update_event_append_fails() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_listing_id = product.id();
        {
            let mut state = lock_state(&state);
            state.find_by_id_result = Some(Ok(Some(versioned_product(product))));
            state.append_result = Some(Err(
                ProductListingEventStoreError::ProductListingEventAppendFailed,
            ));
        }
        let command = UpdateProductListingCommand {
            state: PatchField::Set(ProductState::Available),
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_listing_id, command)
            .await;

        assert!(matches!(
            result,
            Err(UpdateProductListingError::ProductListingEventAppendFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_apply_all_set_patch_fields_when_update() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_listing_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductListingCommand {
            address: PatchField::Set(ProductListingAddress::default()),
            pricing: PatchField::Set(ProductListingPricing::default()),
            state: PatchField::Set(ProductState::Available),
            url: PatchField::Set(url("https://shop.example/products/2")?),
            images: PatchField::Set(IndexSet::new()),
            auction: PatchField::Set(ProductListingAuction {
                start: Some(time::OffsetDateTime::UNIX_EPOCH),
                end: None,
            }),
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_listing_id, command)
            .await;

        assert!(result.is_ok());
        assert!(lock_state(&state).append_count >= 1);
        Ok(())
    }

    #[tokio::test]
    async fn should_apply_clear_patch_fields_when_allowed() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_listing_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductListingCommand {
            address: PatchField::Clear,
            pricing: PatchField::Clear,
            images: PatchField::Clear,
            auction: PatchField::Clear,
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_listing_id, command)
            .await;

        assert!(result.is_ok());
        assert_eq!(1, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_clear_state_when_update() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_listing_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductListingCommand {
            state: PatchField::Clear,
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_listing_id, command)
            .await;

        assert!(matches!(
            result,
            Err(UpdateProductListingError::StateRequired)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_clear_url_when_update() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_listing_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductListingCommand {
            url: PatchField::Clear,
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_listing_id, command)
            .await;

        assert!(matches!(
            result,
            Err(UpdateProductListingError::UrlRequired)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }
}
