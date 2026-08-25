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
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use indexmap::IndexSet;
use localization::Language;
use localization::Localized;
use product_listing_core::description::Description;
use product_listing_core::product_listing::{
    NewProductListing, ProductListing, ProductListingAddress, ProductListingAuction,
    ProductListingPricing, ProductSaleValuation, RehydrateProductError,
};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_core::product_state::ProductState;
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
    pub state: ProductState,
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
    #[error("authenticated actor required to create product")]
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
    #[error("product already exists for shop product identity")]
    ShopListingAlreadyExists,
    #[error("product slug already exists")]
    ProductListingSlugAlreadyExists,
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
    ProductListingCurrentEventIdConflict,
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
    #[error("failed to begin create product transaction")]
    BeginTransactionFailed,
    #[error("failed to commit create product transaction")]
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

pub struct CreateProductListingHandler<U, R, E, A, F> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
    fx_rates: F,
}

impl<U, R, E, A, F> CreateProductListingHandler<U, R, E, A, F> {
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
impl<U, R, E, A, F> CreateProductListingUseCase for CreateProductListingHandler<U, R, E, A, F>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "create_product",
        skip_all,
        fields(
            shop_id = %command.shop_id,
            shop_listing_id = %command.shop_listing_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
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

        let shop_id = command.shop_id;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CreateProductListingError::BeginTransactionFailed)?;

        if let Some(actor_id) = partner_actor(&context.principal) {
            self.authorizer
                .in_transaction(&mut tx)
                .authorize(actor_id, shop_id)
                .await?;
        }

        let mut input = command.into_new_product(ProductListingId::new());
        if input.state == ProductState::Sold {
            let sold_at = time::OffsetDateTime::now_utc();
            input.sale_valuation = Some(sale_valuation(&self.fx_rates, &mut tx, sold_at).await?);
        }
        let product = ProductListing::create(input)?;
        let event_id = product
            .pending_events()
            .last()
            .map(|event| event.event_id)
            .ok_or(CreateProductListingError::CreatedEventMissing)?;

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
            .map_err(|_| CreateProductListingError::CommitTransactionFailed)?;

        tracing::info!(
            event = "product.created",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            product_listing_id = %product.id(),
            event_id = %event_id,
            outcome = "success",
        );

        CreateProductListingResult::try_from(&persisted_product.value)
    }
}

impl CreateProductListingCommand {
    pub fn into_new_product(self, product_listing_id: ProductListingId) -> NewProductListing {
        NewProductListing {
            id: product_listing_id,
            shop_id: self.shop_id,
            seller_id: self.seller_id,
            shop_listing_id: self.shop_listing_id,
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

impl TryFrom<&ProductListing> for CreateProductListingResult {
    type Error = CreateProductListingError;

    fn try_from(product: &ProductListing) -> Result<Self, Self::Error> {
        let event_id = product
            .pending_events()
            .last()
            .map(|event| event.event_id)
            .ok_or(CreateProductListingError::CreatedEventMissing)?;
        Ok(Self {
            product_listing_id: product.id(),
            product_listing_slug_id: product.slug_id().clone(),
            event_id,
        })
    }
}

async fn sale_valuation<Tx, F>(
    fx_rates: &F,
    tx: &mut Tx,
    sold_at: time::OffsetDateTime,
) -> Result<ProductSaleValuation, CreateProductListingError>
where
    F: FxRateSnapshotRepositoryFactory<Tx>,
{
    let mut repository = fx_rates.in_transaction(tx);
    let snapshot = repository
        .find_latest_at_or_before(sold_at)
        .await
        .map_err(CreateProductListingError::from)?
        .ok_or(CreateProductListingError::SaleFxSnapshotMissing)?;
    Ok(ProductSaleValuation {
        sold_at,
        fx_rate_id: snapshot.id(),
    })
}

impl From<RehydrateProductError> for CreateProductListingError {
    fn from(_error: RehydrateProductError) -> Self {
        Self::InvalidProductState
    }
}

impl From<FxRateSnapshotRepositoryError> for CreateProductListingError {
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

fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}

impl From<PartnerProductListingAuthorizationError> for CreateProductListingError {
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

impl From<ProductListingRepositoryError> for CreateProductListingError {
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
impl<U, R, E, A> CreateProductListingHandler<U, R, E, A, MissingFxRateSnapshotFactory> {
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

impl From<ProductListingEventStoreError> for CreateProductListingError {
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
    use application::transaction::TransactionError;
    use money::Currency;
    use money::{MonetaryAmount, Price};
    use product_listing_core::product_listing::ProductListingDomainEvent;
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
                domain_primitives::versioned::Versioned<ProductListing, EventId>,
                ProductListingRepositoryError,
            >,
        >,
        append_result: Option<Result<(), ProductListingEventStoreError>>,
        inserted_product: Option<ProductListing>,
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
    struct AllowPartnerProductListingAuthorizer;

    struct AllowPartnerProductListingAuthorizerTx;

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
        ) -> Result<
            Option<domain_primitives::versioned::Versioned<ProductListing, EventId>>,
            ProductListingRepositoryError,
        > {
            Ok(None)
        }

        async fn find_by_key(
            &mut self,
            _key: &product_listing_core::product_listing_id::ProductListingKey,
        ) -> Result<
            Option<domain_primitives::versioned::Versioned<ProductListing, EventId>>,
            ProductListingRepositoryError,
        > {
            Ok(None)
        }

        async fn insert(
            &mut self,
            product: &ProductListing,
            current_event_id: EventId,
        ) -> Result<
            domain_primitives::versioned::Versioned<ProductListing, EventId>,
            ProductListingRepositoryError,
        > {
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
            product: &ProductListing,
            _expected_event_id: EventId,
            new_event_id: EventId,
        ) -> Result<
            domain_primitives::versioned::Versioned<ProductListing, EventId>,
            ProductListingRepositoryError,
        > {
            Ok(domain_primitives::versioned::Versioned::new(
                product.clone(),
                new_event_id,
            ))
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

    fn create_command() -> Result<CreateProductListingCommand, url::ParseError> {
        let input = new_product(ProductListingId::new())?;
        Ok(CreateProductListingCommand {
            shop_id: input.shop_id,
            seller_id: input.seller_id,
            shop_listing_id: input.shop_listing_id,
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

    #[tokio::test]
    async fn should_create_product_when_valid() -> Result<(), url::ParseError> {
        let state = state();
        let handler = CreateProductListingHandler::new(
            uow(&state),
            repository_factory(&state),
            event_store_factory(&state),
            AllowPartnerProductListingAuthorizer,
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
        let handler = CreateProductListingHandler::new_with_fx_rates(
            uow(&state),
            repository_factory(&state),
            event_store_factory(&state),
            AllowPartnerProductListingAuthorizer,
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
            state.inserted_product.as_ref().and_then(ProductListing::sale_valuation),
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
        let handler = CreateProductListingHandler::new_with_fx_rates(
            uow(&state),
            repository_factory(&state),
            event_store_factory(&state),
            AllowPartnerProductListingAuthorizer,
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
        let handler = CreateProductListingHandler::new(
            uow(&state),
            repository_factory(&state),
            event_store_factory(&state),
            AllowPartnerProductListingAuthorizer,
        );
        let mut command = create_command()?;
        command.state = ProductState::Sold;

        let result = handler.execute(&context(), command).await;

        assert!(matches!(
            result,
            Err(CreateProductListingError::SaleFxSnapshotMissing)
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
        let handler = CreateProductListingHandler::new(
            uow(&state),
            repository_factory(&state),
            event_store_factory(&state),
            AllowPartnerProductListingAuthorizer,
        );

        let result = handler.execute(&context(), create_command()?).await;

        assert!(matches!(
            result,
            Err(CreateProductListingError::BeginTransactionFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_commit_error_when_create_commit_fails() -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).commit_error = true;
        let handler = CreateProductListingHandler::new(
            uow(&state),
            repository_factory(&state),
            event_store_factory(&state),
            AllowPartnerProductListingAuthorizer,
        );

        let result = handler.execute(&context(), create_command()?).await;

        assert!(matches!(
            result,
            Err(CreateProductListingError::CommitTransactionFailed)
        ));
        assert_eq!(1, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_when_create_insert_fails() -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).insert_result = Some(Err(
            ProductListingRepositoryError::ProductListingInsertFailed,
        ));
        let handler = CreateProductListingHandler::new(
            uow(&state),
            repository_factory(&state),
            event_store_factory(&state),
            AllowPartnerProductListingAuthorizer,
        );

        let result = handler.execute(&context(), create_command()?).await;

        assert!(matches!(
            result,
            Err(CreateProductListingError::ProductListingInsertFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_when_create_event_append_fails() -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).append_result = Some(Err(
            ProductListingEventStoreError::ProductListingEventAppendFailed,
        ));
        let handler = CreateProductListingHandler::new(
            uow(&state),
            repository_factory(&state),
            event_store_factory(&state),
            AllowPartnerProductListingAuthorizer,
        );

        let result = handler.execute(&context(), create_command()?).await;

        assert!(matches!(
            result,
            Err(CreateProductListingError::ProductListingEventAppendFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }
}
