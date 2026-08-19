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
use common::product_id::{ProductId, ProductKey};
use common::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteProductResult {
    pub product_id: ProductId,
    pub event_id: EventId,
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteProductError {
    #[error("authenticated actor required to delete product")]
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
    #[error("failed to begin delete product transaction")]
    BeginTransactionFailed,
    #[error("failed to commit delete product transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait DeleteProductUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        product_id: ProductId,
    ) -> Result<DeleteProductResult, DeleteProductError>;

    async fn execute_by_key(
        &self,
        context: &OperationContext,
        product_key: ProductKey,
    ) -> Result<DeleteProductResult, DeleteProductError>;
}

pub struct DeleteProductHandler<U, R, E, A> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
}

impl<U, R, E, A> DeleteProductHandler<U, R, E, A> {
    pub fn new(unit_of_work: U, products: R, events: E, authorizer: A) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            authorizer,
        }
    }
}

enum DeleteProductTarget {
    Id(ProductId),
    Key(ProductKey),
}

impl<U, R, E, A> DeleteProductHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductRepositoryFactory<U::Tx>,
    E: ProductEventStoreFactory<U::Tx>,
    A: PartnerProductAuthorizerFactory<U::Tx>,
{
    async fn execute_for_target(
        &self,
        context: &OperationContext,
        target: DeleteProductTarget,
    ) -> Result<DeleteProductResult, DeleteProductError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductsWrite)
            .authorize::<DeleteProductError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| DeleteProductError::BeginTransactionFailed)?;
        let loaded = match target {
            DeleteProductTarget::Id(product_id) => self
                .products
                .in_transaction(&mut tx)
                .find_by_id(product_id)
                .await?
                .ok_or(DeleteProductError::ProductNotFound)?,
            DeleteProductTarget::Key(product_key) => {
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
                    .ok_or(DeleteProductError::ProductNotFound)?
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
            .map_err(|_| DeleteProductError::CommitTransactionFailed)?;

        tracing::info!(
            event = "product.deleted",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            product_id = %product.id(),
            event_id = %event_id,
            outcome = "success",
        );

        Ok(DeleteProductResult {
            product_id: product.id(),
            event_id,
        })
    }
}

#[async_trait::async_trait]
impl<U, R, E, A> DeleteProductUseCase for DeleteProductHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductRepositoryFactory<U::Tx>,
    E: ProductEventStoreFactory<U::Tx>,
    A: PartnerProductAuthorizerFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "delete_product",
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
    ) -> Result<DeleteProductResult, DeleteProductError> {
        self.execute_for_target(context, DeleteProductTarget::Id(product_id))
            .await
    }

    #[tracing::instrument(
        name = "delete_product_by_key",
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
    ) -> Result<DeleteProductResult, DeleteProductError> {
        self.execute_for_target(context, DeleteProductTarget::Key(product_key))
            .await
    }
}

impl From<OperationAuthorizationError> for DeleteProductError {
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

impl From<PartnerProductAuthorizationError> for DeleteProductError {
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

impl From<ProductRepositoryError> for DeleteProductError {
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

impl From<ProductEventStoreError> for DeleteProductError {
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
    use application::transaction::TransactionError;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use common::product_lifecycle::domain::ProductLifecycle;
    use common::product_slug_id::ProductSlugId;
    use common::product_state::domain::ProductState;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use common::versioned::Versioned;
    use indexmap::IndexSet;
    use localization::Language;
    use localization::Localized;
    use money::Currency;
    use money::{MonetaryAmount, Price};
    use product_core::description::Description;
    use product_core::product::{
        NewProduct, Product, ProductAddress, ProductAuction, ProductDomainEvent, ProductPricing,
        RehydratedProductState,
    };
    use product_core::title::Title;
    use std::sync::{Arc, Mutex, MutexGuard};
    use url::Url;

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
    ) -> DeleteProductHandler<
        FakeUnitOfWork,
        FakeRepositoryFactory,
        FakeEventStoreFactory,
        AllowPartnerProductAuthorizer,
    > {
        DeleteProductHandler::new(
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

    fn url(value: &str) -> Result<Url, url::ParseError> {
        Url::parse(value)
    }

    fn product() -> Result<Product, url::ParseError> {
        let mut product = Product::create(new_product(ProductId::new())?)
            .map_err(|_| url::ParseError::EmptyHost)?;
        let _events = product.take_pending_events();
        Ok(product)
    }

    fn deleted_product() -> Result<Product, url::ParseError> {
        Product::rehydrate(RehydratedProductState {
            lifecycle: ProductLifecycle::Deleted,
            ..rehydrated_state(ProductId::new())?
        })
        .map_err(|_| url::ParseError::EmptyHost)
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

    fn rehydrated_state(product_id: ProductId) -> Result<RehydratedProductState, url::ParseError> {
        let input = new_product(product_id)?;
        Ok(RehydratedProductState {
            id: input.id,
            slug_id: ProductSlugId::from("cabinet-abcdef"),
            shop_id: input.shop_id,
            seller_id: input.seller_id,
            shops_product_id: input.shops_product_id,
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

    fn versioned_product(product: Product) -> Versioned<Product, EventId> {
        Versioned::new(product, EventId::new())
    }

    #[tokio::test]
    async fn should_delete_product_when_active() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));

        let result = handler(&state).execute(&context(), product_id).await;

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
        let key = ProductKey::new(product.shop_id(), product.shops_product_id().clone());
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
        let product_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));

        let result = handler(&state).execute(&context(), product_id).await;

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

        let result = handler(&state).execute(&context(), ProductId::new()).await;

        assert!(matches!(result, Err(DeleteProductError::ProductNotFound)));
        assert_eq!(0, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_map_begin_error_when_delete_begin_fails() {
        let state = state();
        lock_state(&state).begin_error = true;

        let result = handler(&state).execute(&context(), ProductId::new()).await;

        assert!(matches!(
            result,
            Err(DeleteProductError::BeginTransactionFailed)
        ));
    }

    #[tokio::test]
    async fn should_map_commit_error_when_delete_commit_fails() -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).commit_error = true;
        let product = product()?;
        let product_id = product.id();
        lock_state(&state).find_by_id_result = Some(Ok(Some(versioned_product(product))));

        let result = handler(&state).execute(&context(), product_id).await;

        assert!(matches!(
            result,
            Err(DeleteProductError::CommitTransactionFailed)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_when_delete_find_fails() {
        let state = state();
        lock_state(&state).find_by_id_result =
            Some(Err(ProductRepositoryError::ProductLookupByIdFailed));

        let result = handler(&state).execute(&context(), ProductId::new()).await;

        assert!(matches!(
            result,
            Err(DeleteProductError::ProductLookupByIdFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_not_commit_when_delete_repository_fails() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_id = product.id();
        {
            let mut state = lock_state(&state);
            state.find_by_id_result = Some(Ok(Some(versioned_product(product))));
            state.update_result = Some(Err(ProductRepositoryError::ProductUpdateFailed));
        }

        let result = handler(&state).execute(&context(), product_id).await;

        assert!(matches!(
            result,
            Err(DeleteProductError::ProductUpdateFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_when_delete_event_append_fails() -> Result<(), url::ParseError> {
        let state = state();
        let product = product()?;
        let product_id = product.id();
        {
            let mut state = lock_state(&state);
            state.find_by_id_result = Some(Ok(Some(versioned_product(product))));
            state.append_result = Some(Err(ProductEventStoreError::ProductEventAppendFailed));
        }

        let result = handler(&state).execute(&context(), product_id).await;

        assert!(matches!(
            result,
            Err(DeleteProductError::ProductEventAppendFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }
}
