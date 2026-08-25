use crate::ports::{
    PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
    PartnerProductListingAuthorizerFactory, ProductListingEventStore,
    ProductListingEventStoreError, ProductListingEventStoreFactory, ProductListingRepository,
    ProductListingRepositoryError, ProductListingRepositoryFactory,
};
use crate::use_cases::commands::create_product_listing::CreateProductListingResult;
use crate::use_cases::commands::update_product_listing::UpdateProductListingResult;
use application::error::{BoxError, box_error};
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};

use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use localization::Language;
use localization::Localized;
use money::Price;
use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};

use application::transaction::{Transaction, UnitOfWork};
use indexmap::IndexSet;
use product_listing_core::description::Description;
use product_listing_core::product_listing::{
    NewProductListing, ProductListing, ProductListingAddress, ProductListingAuction,
    ProductListingPricing, ProductSaleValuation, ProductStateTransitionError,
    RehydrateProductError,
};
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_state::ProductState;
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
    pub state: Option<ProductState>,
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
    #[error("authenticated actor required to upsert product")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("shop not found")]
    ShopNotFound,
    #[error("product current event id did not match expected event id")]
    ProductListingCurrentEventIdConflict,
    #[error("product key already exists")]
    ProductListingKeyAlreadyExists,
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
    #[error("generic upsert cannot reopen a sold product")]
    SoldProductReopenRequiresExplicitOperation,
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
    #[error("product persistence is temporarily unavailable")]
    ProductListingPersistenceTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("persisted product state is invalid")]
    InvalidPersistedProductState {
        #[source]
        source: BoxError,
    },
    #[error("product event storage is temporarily unavailable")]
    ProductListingEventStoreTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin upsert product transaction")]
    BeginTransactionFailed,
    #[error("failed to commit upsert product transaction")]
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

pub struct UpsertProductListingHandler<U, R, E, A, F> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
    fx_rates: F,
}

impl<U, R, E, A, F> UpsertProductListingHandler<U, R, E, A, F> {
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

impl<U, R, E, A, F> UpsertProductListingHandler<U, R, E, A, F>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    async fn persist(
        &self,
        tx: &mut U::Tx,
        context: &OperationContext,
        command: UpsertProductListingCommand,
    ) -> Result<UpsertProductListingResult, UpsertProductListingError> {
        let key = ProductListingKey::new(command.shop_id, command.shop_listing_id.clone());
        if let Some(actor_id) = partner_actor(&context.principal) {
            self.authorizer
                .in_transaction(tx)
                .authorize(actor_id, command.shop_id)
                .await?;
        }

        let existing = self.products.in_transaction(tx).find_by_key(&key).await?;

        let result = match existing {
            Some(loaded) => {
                let expected_event_id = loaded.version;
                let mut product = loaded.value;
                let sale_valuation = if command.state == Some(ProductState::Sold)
                    && product.state() != ProductState::Sold
                {
                    let sold_at = time::OffsetDateTime::now_utc();
                    Some(sale_valuation(&self.fx_rates, tx, sold_at).await?)
                } else {
                    None
                };
                apply_update(&mut product, &command, sale_valuation)?;
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

                UpsertProductListingResult::Updated(UpdateProductListingResult {
                    product_id: product.id(),
                    event_id,
                })
            }
            None => {
                let mut input = command.into_new_product(ProductListingId::new())?;
                if input.state == ProductState::Sold {
                    let sold_at = time::OffsetDateTime::now_utc();
                    input.sale_valuation = Some(sale_valuation(&self.fx_rates, tx, sold_at).await?);
                }
                let product = ProductListing::create(input)?;
                let event_id = product
                    .pending_events()
                    .last()
                    .map(|event| event.event_id)
                    .ok_or(UpsertProductListingError::InvalidProductState)?;
                let persisted = self
                    .products
                    .in_transaction(tx)
                    .insert(&product, event_id)
                    .await?;
                for event in product.pending_events() {
                    self.events.in_transaction(tx).append(event).await?;
                }

                UpsertProductListingResult::Created(CreateProductListingResult {
                    product_id: persisted.value.id(),
                    product_slug_id: persisted.value.slug_id().clone(),
                    event_id,
                })
            }
        };

        Ok(result)
    }

    async fn execute_once(
        &self,
        context: &OperationContext,
        command: UpsertProductListingCommand,
    ) -> Result<UpsertProductListingResult, UpsertProductListingError> {
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
impl<U, R, E, A, F> UpsertProductListingUseCase for UpsertProductListingHandler<U, R, E, A, F>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "upsert_product",
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
        command: UpsertProductListingCommand,
    ) -> Result<UpsertProductListingResult, UpsertProductListingError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductsWrite)
            .authorize::<UpsertProductListingError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let result = match self.execute_once(context, command.clone()).await {
            Err(UpsertProductListingError::ProductListingKeyAlreadyExists) => {
                self.execute_once(context, command).await
            }
            result => result,
        }?;
        tracing::info!(
            event = "product.upserted",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            product_id = %match &result {
                UpsertProductListingResult::Created(value) => value.product_id,
                UpsertProductListingResult::Updated(value) => value.product_id,
            },
            outcome = "success",
        );
        Ok(result)
    }
}

impl UpsertProductListingCommand {
    fn into_new_product(
        self,
        product_id: ProductListingId,
    ) -> Result<NewProductListing, UpsertProductListingError> {
        let url = match self.url {
            Some(url) => url,
            None => Url::parse(MISSING_PRODUCT_URL).map_err(|error| {
                UpsertProductListingError::InvalidPersistedProductState {
                    source: box_error(error),
                }
            })?,
        };
        Ok(NewProductListing {
            id: product_id,
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
            sale_valuation: None,
            state: self.state.unwrap_or(ProductState::Listed),
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
    sale_valuation: Option<ProductSaleValuation>,
) -> Result<(), UpsertProductListingError> {
    let mut pricing = product.pricing();
    let mut pricing_changed = false;
    if let Some(price) = command.price {
        pricing.price = Some(price);
        pricing_changed = true;
    }
    if let Some(price_estimate_min) = command.price_estimate_min {
        pricing.price_estimate_min = Some(price_estimate_min);
        pricing_changed = true;
    }
    if let Some(price_estimate_max) = command.price_estimate_max {
        pricing.price_estimate_max = Some(price_estimate_max);
        pricing_changed = true;
    }
    if pricing_changed {
        product.replace_pricing(pricing);
    }
    if let Some(state) = command.state {
        apply_state(product, state, sale_valuation)?;
    }
    if let Some(url) = &command.url {
        product.change_url(url.clone());
    }
    product.replace_images(command.images.clone());

    if command.auction_start.is_some() || command.auction_end.is_some() {
        let mut auction = product.auction();
        if let Some(start) = command.auction_start {
            auction.start = Some(start);
        }
        if let Some(end) = command.auction_end {
            auction.end = Some(end);
        }
        product.replace_auction(auction);
    }

    Ok(())
}

fn apply_state(
    product: &mut ProductListing,
    state: ProductState,
    sale_valuation: Option<ProductSaleValuation>,
) -> Result<(), UpsertProductListingError> {
    if product.state() == state {
        return Ok(());
    }
    match state {
        ProductState::Listed => product.mark_listed()?,
        ProductState::Available => product.mark_available()?,
        ProductState::Reserved => product.mark_reserved()?,
        ProductState::Sold => product
            .mark_sold(sale_valuation.ok_or(UpsertProductListingError::SaleFxSnapshotMissing)?)?,
        ProductState::Removed => product.mark_removed()?,
        ProductState::Unknown => product.mark_unknown()?,
    };
    Ok(())
}

async fn sale_valuation<Tx, F>(
    fx_rates: &F,
    tx: &mut Tx,
    sold_at: time::OffsetDateTime,
) -> Result<ProductSaleValuation, UpsertProductListingError>
where
    F: FxRateSnapshotRepositoryFactory<Tx>,
{
    let mut repository = fx_rates.in_transaction(tx);
    let snapshot = repository
        .find_latest_at_or_before(sold_at)
        .await
        .map_err(UpsertProductListingError::from)?
        .ok_or(UpsertProductListingError::SaleFxSnapshotMissing)?;
    Ok(ProductSaleValuation {
        sold_at,
        fx_rate_id: snapshot.id(),
    })
}

fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
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
                Self::PartnerProductListingAuthorizationTemporarilyUnavailable { source }
            }
            PartnerProductListingAuthorizationError::Internal { source } => {
                Self::PartnerProductListingAuthorizationInternal { source }
            }
        }
    }
}

impl From<ProductStateTransitionError> for UpsertProductListingError {
    fn from(error: ProductStateTransitionError) -> Self {
        match error {
            ProductStateTransitionError::SoldProductReopenRequiresExplicitOperation => {
                Self::SoldProductReopenRequiresExplicitOperation
            }
        }
    }
}

impl From<FxRateSnapshotRepositoryError> for UpsertProductListingError {
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

impl From<RehydrateProductError> for UpsertProductListingError {
    fn from(error: RehydrateProductError) -> Self {
        Self::InvalidPersistedProductState {
            source: box_error(error),
        }
    }
}

impl From<ProductListingRepositoryError> for UpsertProductListingError {
    fn from(error: ProductListingRepositoryError) -> Self {
        match error {
            ProductListingRepositoryError::ProductListingCurrentEventIdConflict => {
                Self::ProductListingCurrentEventIdConflict
            }
            ProductListingRepositoryError::ShopListingAlreadyExists => {
                Self::ProductListingKeyAlreadyExists
            }
            ProductListingRepositoryError::ProductListingSlugAlreadyExists => {
                Self::ProductListingSlugAlreadyExists
            }
            ProductListingRepositoryError::InvalidProductListingSlugPersisted
            | ProductListingRepositoryError::IncompleteTitlePersisted
            | ProductListingRepositoryError::InvalidTitleLanguagePersisted
            | ProductListingRepositoryError::IncompleteDescriptionPersisted
            | ProductListingRepositoryError::InvalidDescriptionLanguagePersisted
            | ProductListingRepositoryError::IncompletePricePersisted
            | ProductListingRepositoryError::NegativePriceAmountPersisted
            | ProductListingRepositoryError::InvalidPriceCurrencyPersisted
            | ProductListingRepositoryError::InvalidProductStatePersisted
            | ProductListingRepositoryError::InvalidProductLifecyclePersisted
            | ProductListingRepositoryError::InvalidProductListingUrlPersisted
            | ProductListingRepositoryError::InvalidProductListingImagesPersisted
            | ProductListingRepositoryError::InvalidProductListingImageUrlPersisted
            | ProductListingRepositoryError::InvalidProductListingImageProhibitedContentPersisted
            | ProductListingRepositoryError::InvalidAggregateStatePersisted => {
                Self::InvalidPersistedProductState {
                    source: box_error(error),
                }
            }
            ProductListingRepositoryError::ProductListingLookupByIdFailed
            | ProductListingRepositoryError::ProductListingLookupByKeyFailed { .. }
            | ProductListingRepositoryError::ProductListingInsertFailed
            | ProductListingRepositoryError::ProductListingUpdateFailed => {
                Self::ProductListingPersistenceTemporarilyUnavailable {
                    source: box_error(error),
                }
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
impl<U, R, E, A> UpsertProductListingHandler<U, R, E, A, MissingFxRateSnapshotFactory> {
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

impl From<ProductListingEventStoreError> for UpsertProductListingError {
    fn from(error: ProductListingEventStoreError) -> Self {
        match error {
            ProductListingEventStoreError::ProductListingEventAlreadyExists => {
                Self::ProductListingCurrentEventIdConflict
            }
            ProductListingEventStoreError::ProductListingEventAppendFailed
            | ProductListingEventStoreError::CurrentProductListingEventLookupFailed => {
                Self::ProductListingEventStoreTemporarilyUnavailable {
                    source: box_error(error),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::operation_context::{CorrelationId, RequestId};
    use application::transaction::TransactionError;
    use domain_primitives::event_id::EventId;
    use domain_primitives::versioned::Versioned;
    use money::Currency;
    use money::{MonetaryAmount, Price};
    use product_listing_core::product_listing::ProductListingDomainEvent;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Default)]
    struct FakeState {
        begin_count: usize,
        commit_count: usize,
        authorization_count: usize,
        authorization_result: Option<Result<(), PartnerProductListingAuthorizationError>>,
        find_by_key_result: Option<
            Result<Option<Versioned<ProductListing, EventId>>, ProductListingRepositoryError>,
        >,
        retry_find_by_key_result: Option<
            Result<Option<Versioned<ProductListing, EventId>>, ProductListingRepositoryError>,
        >,
        find_by_key_count: usize,
        insert_result: Option<Result<(), ProductListingRepositoryError>>,
        insert_count: usize,
        update_count: usize,
        append_count: usize,
        append_result: Option<Result<(), ProductListingEventStoreError>>,
        last_updated: Option<ProductListing>,
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

    #[derive(Clone)]
    struct FakeAuthorizerFactory {
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

    struct FakeAuthorizer {
        state: SharedState,
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

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            lock_state(&self.state).begin_count += 1;
            Ok(FakeTx {
                state: Arc::clone(&self.state),
            })
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            lock_state(&self.state).commit_count += 1;
            Ok(())
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
            Ok(None)
        }

        async fn find_by_key(
            &mut self,
            _key: &ProductListingKey,
        ) -> Result<Option<Versioned<ProductListing, EventId>>, ProductListingRepositoryError>
        {
            let mut state = lock_state(&self.state);
            state.find_by_key_count += 1;
            match state.find_by_key_count {
                1 => match state.find_by_key_result.take() {
                    Some(result) => result,
                    None => Ok(None),
                },
                _ => match state.retry_find_by_key_result.take() {
                    Some(result) => result,
                    None => Ok(None),
                },
            }
        }

        async fn insert(
            &mut self,
            product: &ProductListing,
            current_event_id: EventId,
        ) -> Result<Versioned<ProductListing, EventId>, ProductListingRepositoryError> {
            let mut state = lock_state(&self.state);
            state.insert_count += 1;
            if let Some(Err(error)) = state.insert_result.take() {
                return Err(error);
            }
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
            state.last_updated = Some(product.clone());
            Ok(Versioned::new(product.clone(), new_event_id))
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
            _product_id: ProductListingId,
        ) -> Result<Option<EventId>, ProductListingEventStoreError> {
            Ok(None)
        }
    }

    impl PartnerProductListingAuthorizerFactory<FakeTx> for FakeAuthorizerFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl PartnerProductListingAuthorizer + 'tx {
            FakeAuthorizer {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl PartnerProductListingAuthorizer for FakeAuthorizer {
        async fn authorize(
            &mut self,
            _actor_id: UserId,
            _shop_id: ShopId,
        ) -> Result<(), PartnerProductListingAuthorizationError> {
            let mut state = lock_state(&self.state);
            state.authorization_count += 1;
            match state.authorization_result.take() {
                Some(result) => result,
                None => Ok(()),
            }
        }
    }

    fn handler(
        state: &SharedState,
    ) -> UpsertProductListingHandler<
        FakeUnitOfWork,
        FakeRepositoryFactory,
        FakeEventStoreFactory,
        FakeAuthorizerFactory,
        MissingFxRateSnapshotFactory,
    > {
        UpsertProductListingHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(state),
            },
            FakeRepositoryFactory {
                state: Arc::clone(state),
            },
            FakeEventStoreFactory {
                state: Arc::clone(state),
            },
            FakeAuthorizerFactory {
                state: Arc::clone(state),
            },
        )
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn command() -> Result<UpsertProductListingCommand, url::ParseError> {
        Ok(UpsertProductListingCommand {
            shop_id: ShopId::new(),
            seller_id: ShopId::new(),
            shop_listing_id: ShopListingId::from("partner-product"),
            address: ProductListingAddress::default(),
            title: Some(Localized::new(Language::En, Title::from("Cabinet"))),
            description: Some(Localized::new(
                Language::En,
                Description::from("Old cabinet"),
            )),
            price: Some(Price::new(MonetaryAmount::from(100_u64), Currency::Eur)),
            price_estimate_min: None,
            price_estimate_max: None,
            state: Some(ProductState::Listed),
            url: Some(Url::parse("https://shop.example/products/1")?),
            images: IndexSet::new(),
            auction_start: None,
            auction_end: None,
        })
    }

    fn existing_product() -> Result<ProductListing, url::ParseError> {
        let mut product = ProductListing::create(NewProductListing {
            id: ProductListingId::new(),
            shop_id: ShopId::new(),
            seller_id: ShopId::new(),
            shop_listing_id: ShopListingId::from("partner-product"),
            address: ProductListingAddress::default(),
            title: Some(Localized::new(Language::En, Title::from("Cabinet"))),
            description: Some(Localized::new(
                Language::En,
                Description::from("Old cabinet"),
            )),
            pricing: ProductListingPricing {
                price: Some(Price::new(MonetaryAmount::from(100_u64), Currency::Eur)),
                ..Default::default()
            },
            sale_valuation: None,
            state: ProductState::Listed,
            url: Url::parse("https://shop.example/products/1")?,
            images: IndexSet::from([ProductListingImage {
                url: Url::parse("https://shop.example/products/1.jpg")?,
                prohibited_content:
                    product_listing_core::prohibited_content::ProhibitedContent::Unknown,
            }]),
            auction: ProductListingAuction::default(),
        })
        .map_err(|_| url::ParseError::EmptyHost)?;
        let _ = product.take_pending_events();
        Ok(product)
    }

    #[tokio::test]
    async fn should_create_and_commit_once_when_product_is_missing() -> Result<(), url::ParseError>
    {
        let state = state();

        let result = handler(&state)
            .execute(&context(Principal::System), command()?)
            .await;

        assert!(matches!(result, Ok(UpsertProductListingResult::Created(_))));
        let state = lock_state(&state);
        assert_eq!(1, state.begin_count);
        assert_eq!(1, state.insert_count);
        assert_eq!(1, state.append_count);
        assert_eq!(1, state.commit_count);
        assert_eq!(0, state.authorization_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_clear_images_and_preserve_omitted_price_when_product_exists()
    -> Result<(), url::ParseError> {
        let state = state();
        let existing = existing_product()?;
        let existing_price = existing.pricing().price;
        lock_state(&state).find_by_key_result =
            Some(Ok(Some(Versioned::new(existing, EventId::new()))));
        let mut input = command()?;
        input.price = None;
        input.images = IndexSet::new();

        let result = handler(&state)
            .execute(&context(Principal::System), input)
            .await;

        assert!(matches!(result, Ok(UpsertProductListingResult::Updated(_))));
        let state = lock_state(&state);
        assert_eq!(1, state.update_count);
        assert_eq!(1, state.commit_count);
        let updated = state
            .last_updated
            .as_ref()
            .ok_or(url::ParseError::EmptyHost)?;
        assert!(updated.images().is_empty());
        assert_eq!(existing_price, updated.pricing().price);
        Ok(())
    }

    #[tokio::test]
    async fn should_retry_as_update_when_concurrent_insert_claims_product_key()
    -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).find_by_key_result = Some(Ok(None));
        lock_state(&state).retry_find_by_key_result = Some(Ok(Some(Versioned::new(
            existing_product()?,
            EventId::new(),
        ))));
        lock_state(&state).insert_result =
            Some(Err(ProductListingRepositoryError::ShopListingAlreadyExists));

        let result = handler(&state)
            .execute(&context(Principal::System), command()?)
            .await;

        assert!(matches!(result, Ok(UpsertProductListingResult::Updated(_))));
        let state = lock_state(&state);
        assert_eq!(2, state.begin_count);
        assert_eq!(1, state.insert_count);
        assert_eq!(1, state.update_count);
        assert_eq!(1, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_when_event_append_fails() -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).append_result = Some(Err(
            ProductListingEventStoreError::ProductListingEventAppendFailed,
        ));

        let result = handler(&state)
            .execute(&context(Principal::System), command()?)
            .await;

        assert!(matches!(
            result,
            Err(UpsertProductListingError::ProductListingEventStoreTemporarilyUnavailable { .. })
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_authorize_inside_transaction_and_not_commit_when_forbidden()
    -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).authorization_result =
            Some(Err(PartnerProductListingAuthorizationError::Forbidden));

        let result = handler(&state)
            .execute(&context(Principal::User(UserId::new())), command()?)
            .await;

        assert!(matches!(result, Err(UpsertProductListingError::Forbidden)));
        let state = lock_state(&state);
        assert_eq!(1, state.begin_count);
        assert_eq!(1, state.authorization_count);
        assert_eq!(0, state.insert_count);
        assert_eq!(0, state.commit_count);
        Ok(())
    }
}
