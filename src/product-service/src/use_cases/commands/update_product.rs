use crate::ports::{
    PartnerProductAuthorizationError, PartnerProductAuthorizer, PartnerProductAuthorizerFactory,
    ProductEventStore, ProductEventStoreError, ProductEventStoreFactory, ProductRepository,
    ProductRepositoryError, ProductRepositoryFactory,
};
use application::transaction::{Transaction, UnitOfWork};
use common::error::boxed::BoxError;
use common::event_id::EventId;
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use common::patch_field::PatchField;
use common::product_id::{ProductId, ProductKey};
use common::product_state::domain::ProductState;
use common::user_id::UserId;
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use indexmap::IndexSet;
use money::Price;
use product_core::product::{
    ProductAddress, ProductAuction, ProductPricing, ProductSaleValuation,
    ProductStateTransitionError,
};
use product_core::product_image::ProductImage;
use url::Url;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateProductCommand {
    pub address: PatchField<ProductAddress>,
    pub pricing: PatchField<ProductPricing>,
    pub price: PatchField<Price>,
    pub price_estimate_min: PatchField<Price>,
    pub price_estimate_max: PatchField<Price>,
    pub state: PatchField<ProductState>,
    pub url: PatchField<Url>,
    pub images: PatchField<IndexSet<ProductImage>>,
    pub auction: PatchField<ProductAuction>,
    pub auction_start: PatchField<Option<time::OffsetDateTime>>,
    pub auction_end: PatchField<Option<time::OffsetDateTime>>,
}

impl UpdateProductCommand {
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
pub struct UpdateProductResult {
    pub product_id: ProductId,
    pub event_id: Option<EventId>,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateProductError {
    #[error("authenticated actor required to update product")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("shop not found")]
    ShopNotFound,
    #[error("partner product authorization is temporarily unavailable")]
    PartnerProductAuthorizationTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("partner product authorization failed internally")]
    PartnerProductAuthorizationInternal {
        #[source]
        source: BoxError,
    },
    #[error("product not found")]
    ProductNotFound,
    #[error("product current event id did not match expected event id")]
    ProductCurrentEventIdConflict,
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
    ShopProductAlreadyExists,
    #[error("product slug already exists")]
    ProductSlugAlreadyExists,
    #[error("product lookup by id failed")]
    ProductLookupByIdFailed,
    #[error("product lookup by shop product identity failed")]
    ProductLookupByKeyFailed {
        #[source]
        source: BoxError,
    },
    #[error("product insert failed")]
    ProductInsertFailed,
    #[error("product update failed")]
    ProductUpdateFailed,
    #[error("persisted product slug is invalid")]
    InvalidProductSlugPersisted,
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
    InvalidProductUrlPersisted,
    #[error("persisted product images value is invalid")]
    InvalidProductImagesPersisted,
    #[error("persisted product image URL is invalid")]
    InvalidProductImageUrlPersisted,
    #[error("persisted product image prohibited-content value is invalid")]
    InvalidProductImageProhibitedContentPersisted,
    #[error("persisted aggregate state is invalid")]
    InvalidAggregateStatePersisted,
    #[error("product event already exists")]
    ProductEventAlreadyExists,
    #[error("product event append failed")]
    ProductEventAppendFailed,
    #[error("current product event lookup failed")]
    CurrentProductEventLookupFailed,
    #[error("failed to begin update product transaction")]
    BeginTransactionFailed,
    #[error("failed to commit update product transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpdateProductUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        product_id: ProductId,
        command: UpdateProductCommand,
    ) -> Result<UpdateProductResult, UpdateProductError>;

    async fn execute_by_key(
        &self,
        context: &OperationContext,
        product_key: ProductKey,
        command: UpdateProductCommand,
    ) -> Result<UpdateProductResult, UpdateProductError>;
}

pub struct UpdateProductHandler<U, R, E, A, F> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
    fx_rates: F,
}

impl<U, R, E, A, F> UpdateProductHandler<U, R, E, A, F> {
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

enum UpdateProductTarget {
    Id(ProductId),
    Key(ProductKey),
}

impl<U, R, E, A, F> UpdateProductHandler<U, R, E, A, F>
where
    U: UnitOfWork,
    R: ProductRepositoryFactory<U::Tx>,
    E: ProductEventStoreFactory<U::Tx>,
    A: PartnerProductAuthorizerFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    async fn persist_for_target(
        &self,
        tx: &mut U::Tx,
        context: &OperationContext,
        target: UpdateProductTarget,
        command: UpdateProductCommand,
    ) -> Result<UpdateProductResult, UpdateProductError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductsWrite)
            .authorize::<UpdateProductError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let loaded = match target {
            UpdateProductTarget::Id(product_id) => {
                let loaded = self
                    .products
                    .in_transaction(tx)
                    .find_by_id(product_id)
                    .await?
                    .ok_or(UpdateProductError::ProductNotFound)?;

                if let Some(actor_id) = partner_actor(&context.principal) {
                    self.authorizer
                        .in_transaction(tx)
                        .authorize(actor_id, loaded.value.shop_id())
                        .await?;
                }

                loaded
            }
            UpdateProductTarget::Key(product_key) => {
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
                    .ok_or(UpdateProductError::ProductNotFound)?
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

        Ok(UpdateProductResult {
            product_id: product.id(),
            event_id,
        })
    }
}

#[async_trait::async_trait]
impl<U, R, E, A, F> UpdateProductUseCase for UpdateProductHandler<U, R, E, A, F>
where
    U: UnitOfWork,
    R: ProductRepositoryFactory<U::Tx>,
    E: ProductEventStoreFactory<U::Tx>,
    A: PartnerProductAuthorizerFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "update_product",
        skip_all,
        fields(
            product_id = %product_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        product_id: ProductId,
        command: UpdateProductCommand,
    ) -> Result<UpdateProductResult, UpdateProductError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateProductError::BeginTransactionFailed)?;
        let result = self
            .persist_for_target(
                &mut tx,
                context,
                UpdateProductTarget::Id(product_id),
                command,
            )
            .await?;
        tx.commit()
            .await
            .map_err(|_| UpdateProductError::CommitTransactionFailed)?;
        Ok(result)
    }

    #[tracing::instrument(
        name = "update_product_by_key",
        skip_all,
        fields(
            shop_id = %product_key.shop_id,
            shops_product_id = %product_key.shops_product_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute_by_key(
        &self,
        context: &OperationContext,
        product_key: ProductKey,
        command: UpdateProductCommand,
    ) -> Result<UpdateProductResult, UpdateProductError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateProductError::BeginTransactionFailed)?;
        let result = self
            .persist_for_target(
                &mut tx,
                context,
                UpdateProductTarget::Key(product_key),
                command,
            )
            .await?;
        tx.commit()
            .await
            .map_err(|_| UpdateProductError::CommitTransactionFailed)?;
        Ok(result)
    }
}

fn apply_command(
    product: &mut product_core::product::Product,
    command: UpdateProductCommand,
    sale_valuation: Option<ProductSaleValuation>,
) -> Result<(), UpdateProductError> {
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
        PatchField::Clear => return Err(UpdateProductError::StateRequired),
    }
    match command.url {
        PatchField::Unchanged => {}
        PatchField::Set(url) => {
            product.change_url(url);
        }
        PatchField::Clear => return Err(UpdateProductError::UrlRequired),
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
    product: &mut product_core::product::Product,
    state: ProductState,
    sale_valuation: Option<ProductSaleValuation>,
) -> Result<(), UpdateProductError> {
    if product.state() == state {
        return Ok(());
    }
    match state {
        ProductState::Listed => product.mark_listed()?,
        ProductState::Available => product.mark_available()?,
        ProductState::Reserved => product.mark_reserved()?,
        ProductState::Sold => {
            product.mark_sold(sale_valuation.ok_or(UpdateProductError::SaleFxSnapshotMissing)?)?
        }
        ProductState::Removed => product.mark_removed()?,
        ProductState::Unknown => product.mark_unknown()?,
    };
    Ok(())
}

fn apply_price_patch(
    product: &mut product_core::product::Product,
    patch: PatchField<Price>,
    apply: impl FnOnce(&mut ProductPricing, Option<Price>),
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
    product: &mut product_core::product::Product,
    patch: PatchField<Option<time::OffsetDateTime>>,
    apply: impl FnOnce(&mut ProductAuction, Option<time::OffsetDateTime>),
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
) -> Result<ProductSaleValuation, UpdateProductError>
where
    F: FxRateSnapshotRepositoryFactory<Tx>,
{
    let mut repository = fx_rates.in_transaction(tx);
    let snapshot = repository
        .find_latest_at_or_before(sold_at)
        .await
        .map_err(UpdateProductError::from)?
        .ok_or(UpdateProductError::SaleFxSnapshotMissing)?;
    Ok(ProductSaleValuation {
        sold_at,
        fx_rate_id: snapshot.id(),
    })
}

impl From<ProductStateTransitionError> for UpdateProductError {
    fn from(error: ProductStateTransitionError) -> Self {
        match error {
            ProductStateTransitionError::SoldProductReopenRequiresExplicitOperation => {
                Self::SoldProductReopenRequiresExplicitOperation
            }
        }
    }
}

impl From<FxRateSnapshotRepositoryError> for UpdateProductError {
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

impl From<OperationAuthorizationError> for UpdateProductError {
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

impl From<PartnerProductAuthorizationError> for UpdateProductError {
    fn from(error: PartnerProductAuthorizationError) -> Self {
        match error {
            PartnerProductAuthorizationError::ShopNotFound => Self::ShopNotFound,
            PartnerProductAuthorizationError::Forbidden => Self::Forbidden,
            PartnerProductAuthorizationError::TemporarilyUnavailable { source } => {
                Self::PartnerProductAuthorizationTemporarilyUnavailable { source }
            }
            PartnerProductAuthorizationError::Internal { source } => {
                Self::PartnerProductAuthorizationInternal { source }
            }
        }
    }
}

impl From<ProductRepositoryError> for UpdateProductError {
    fn from(error: ProductRepositoryError) -> Self {
        match error {
            ProductRepositoryError::ProductCurrentEventIdConflict => {
                Self::ProductCurrentEventIdConflict
            }
            ProductRepositoryError::ShopProductAlreadyExists => Self::ShopProductAlreadyExists,
            ProductRepositoryError::ProductSlugAlreadyExists => Self::ProductSlugAlreadyExists,
            ProductRepositoryError::ProductLookupByIdFailed => Self::ProductLookupByIdFailed,
            ProductRepositoryError::ProductLookupByKeyFailed { source } => {
                Self::ProductLookupByKeyFailed { source }
            }
            ProductRepositoryError::ProductInsertFailed => Self::ProductInsertFailed,
            ProductRepositoryError::ProductUpdateFailed => Self::ProductUpdateFailed,
            ProductRepositoryError::InvalidProductSlugPersisted => {
                Self::InvalidProductSlugPersisted
            }
            ProductRepositoryError::IncompleteTitlePersisted => Self::IncompleteTitlePersisted,
            ProductRepositoryError::InvalidTitleLanguagePersisted => {
                Self::InvalidTitleLanguagePersisted
            }
            ProductRepositoryError::IncompleteDescriptionPersisted => {
                Self::IncompleteDescriptionPersisted
            }
            ProductRepositoryError::InvalidDescriptionLanguagePersisted => {
                Self::InvalidDescriptionLanguagePersisted
            }
            ProductRepositoryError::IncompletePricePersisted => Self::IncompletePricePersisted,
            ProductRepositoryError::NegativePriceAmountPersisted => {
                Self::NegativePriceAmountPersisted
            }
            ProductRepositoryError::InvalidPriceCurrencyPersisted => {
                Self::InvalidPriceCurrencyPersisted
            }
            ProductRepositoryError::InvalidProductStatePersisted => {
                Self::InvalidProductStatePersisted
            }
            ProductRepositoryError::InvalidProductLifecyclePersisted => {
                Self::InvalidProductLifecyclePersisted
            }
            ProductRepositoryError::InvalidProductUrlPersisted => Self::InvalidProductUrlPersisted,
            ProductRepositoryError::InvalidProductImagesPersisted => {
                Self::InvalidProductImagesPersisted
            }
            ProductRepositoryError::InvalidProductImageUrlPersisted => {
                Self::InvalidProductImageUrlPersisted
            }
            ProductRepositoryError::InvalidProductImageProhibitedContentPersisted => {
                Self::InvalidProductImageProhibitedContentPersisted
            }
            ProductRepositoryError::InvalidAggregateStatePersisted => {
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
        _id: common::fx_rate_id::FxRateId,
    ) -> Result<Option<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
        Ok(None)
    }

    async fn find_by_ids(
        &mut self,
        _ids: &[common::fx_rate_id::FxRateId],
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
impl<U, R, E, A> UpdateProductHandler<U, R, E, A, MissingFxRateSnapshotFactory> {
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

impl From<ProductEventStoreError> for UpdateProductError {
    fn from(error: ProductEventStoreError) -> Self {
        match error {
            ProductEventStoreError::ProductEventAlreadyExists => Self::ProductEventAlreadyExists,
            ProductEventStoreError::ProductEventAppendFailed => Self::ProductEventAppendFailed,
            ProductEventStoreError::CurrentProductEventLookupFailed => {
                Self::CurrentProductEventLookupFailed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use localization::Language;
    use localization::Localized;
    use money::Currency;
    use money::{MonetaryAmount, Price};

    use application::transaction::TransactionError;
    use common::shops_product_id::ShopsProductId;
    use common::versioned::Versioned;
    use product_core::description::Description;
    use product_core::product::{NewProduct, Product, ProductDomainEvent};
    use product_core::title::Title;
    use shop_core::shop_id::ShopId;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Debug, Default)]
    struct FakeState {
        begin_error: bool,
        commit_error: bool,
        find_by_id_result:
            Option<Result<Option<Versioned<Product, EventId>>, ProductRepositoryError>>,
        find_by_key_result:
            Option<Result<Option<Versioned<Product, EventId>>, ProductRepositoryError>>,
        update_result: Option<Result<(), ProductRepositoryError>>,
        append_result: Option<Result<(), ProductEventStoreError>>,
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
    struct AllowPartnerProductAuthorizer;

    struct AllowPartnerProductAuthorizerTx;

    #[derive(Clone, Copy)]
    struct DenyPartnerProductAuthorizer;

    struct DenyPartnerProductAuthorizerTx;

    impl PartnerProductAuthorizerFactory<FakeTx> for AllowPartnerProductAuthorizer {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl PartnerProductAuthorizer + 'tx {
            AllowPartnerProductAuthorizerTx
        }
    }

    #[async_trait::async_trait]
    impl PartnerProductAuthorizer for AllowPartnerProductAuthorizerTx {
        async fn authorize(
            &mut self,
            _actor_id: UserId,
            _shop_id: ShopId,
        ) -> Result<(), PartnerProductAuthorizationError> {
            Ok(())
        }
    }

    impl PartnerProductAuthorizerFactory<FakeTx> for DenyPartnerProductAuthorizer {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl PartnerProductAuthorizer + 'tx {
            DenyPartnerProductAuthorizerTx
        }
    }

    #[async_trait::async_trait]
    impl PartnerProductAuthorizer for DenyPartnerProductAuthorizerTx {
        async fn authorize(
            &mut self,
            _actor_id: UserId,
            _shop_id: ShopId,
        ) -> Result<(), PartnerProductAuthorizationError> {
            Err(PartnerProductAuthorizationError::Forbidden)
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

    impl ProductRepositoryFactory<FakeTx> for FakeRepositoryFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl ProductRepository + 'tx {
            FakeRepository {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductRepository for FakeRepository {
        async fn find_by_id(
            &mut self,
            _id: ProductId,
        ) -> Result<Option<Versioned<Product, EventId>>, ProductRepositoryError> {
            let mut state = lock_state(&self.state);
            match state.find_by_id_result.take() {
                Some(result) => result,
                None => Ok(None),
            }
        }

        async fn find_by_key(
            &mut self,
            _key: &ProductKey,
        ) -> Result<Option<Versioned<Product, EventId>>, ProductRepositoryError> {
            match lock_state(&self.state).find_by_key_result.take() {
                Some(result) => result,
                None => Ok(None),
            }
        }

        async fn insert(
            &mut self,
            product: &Product,
            current_event_id: EventId,
        ) -> Result<Versioned<Product, EventId>, ProductRepositoryError> {
            Ok(Versioned::new(product.clone(), current_event_id))
        }

        async fn update(
            &mut self,
            product: &Product,
            _expected_event_id: EventId,
            new_event_id: EventId,
        ) -> Result<Versioned<Product, EventId>, ProductRepositoryError> {
            let mut state = lock_state(&self.state);
            state.update_count += 1;
            match state.update_result.take() {
                Some(Err(error)) => Err(error),
                Some(Ok(())) | None => Ok(Versioned::new(product.clone(), new_event_id)),
            }
        }
    }

    impl ProductEventStoreFactory<FakeTx> for FakeEventStoreFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl ProductEventStore + 'tx {
            FakeEventStore {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductEventStore for FakeEventStore {
        async fn append(
            &mut self,
            _event: &ProductDomainEvent,
        ) -> Result<(), ProductEventStoreError> {
            let mut state = lock_state(&self.state);
            state.append_count += 1;
            match state.append_result.take() {
                Some(result) => result,
                None => Ok(()),
            }
        }

        async fn find_current_event_id(
            &mut self,
            _product_id: ProductId,
        ) -> Result<Option<EventId>, ProductEventStoreError> {
            Ok(None)
        }
    }

    fn handler(
        state: &SharedState,
    ) -> UpdateProductHandler<
        FakeUnitOfWork,
        FakeRepositoryFactory,
        FakeEventStoreFactory,
        AllowPartnerProductAuthorizer,
        MissingFxRateSnapshotFactory,
    > {
        UpdateProductHandler::new(
            uow(state),
            repository_factory(state),
            event_store_factory(state),
            AllowPartnerProductAuthorizer,
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

    fn product() -> Result<Product, url::ParseError> {
        let mut product = Product::create(new_product(ProductId::new())?)
            .map_err(|_| url::ParseError::EmptyHost)?;
        let _events = product.take_pending_events();
        Ok(product)
    }

    fn new_product(product_id: ProductId) -> Result<NewProduct, url::ParseError> {
        Ok(NewProduct {
            id: product_id,
            shop_id: ShopId::new(),
            seller_id: ShopId::new(),
            shops_product_id: ShopsProductId::new(),
            address: ProductAddress::default(),
            title: Some(Localized {
                localization: Language::En,
                payload: Title::from("Cabinet"),
            }),
            description: Some(Localized {
                localization: Language::En,
                payload: Description::from("Old cabinet"),
            }),
            pricing: ProductPricing {
                price: Some(Price::new(MonetaryAmount::from(100_u64), Currency::Eur)),
                ..Default::default()
            },
            sale_valuation: None,
            state: ProductState::Listed,
            url: url("https://shop.example/products/1")?,
            images: IndexSet::new(),
            auction: ProductAuction::default(),
        })
    }

    fn versioned_product(product: Product) -> Versioned<Product, EventId> {
        Versioned::new(product, EventId::new())
    }

    #[test]
    fn should_report_empty_update_when_all_fields_unchanged() {
        let command = UpdateProductCommand {
            ..Default::default()
        };

        assert!(command.is_empty());
    }

    #[tokio::test]
    async fn should_not_persist_or_commit_when_sold_transition_has_no_persisted_fx_snapshot()
    -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductCommand {
            state: PatchField::Set(ProductState::Sold),
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_id, command)
            .await;

        assert!(matches!(
            result,
            Err(UpdateProductError::SaleFxSnapshotMissing)
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
        let product_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductCommand {
            state: PatchField::Set(ProductState::Available),
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_id, command)
            .await;

        assert!(matches!(
            result,
            Ok(UpdateProductResult {
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
        let product_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductCommand {
            state: PatchField::Set(ProductState::Available),
            ..Default::default()
        };
        let handler = UpdateProductHandler::new(
            uow(&state),
            repository_factory(&state),
            event_store_factory(&state),
            DenyPartnerProductAuthorizer,
        );

        let result = handler
            .execute(&partner_context(), product_id, command)
            .await;

        assert!(matches!(result, Err(UpdateProductError::Forbidden)));
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
        let key = ProductKey::new(product.shop_id(), product.shops_product_id().clone());
        lock_state(&state).find_by_key_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductCommand {
            state: PatchField::Set(ProductState::Available),
            ..Default::default()
        };

        let result = handler(&state)
            .execute_by_key(&context(), key, command)
            .await;

        assert!(matches!(
            result,
            Ok(UpdateProductResult {
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
        let product_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductCommand {
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_id, command)
            .await;

        assert!(matches!(
            result,
            Ok(UpdateProductResult { event_id: None, .. })
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
        let product_id = ProductId::new();
        let command = UpdateProductCommand {
            state: PatchField::Set(ProductState::Available),
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_id, command)
            .await;

        assert!(matches!(result, Err(UpdateProductError::ProductNotFound)));
        assert_eq!(0, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_map_begin_error_when_update_begin_fails() {
        let state = state();
        let product_id = ProductId::new();
        lock_state(&state).begin_error = true;
        let command = UpdateProductCommand {
            state: PatchField::Set(ProductState::Available),
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_id, command)
            .await;

        assert!(matches!(
            result,
            Err(UpdateProductError::BeginTransactionFailed)
        ));
    }

    #[tokio::test]
    async fn should_map_commit_error_when_update_commit_fails() -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).commit_error = true;
        let product = product()?;
        let product_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductCommand {
            state: PatchField::Set(ProductState::Available),
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_id, command)
            .await;

        assert!(matches!(
            result,
            Err(UpdateProductError::CommitTransactionFailed)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_when_update_find_fails() {
        let state = state();
        let product_id = ProductId::new();
        lock_state(&state).find_by_id_result =
            Some(Err(ProductRepositoryError::ProductLookupByIdFailed));
        let command = UpdateProductCommand {
            state: PatchField::Set(ProductState::Available),
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_id, command)
            .await;

        assert!(matches!(
            result,
            Err(UpdateProductError::ProductLookupByIdFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_not_commit_when_update_repository_fails() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_id = product.id();
        {
            let mut state = lock_state(&state);
            state.find_by_id_result = Some(Ok(Some(versioned_product(product))));
            state.update_result = Some(Err(ProductRepositoryError::ProductUpdateFailed));
        }
        let command = UpdateProductCommand {
            state: PatchField::Set(ProductState::Available),
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_id, command)
            .await;

        assert!(matches!(
            result,
            Err(UpdateProductError::ProductUpdateFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_when_update_event_append_fails() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_id = product.id();
        {
            let mut state = lock_state(&state);
            state.find_by_id_result = Some(Ok(Some(versioned_product(product))));
            state.append_result = Some(Err(ProductEventStoreError::ProductEventAppendFailed));
        }
        let command = UpdateProductCommand {
            state: PatchField::Set(ProductState::Available),
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_id, command)
            .await;

        assert!(matches!(
            result,
            Err(UpdateProductError::ProductEventAppendFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_apply_all_set_patch_fields_when_update() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductCommand {
            address: PatchField::Set(ProductAddress::default()),
            pricing: PatchField::Set(ProductPricing::default()),
            state: PatchField::Set(ProductState::Available),
            url: PatchField::Set(url("https://shop.example/products/2")?),
            images: PatchField::Set(IndexSet::new()),
            auction: PatchField::Set(ProductAuction {
                start: Some(time::OffsetDateTime::UNIX_EPOCH),
                end: None,
            }),
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_id, command)
            .await;

        assert!(result.is_ok());
        assert!(lock_state(&state).append_count >= 1);
        Ok(())
    }

    #[tokio::test]
    async fn should_apply_clear_patch_fields_when_allowed() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductCommand {
            address: PatchField::Clear,
            pricing: PatchField::Clear,
            images: PatchField::Clear,
            auction: PatchField::Clear,
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_id, command)
            .await;

        assert!(result.is_ok());
        assert_eq!(1, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_clear_state_when_update() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductCommand {
            state: PatchField::Clear,
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_id, command)
            .await;

        assert!(matches!(result, Err(UpdateProductError::StateRequired)));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_clear_url_when_update() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));
        let command = UpdateProductCommand {
            url: PatchField::Clear,
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), product_id, command)
            .await;

        assert!(matches!(result, Err(UpdateProductError::UrlRequired)));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }
}
