use crate::ports::{
    PartnerProductAuthorizationError, PartnerProductAuthorizer, PartnerProductAuthorizerFactory,
    ProductEventStore, ProductEventStoreError, ProductEventStoreFactory, ProductRepository,
    ProductRepositoryError, ProductRepositoryFactory,
};
use application::error::BoxError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use indexmap::IndexSet;
use localization::Language;
use localization::Localized;
use product_core::description::Description;
use product_core::product::{
    NewProduct, Product, ProductAddress, ProductAuction, ProductPricing, ProductSaleValuation,
    RehydrateProductError,
};
use product_core::product_id::ProductId;
use product_core::product_image::ProductImage;
use product_core::product_slug_id::ProductSlugId;
use product_core::product_state::ProductState;
use product_core::shops_product_id::ShopsProductId;
use product_core::title::Title;
use shop_core::shop_id::ShopId;
use url::Url;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateProductCommand {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub address: ProductAddress,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductPricing,
    pub state: ProductState,
    pub url: Url,
    pub images: IndexSet<ProductImage>,
    pub auction: ProductAuction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateProductResult {
    pub product_id: ProductId,
    pub product_slug_id: ProductSlugId,
    pub event_id: EventId,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateProductError {
    #[error("authenticated actor required to create product")]
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
    #[error("product already exists for shop product identity")]
    ShopProductAlreadyExists,
    #[error("product slug already exists")]
    ProductSlugAlreadyExists,
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
    #[error("created product did not record a domain event")]
    CreatedEventMissing,
    #[error("product current event id did not match expected event id")]
    ProductCurrentEventIdConflict,
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
    #[error("failed to begin create product transaction")]
    BeginTransactionFailed,
    #[error("failed to commit create product transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait CreateProductUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateProductCommand,
    ) -> Result<CreateProductResult, CreateProductError>;
}

pub struct CreateProductHandler<U, R, E, A, F> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
    fx_rates: F,
}

impl<U, R, E, A, F> CreateProductHandler<U, R, E, A, F> {
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

#[async_trait::async_trait]
impl<U, R, E, A, F> CreateProductUseCase for CreateProductHandler<U, R, E, A, F>
where
    U: UnitOfWork,
    R: ProductRepositoryFactory<U::Tx>,
    E: ProductEventStoreFactory<U::Tx>,
    A: PartnerProductAuthorizerFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "create_product",
        skip_all,
        fields(
            shop_id = %command.shop_id,
            shops_product_id = %command.shops_product_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateProductCommand,
    ) -> Result<CreateProductResult, CreateProductError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductsWrite)
            .authorize::<CreateProductError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let shop_id = command.shop_id;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CreateProductError::BeginTransactionFailed)?;

        if let Some(actor_id) = partner_actor(&context.principal) {
            self.authorizer
                .in_transaction(&mut tx)
                .authorize(actor_id, shop_id)
                .await?;
        }

        let mut input = command.into_new_product(ProductId::new());
        if input.state == ProductState::Sold {
            let sold_at = time::OffsetDateTime::now_utc();
            input.sale_valuation = Some(sale_valuation(&self.fx_rates, &mut tx, sold_at).await?);
        }
        let product = Product::create(input)?;
        let event_id = product
            .pending_events()
            .last()
            .map(|event| event.event_id)
            .ok_or(CreateProductError::CreatedEventMissing)?;

        let persisted_product = self
            .products
            .in_transaction(&mut tx)
            .insert(&product, event_id)
            .await?;
        for event in product.pending_events() {
            self.events.in_transaction(&mut tx).append(event).await?;
        }

        tx.commit()
            .await
            .map_err(|_| CreateProductError::CommitTransactionFailed)?;

        tracing::info!(
            event = "product.created",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            product_id = %product.id(),
            event_id = %event_id,
            outcome = "success",
        );

        CreateProductResult::try_from(&persisted_product.value)
    }
}

impl CreateProductCommand {
    pub fn into_new_product(self, product_id: ProductId) -> NewProduct {
        NewProduct {
            id: product_id,
            shop_id: self.shop_id,
            seller_id: self.seller_id,
            shops_product_id: self.shops_product_id,
            address: self.address,
            title: self.title,
            description: self.description,
            pricing: self.pricing,
            sale_valuation: None,
            state: self.state,
            url: self.url,
            images: self.images,
            auction: self.auction,
        }
    }
}

impl TryFrom<&Product> for CreateProductResult {
    type Error = CreateProductError;

    fn try_from(product: &Product) -> Result<Self, Self::Error> {
        let event_id = product
            .pending_events()
            .last()
            .map(|event| event.event_id)
            .ok_or(CreateProductError::CreatedEventMissing)?;
        Ok(Self {
            product_id: product.id(),
            product_slug_id: product.slug_id().clone(),
            event_id,
        })
    }
}

async fn sale_valuation<Tx, F>(
    fx_rates: &F,
    tx: &mut Tx,
    sold_at: time::OffsetDateTime,
) -> Result<ProductSaleValuation, CreateProductError>
where
    F: FxRateSnapshotRepositoryFactory<Tx>,
{
    let mut repository = fx_rates.in_transaction(tx);
    let snapshot = repository
        .find_latest_at_or_before(sold_at)
        .await
        .map_err(CreateProductError::from)?
        .ok_or(CreateProductError::SaleFxSnapshotMissing)?;
    Ok(ProductSaleValuation {
        sold_at,
        fx_rate_id: snapshot.id(),
    })
}

impl From<RehydrateProductError> for CreateProductError {
    fn from(_error: RehydrateProductError) -> Self {
        Self::InvalidProductState
    }
}

impl From<FxRateSnapshotRepositoryError> for CreateProductError {
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

impl From<OperationAuthorizationError> for CreateProductError {
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

impl From<PartnerProductAuthorizationError> for CreateProductError {
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

impl From<ProductRepositoryError> for CreateProductError {
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
impl<U, R, E, A> CreateProductHandler<U, R, E, A, MissingFxRateSnapshotFactory> {
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

impl From<ProductEventStoreError> for CreateProductError {
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
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use application::transaction::TransactionError;
    use money::Currency;
    use money::{MonetaryAmount, Price};
    use product_core::product::ProductDomainEvent;
    use std::sync::{Arc, Mutex, MutexGuard};
    use strum::IntoEnumIterator;

    #[derive(Debug, Default)]
    struct FakeState {
        begin_error: bool,
        commit_error: bool,
        begin_count: usize,
        commit_count: usize,
        insert_result: Option<
            Result<
                domain_primitives::versioned::Versioned<Product, EventId>,
                ProductRepositoryError,
            >,
        >,
        append_result: Option<Result<(), ProductEventStoreError>>,
        inserted_product: Option<Product>,
        insert_count: usize,
        append_count: usize,
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

    #[derive(Clone)]
    struct TrackingFxRateSnapshotFactory {
        snapshot: fxrate_core::FxRateSnapshot,
        latest_count: Arc<Mutex<usize>>,
    }

    struct TrackingFxRateSnapshotRepository {
        snapshot: fxrate_core::FxRateSnapshot,
        latest_count: Arc<Mutex<usize>>,
    }

    impl FxRateSnapshotRepositoryFactory<FakeTx> for TrackingFxRateSnapshotFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl FxRateSnapshotRepository + 'tx {
            TrackingFxRateSnapshotRepository {
                snapshot: self.snapshot.clone(),
                latest_count: Arc::clone(&self.latest_count),
            }
        }
    }

    #[async_trait::async_trait]
    impl FxRateSnapshotRepository for TrackingFxRateSnapshotRepository {
        async fn find_latest(
            &mut self,
        ) -> Result<Option<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut count = match self.latest_count.lock() {
                Ok(count) => count,
                Err(poisoned) => poisoned.into_inner(),
            };
            *count += 1;
            Ok(Some(self.snapshot.clone()))
        }

        async fn find_latest_at_or_before(
            &mut self,
            _timestamp: time::OffsetDateTime,
        ) -> Result<Option<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut count = match self.latest_count.lock() {
                Ok(count) => count,
                Err(poisoned) => poisoned.into_inner(),
            };
            *count += 1;
            Ok(Some(self.snapshot.clone()))
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

    #[derive(Clone, Copy)]
    struct AllowPartnerProductAuthorizer;

    struct AllowPartnerProductAuthorizerTx;

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
            let mut state = lock_state(&self.state);
            state.begin_count += 1;
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
        ) -> Result<
            Option<domain_primitives::versioned::Versioned<Product, EventId>>,
            ProductRepositoryError,
        > {
            Ok(None)
        }

        async fn find_by_key(
            &mut self,
            _key: &product_core::product_id::ProductKey,
        ) -> Result<
            Option<domain_primitives::versioned::Versioned<Product, EventId>>,
            ProductRepositoryError,
        > {
            Ok(None)
        }

        async fn insert(
            &mut self,
            product: &Product,
            current_event_id: EventId,
        ) -> Result<domain_primitives::versioned::Versioned<Product, EventId>, ProductRepositoryError>
        {
            let mut state = lock_state(&self.state);
            state.insert_count += 1;
            state.inserted_product = Some(product.clone());
            match state.insert_result.take() {
                Some(result) => result,
                None => Ok(domain_primitives::versioned::Versioned::new(
                    product.clone(),
                    current_event_id,
                )),
            }
        }

        async fn update(
            &mut self,
            product: &Product,
            _expected_event_id: EventId,
            new_event_id: EventId,
        ) -> Result<domain_primitives::versioned::Versioned<Product, EventId>, ProductRepositoryError>
        {
            Ok(domain_primitives::versioned::Versioned::new(
                product.clone(),
                new_event_id,
            ))
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

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn url(value: &str) -> Result<Url, url::ParseError> {
        Url::parse(value)
    }

    fn snapshot() -> fxrate_core::FxRateSnapshot {
        let captured = fxrate_core::NewFxRateSnapshot::capture_eur(
            fxrate_core::FxRateId::new(),
            time::OffsetDateTime::UNIX_EPOCH,
            fxrate_core::FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| {
                fxrate_core::FxRateQuote::new(
                    currency,
                    if currency == Currency::Eur {
                        fxrate_core::FX_RATE_SCALE
                    } else {
                        1_250_000
                    },
                )
            }),
        );
        let captured = match captured {
            Ok(snapshot) => snapshot,
            Err(error) => panic!("test FX snapshot must be valid: {error}"),
        };
        let generation = match fxrate_core::FxRateGeneration::try_from(1) {
            Ok(generation) => generation,
            Err(error) => panic!("test FX generation must be valid: {error}"),
        };
        captured.into_persisted(generation)
    }

    fn create_command() -> Result<CreateProductCommand, url::ParseError> {
        let input = new_product(ProductId::new())?;
        Ok(CreateProductCommand {
            shop_id: input.shop_id,
            seller_id: input.seller_id,
            shops_product_id: input.shops_product_id,
            address: input.address,
            title: input.title,
            description: input.description,
            pricing: input.pricing,
            state: input.state,
            url: input.url,
            images: input.images,
            auction: input.auction,
        })
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

    #[tokio::test]
    async fn should_create_product_when_valid() -> Result<(), url::ParseError> {
        let state = state();
        let handler = CreateProductHandler::new(
            uow(&state),
            repository_factory(&state),
            event_store_factory(&state),
            AllowPartnerProductAuthorizer,
        );

        let result = handler.execute(&context(), create_command()?).await;

        assert!(result.is_ok());
        let state = lock_state(&state);
        assert_eq!(1, state.begin_count);
        assert_eq!(1, state.insert_count);
        assert_eq!(1, state.append_count);
        assert_eq!(1, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_capture_persisted_snapshot_when_creating_sold_product()
    -> Result<(), url::ParseError> {
        let state = state();
        let snapshot = snapshot();
        let latest_count = Arc::new(Mutex::new(0));
        let handler = CreateProductHandler::new_with_fx_rates(
            uow(&state),
            repository_factory(&state),
            event_store_factory(&state),
            AllowPartnerProductAuthorizer,
            TrackingFxRateSnapshotFactory {
                snapshot: snapshot.clone(),
                latest_count: Arc::clone(&latest_count),
            },
        );
        let mut command = create_command()?;
        command.state = ProductState::Sold;

        let result = handler.execute(&context(), command).await;

        assert!(result.is_ok());
        let state = lock_state(&state);
        assert!(matches!(
            state.inserted_product.as_ref().and_then(Product::sale_valuation),
            Some(valuation) if valuation.fx_rate_id == snapshot.id()
        ));
        let latest_count = match latest_count.lock() {
            Ok(count) => *count,
            Err(poisoned) => *poisoned.into_inner(),
        };
        assert_eq!(1, latest_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_read_fx_snapshot_when_creating_non_sold_product()
    -> Result<(), url::ParseError> {
        let state = state();
        let latest_count = Arc::new(Mutex::new(0));
        let handler = CreateProductHandler::new_with_fx_rates(
            uow(&state),
            repository_factory(&state),
            event_store_factory(&state),
            AllowPartnerProductAuthorizer,
            TrackingFxRateSnapshotFactory {
                snapshot: snapshot(),
                latest_count: Arc::clone(&latest_count),
            },
        );

        let result = handler.execute(&context(), create_command()?).await;

        assert!(result.is_ok());
        let latest_count = match latest_count.lock() {
            Ok(count) => *count,
            Err(poisoned) => *poisoned.into_inner(),
        };
        assert_eq!(0, latest_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_persist_or_commit_when_sold_product_has_no_persisted_fx_snapshot()
    -> Result<(), url::ParseError> {
        let state = state();
        let handler = CreateProductHandler::new(
            uow(&state),
            repository_factory(&state),
            event_store_factory(&state),
            AllowPartnerProductAuthorizer,
        );
        let mut command = create_command()?;
        command.state = ProductState::Sold;

        let result = handler.execute(&context(), command).await;

        assert!(matches!(
            result,
            Err(CreateProductError::SaleFxSnapshotMissing)
        ));
        let state = lock_state(&state);
        assert_eq!(0, state.insert_count);
        assert_eq!(0, state.append_count);
        assert_eq!(0, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_begin_error_when_create_begin_fails() -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).begin_error = true;
        let handler = CreateProductHandler::new(
            uow(&state),
            repository_factory(&state),
            event_store_factory(&state),
            AllowPartnerProductAuthorizer,
        );

        let result = handler.execute(&context(), create_command()?).await;

        assert!(matches!(
            result,
            Err(CreateProductError::BeginTransactionFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_commit_error_when_create_commit_fails() -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).commit_error = true;
        let handler = CreateProductHandler::new(
            uow(&state),
            repository_factory(&state),
            event_store_factory(&state),
            AllowPartnerProductAuthorizer,
        );

        let result = handler.execute(&context(), create_command()?).await;

        assert!(matches!(
            result,
            Err(CreateProductError::CommitTransactionFailed)
        ));
        assert_eq!(1, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_when_create_insert_fails() -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).insert_result = Some(Err(ProductRepositoryError::ProductInsertFailed));
        let handler = CreateProductHandler::new(
            uow(&state),
            repository_factory(&state),
            event_store_factory(&state),
            AllowPartnerProductAuthorizer,
        );

        let result = handler.execute(&context(), create_command()?).await;

        assert!(matches!(
            result,
            Err(CreateProductError::ProductInsertFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_when_create_event_append_fails() -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).append_result =
            Some(Err(ProductEventStoreError::ProductEventAppendFailed));
        let handler = CreateProductHandler::new(
            uow(&state),
            repository_factory(&state),
            event_store_factory(&state),
            AllowPartnerProductAuthorizer,
        );

        let result = handler.execute(&context(), create_command()?).await;

        assert!(matches!(
            result,
            Err(CreateProductError::ProductEventAppendFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }
}
