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
use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteProductListingResult {
    pub product_listing_id: ProductListingId,
    pub event_id: EventId,
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteProductListingError {
    #[error("authenticated actor required to delete product")]
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
    #[error("failed to begin delete product transaction")]
    BeginTransactionFailed,
    #[error("failed to commit delete product transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait DeleteProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        product_listing_id: ProductListingId,
    ) -> Result<DeleteProductListingResult, DeleteProductListingError>;

    async fn execute_by_key(
        &self,
        context: &OperationContext,
        product_key: ProductListingKey,
    ) -> Result<DeleteProductListingResult, DeleteProductListingError>;
}

pub struct DeleteProductListingHandler<U, R, E, A> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
}

impl<U, R, E, A> DeleteProductListingHandler<U, R, E, A> {
    pub fn new(unit_of_work: U, products: R, events: E, authorizer: A) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            authorizer,
        }
    }
}

enum DeleteProductListingTarget {
    Id(ProductListingId),
    Key(ProductListingKey),
}

impl<U, R, E, A> DeleteProductListingHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
{
    async fn execute_for_target(
        &self,
        context: &OperationContext,
        target: DeleteProductListingTarget,
    ) -> Result<DeleteProductListingResult, DeleteProductListingError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductsWrite)
            .authorize::<DeleteProductListingError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| DeleteProductListingError::BeginTransactionFailed)?;
        let loaded = match target {
            DeleteProductListingTarget::Id(product_listing_id) => self
                .products
                .in_transaction(&mut tx)
                .find_by_id(product_listing_id)
                .await?
                .ok_or(DeleteProductListingError::ProductListingNotFound)?,
            DeleteProductListingTarget::Key(product_key) => {
                if let Some(actor_id) = partner_actor(&context.principal) {
                    self.authorizer
                        .in_transaction(&mut tx)
                        .authorize(actor_id, product_key.shop_id)
                        .await?;
                }
                self.products
                    .in_transaction(&mut tx)
                    .find_by_key(&product_key)
                    .await?
                    .ok_or(DeleteProductListingError::ProductListingNotFound)?
            }
        };
        let expected_event_id = loaded.version;
        let mut product = loaded.value;
        product.delete();
        let events = product.take_pending_events();
        let event_id = events
            .last()
            .map(|event| event.event_id)
            .unwrap_or(expected_event_id);

        if !events.is_empty() {
            product = self
                .products
                .in_transaction(&mut tx)
                .update(&product, expected_event_id, event_id)
                .await?
                .value;
            for event in &events {
                self.events.in_transaction(&mut tx).append(event).await?;
            }
        }

        tx.commit()
            .await
            .map_err(|_| DeleteProductListingError::CommitTransactionFailed)?;

        tracing::info!(
            event = "product.deleted",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            product_listing_id = %product.id(),
            event_id = %event_id,
            outcome = "success",
        );

        Ok(DeleteProductListingResult {
            product_listing_id: product.id(),
            event_id,
        })
    }
}

#[async_trait::async_trait]
impl<U, R, E, A> DeleteProductListingUseCase for DeleteProductListingHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "delete_product",
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
    ) -> Result<DeleteProductListingResult, DeleteProductListingError> {
        self.execute_for_target(context, DeleteProductListingTarget::Id(product_listing_id))
            .await
    }

    #[tracing::instrument(
        name = "delete_product_by_key",
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
    ) -> Result<DeleteProductListingResult, DeleteProductListingError> {
        self.execute_for_target(context, DeleteProductListingTarget::Key(product_key))
            .await
    }
}

impl From<OperationAuthorizationError> for DeleteProductListingError {
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

impl From<PartnerProductListingAuthorizationError> for DeleteProductListingError {
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

impl From<ProductListingRepositoryError> for DeleteProductListingError {
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

impl From<ProductListingEventStoreError> for DeleteProductListingError {
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
    use domain_primitives::versioned::Versioned;
    use indexmap::IndexSet;
    use localization::Language;
    use localization::Localized;
    use money::Currency;
    use money::{MonetaryAmount, Price};
    use product_listing_core::description::Description;
    use product_listing_core::product_lifecycle::ProductLifecycle;
    use product_listing_core::product_listing::{
        NewProductListing, ProductListing, ProductListingAddress, ProductListingAuction,
        ProductListingDomainEvent, ProductListingPricing, RehydratedProductListingState,
    };
    use product_listing_core::product_listing_slug_id::ProductListingSlugId;
    use product_listing_core::product_state::ProductState;
    use product_listing_core::shop_listing_id::ShopListingId;
    use product_listing_core::title::Title;
    use shop_core::shop_id::ShopId;
    use std::sync::{Arc, Mutex, MutexGuard};
    use url::Url;

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
    ) -> DeleteProductListingHandler<
        FakeUnitOfWork,
        FakeRepositoryFactory,
        FakeEventStoreFactory,
        AllowPartnerProductListingAuthorizer,
    > {
        DeleteProductListingHandler::new(
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

    fn url(value: &str) -> Result<Url, url::ParseError> {
        Url::parse(value)
    }

    fn product() -> Result<ProductListing, url::ParseError> {
        let mut product = ProductListing::create(new_product(ProductListingId::new())?)
            .map_err(|_| url::ParseError::EmptyHost)?;
        let _events = product.take_pending_events();
        Ok(product)
    }

    fn deleted_product() -> Result<ProductListing, url::ParseError> {
        ProductListing::rehydrate(RehydratedProductListingState {
            lifecycle: ProductLifecycle::Deleted,
            ..rehydrated_state(ProductListingId::new())?
        })
        .map_err(|_| url::ParseError::EmptyHost)
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

    fn rehydrated_state(
        product_listing_id: ProductListingId,
    ) -> Result<RehydratedProductListingState, url::ParseError> {
        let input = new_product(product_listing_id)?;
        Ok(RehydratedProductListingState {
            id: input.id,
            slug_id: ProductListingSlugId::from("cabinet-abcdef"),
            shop_id: input.shop_id,
            seller_id: input.seller_id,
            shop_listing_id: input.shop_listing_id,
            address: input.address,
            title: input.title,
            description: input.description,
            pricing: input.pricing,
            sale_valuation: input.sale_valuation,
            state: input.state,
            lifecycle: ProductLifecycle::Active,
            url: input.url,
            images: input.images,
            auction: input.auction,
        })
    }

    fn versioned_product(product: ProductListing) -> Versioned<ProductListing, EventId> {
        Versioned::new(product, EventId::new())
    }

    #[tokio::test]
    async fn should_delete_product_when_active() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_listing_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));

        let result = handler(&state)
            .execute(&context(), product_listing_id)
            .await;

        assert!(result.is_ok());
        let state = lock_state(&state);
        assert_eq!(1, state.update_count);
        assert_eq!(1, state.append_count);
        assert_eq!(1, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_delete_product_by_partner_key() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let key = ProductListingKey::new(product.shop_id(), product.shop_listing_id().clone());
        lock_state(&state).find_by_key_result = Some(Ok(Some(versioned_product(product))));

        let result = handler(&state).execute_by_key(&context(), key).await;

        assert!(result.is_ok());
        let state = lock_state(&state);
        assert_eq!(1, state.update_count);
        assert_eq!(1, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_commit_idempotent_delete_when_already_deleted() -> Result<(), url::ParseError> {
        let state = state();
        let product = deleted_product()?;
        let product_listing_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));

        let result = handler(&state)
            .execute(&context(), product_listing_id)
            .await;

        assert!(result.is_ok());
        let state = lock_state(&state);
        assert_eq!(0, state.update_count);
        assert_eq!(0, state.append_count);
        assert_eq!(1, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_not_found_when_delete_product_missing() {
        let state = state();

        let result = handler(&state)
            .execute(&context(), ProductListingId::new())
            .await;

        assert!(matches!(
            result,
            Err(DeleteProductListingError::ProductListingNotFound)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_map_begin_error_when_delete_begin_fails() {
        let state = state();
        lock_state(&state).begin_error = true;

        let result = handler(&state)
            .execute(&context(), ProductListingId::new())
            .await;

        assert!(matches!(
            result,
            Err(DeleteProductListingError::BeginTransactionFailed)
        ));
    }

    #[tokio::test]
    async fn should_map_commit_error_when_delete_commit_fails() -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).commit_error = true;
        let product = product()?;
        let product_listing_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));

        let result = handler(&state)
            .execute(&context(), product_listing_id)
            .await;

        assert!(matches!(
            result,
            Err(DeleteProductListingError::CommitTransactionFailed)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_when_delete_find_fails() {
        let state = state();
        lock_state(&state).find_by_id_result = Some(Err(
            ProductListingRepositoryError::ProductListingLookupByIdFailed,
        ));

        let result = handler(&state)
            .execute(&context(), ProductListingId::new())
            .await;

        assert!(matches!(
            result,
            Err(DeleteProductListingError::ProductListingLookupByIdFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_not_commit_when_delete_repository_fails() -> Result<(), url::ParseError> {
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

        let result = handler(&state)
            .execute(&context(), product_listing_id)
            .await;

        assert!(matches!(
            result,
            Err(DeleteProductListingError::ProductListingUpdateFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_when_delete_event_append_fails() -> Result<(), url::ParseError> {
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

        let result = handler(&state)
            .execute(&context(), product_listing_id)
            .await;

        assert!(matches!(
            result,
            Err(DeleteProductListingError::ProductListingEventAppendFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }
}
