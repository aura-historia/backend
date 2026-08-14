use crate::ports::{
    PartnerProductAuthorizationError, PartnerProductAuthorizer, PartnerProductAuthorizerFactory,
    ProductEventStore, ProductEventStoreError, ProductEventStoreFactory, ProductRepository,
    ProductRepositoryError, ProductRepositoryFactory,
};
use crate::use_cases::commands::create_product::CreateProductResult;
use crate::use_cases::commands::update_product::UpdateProductResult;
use common::error::boxed::{BoxError, box_error};

use common::language::domain::Language;
use common::localized::Localized;
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use common::price::domain::Price;
use common::product_id::{ProductId, ProductKey};

use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;
use indexmap::IndexSet;
use product_core::description::Description;
use product_core::product::{
    NewProduct, Product, ProductAddress, ProductAuction, ProductPricing, RehydrateProductError,
};
use product_core::product_image::ProductImage;
use product_core::title::Title;
use url::Url;

const MISSING_PRODUCT_URL: &str = "https://not-provided.invalid";

#[derive(Debug, Clone, PartialEq)]
pub struct UpsertProductCommand {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub address: ProductAddress,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub price: Option<Price>,
    pub price_estimate_min: Option<Price>,
    pub price_estimate_max: Option<Price>,
    pub state: Option<ProductState>,
    pub url: Option<Url>,
    pub images: IndexSet<ProductImage>,
    pub auction_start: Option<time::OffsetDateTime>,
    pub auction_end: Option<time::OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UpsertProductResult {
    Created(CreateProductResult),
    Updated(UpdateProductResult),
}

#[derive(Debug, thiserror::Error)]
pub enum UpsertProductError {
    #[error("authenticated actor required to upsert product")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("shop not found")]
    ShopNotFound,
    #[error("product current event id did not match expected event id")]
    ProductCurrentEventIdConflict,
    #[error("product key already exists")]
    ProductKeyAlreadyExists,
    #[error("product slug already exists")]
    ProductSlugAlreadyExists,
    #[error("product state is invalid")]
    InvalidProductState,
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
    #[error("product persistence is temporarily unavailable")]
    ProductPersistenceTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("persisted product state is invalid")]
    InvalidPersistedProductState {
        #[source]
        source: BoxError,
    },
    #[error("product event storage is temporarily unavailable")]
    ProductEventStoreTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin upsert product transaction")]
    BeginTransactionFailed,
    #[error("failed to commit upsert product transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpsertProductUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpsertProductCommand,
    ) -> Result<UpsertProductResult, UpsertProductError>;
}

pub struct UpsertProductHandler<U, R, E, A> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
}

impl<U, R, E, A> UpsertProductHandler<U, R, E, A> {
    pub fn new(unit_of_work: U, products: R, events: E, authorizer: A) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            authorizer,
        }
    }
}

impl<U, R, E, A> UpsertProductHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductRepositoryFactory<U::Tx>,
    E: ProductEventStoreFactory<U::Tx>,
    A: PartnerProductAuthorizerFactory<U::Tx>,
{
    async fn persist(
        &self,
        tx: &mut U::Tx,
        context: &OperationContext,
        command: UpsertProductCommand,
    ) -> Result<UpsertProductResult, UpsertProductError> {
        let key = ProductKey::new(command.shop_id, command.shops_product_id.clone());
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
                apply_update(&mut product, &command);
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

                UpsertProductResult::Updated(UpdateProductResult {
                    product_id: product.id(),
                    event_id,
                })
            }
            None => {
                let product = Product::create(command.into_new_product(ProductId::new())?)?;
                let event_id = product
                    .pending_events()
                    .last()
                    .map(|event| event.event_id)
                    .ok_or(UpsertProductError::InvalidProductState)?;
                let persisted = self
                    .products
                    .in_transaction(tx)
                    .insert(&product, event_id)
                    .await?;
                for event in product.pending_events() {
                    self.events.in_transaction(tx).append(event).await?;
                }

                UpsertProductResult::Created(CreateProductResult {
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
        command: UpsertProductCommand,
    ) -> Result<UpsertProductResult, UpsertProductError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpsertProductError::BeginTransactionFailed)?;
        let result = self.persist(&mut tx, context, command).await?;
        tx.commit()
            .await
            .map_err(|_| UpsertProductError::CommitTransactionFailed)?;
        Ok(result)
    }
}

#[async_trait::async_trait]
impl<U, R, E, A> UpsertProductUseCase for UpsertProductHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductRepositoryFactory<U::Tx>,
    E: ProductEventStoreFactory<U::Tx>,
    A: PartnerProductAuthorizerFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "upsert_product",
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
        command: UpsertProductCommand,
    ) -> Result<UpsertProductResult, UpsertProductError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductsWrite)
            .authorize::<UpsertProductError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let result = match self.execute_once(context, command.clone()).await {
            Err(UpsertProductError::ProductKeyAlreadyExists) => {
                self.execute_once(context, command).await
            }
            result => result,
        }?;
        tracing::info!(
            event = "product.upserted",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            product_id = %match &result {
                UpsertProductResult::Created(value) => value.product_id,
                UpsertProductResult::Updated(value) => value.product_id,
            },
            outcome = "success",
        );
        Ok(result)
    }
}

impl UpsertProductCommand {
    fn into_new_product(self, product_id: ProductId) -> Result<NewProduct, UpsertProductError> {
        let url = match self.url {
            Some(url) => url,
            None => Url::parse(MISSING_PRODUCT_URL).map_err(|error| {
                UpsertProductError::InvalidPersistedProductState {
                    source: box_error(error),
                }
            })?,
        };
        Ok(NewProduct {
            id: product_id,
            shop_id: self.shop_id,
            seller_id: self.seller_id,
            shops_product_id: self.shops_product_id,
            address: self.address,
            title: self
                .title
                .or_else(|| Some(Localized::new(Language::En, Title::from("")))),
            description: self.description,
            pricing: ProductPricing {
                price: self.price,
                price_estimate_min: self.price_estimate_min,
                price_estimate_max: self.price_estimate_max,
                fx_rate_id: None,
            },
            state: self.state.unwrap_or(ProductState::Listed),
            url,
            images: self.images,
            auction: ProductAuction {
                start: self.auction_start,
                end: self.auction_end,
            },
        })
    }
}

fn apply_update(product: &mut Product, command: &UpsertProductCommand) {
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
        product.change_state(state);
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
}

fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}

impl From<OperationAuthorizationError> for UpsertProductError {
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

impl From<PartnerProductAuthorizationError> for UpsertProductError {
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

impl From<RehydrateProductError> for UpsertProductError {
    fn from(error: RehydrateProductError) -> Self {
        Self::InvalidPersistedProductState {
            source: box_error(error),
        }
    }
}

impl From<ProductRepositoryError> for UpsertProductError {
    fn from(error: ProductRepositoryError) -> Self {
        match error {
            ProductRepositoryError::ProductCurrentEventIdConflict => {
                Self::ProductCurrentEventIdConflict
            }
            ProductRepositoryError::ShopProductAlreadyExists => Self::ProductKeyAlreadyExists,
            ProductRepositoryError::ProductSlugAlreadyExists => Self::ProductSlugAlreadyExists,
            ProductRepositoryError::InvalidProductSlugPersisted
            | ProductRepositoryError::IncompleteTitlePersisted
            | ProductRepositoryError::InvalidTitleLanguagePersisted
            | ProductRepositoryError::IncompleteDescriptionPersisted
            | ProductRepositoryError::InvalidDescriptionLanguagePersisted
            | ProductRepositoryError::IncompletePricePersisted
            | ProductRepositoryError::NegativePriceAmountPersisted
            | ProductRepositoryError::InvalidPriceCurrencyPersisted
            | ProductRepositoryError::InvalidProductStatePersisted
            | ProductRepositoryError::InvalidProductLifecyclePersisted
            | ProductRepositoryError::InvalidProductUrlPersisted
            | ProductRepositoryError::InvalidProductImagesPersisted
            | ProductRepositoryError::InvalidProductImageUrlPersisted
            | ProductRepositoryError::InvalidProductImageProhibitedContentPersisted
            | ProductRepositoryError::InvalidAggregateStatePersisted => {
                Self::InvalidPersistedProductState {
                    source: box_error(error),
                }
            }
            ProductRepositoryError::ProductLookupByIdFailed
            | ProductRepositoryError::ProductLookupByKeyFailed { .. }
            | ProductRepositoryError::ProductInsertFailed
            | ProductRepositoryError::ProductUpdateFailed => {
                Self::ProductPersistenceTemporarilyUnavailable {
                    source: box_error(error),
                }
            }
        }
    }
}

impl From<ProductEventStoreError> for UpsertProductError {
    fn from(error: ProductEventStoreError) -> Self {
        match error {
            ProductEventStoreError::ProductEventAlreadyExists => {
                Self::ProductCurrentEventIdConflict
            }
            ProductEventStoreError::ProductEventAppendFailed
            | ProductEventStoreError::CurrentProductEventLookupFailed => {
                Self::ProductEventStoreTemporarilyUnavailable {
                    source: box_error(error),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::currency::domain::Currency;
    use common::event_id::EventId;
    use common::operation_context::{CorrelationId, RequestId};
    use common::price::domain::{MonetaryAmount, Price};
    use common::transaction::TransactionError;
    use common::versioned::Versioned;
    use product_core::product::ProductDomainEvent;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Default)]
    struct FakeState {
        begin_count: usize,
        commit_count: usize,
        authorization_count: usize,
        authorization_result: Option<Result<(), PartnerProductAuthorizationError>>,
        find_by_key_result:
            Option<Result<Option<Versioned<Product, EventId>>, ProductRepositoryError>>,
        retry_find_by_key_result:
            Option<Result<Option<Versioned<Product, EventId>>, ProductRepositoryError>>,
        find_by_key_count: usize,
        insert_result: Option<Result<(), ProductRepositoryError>>,
        insert_count: usize,
        update_count: usize,
        append_count: usize,
        append_result: Option<Result<(), ProductEventStoreError>>,
        last_updated: Option<Product>,
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
            Ok(None)
        }

        async fn find_by_key(
            &mut self,
            _key: &ProductKey,
        ) -> Result<Option<Versioned<Product, EventId>>, ProductRepositoryError> {
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
            product: &Product,
            current_event_id: EventId,
        ) -> Result<Versioned<Product, EventId>, ProductRepositoryError> {
            let mut state = lock_state(&self.state);
            state.insert_count += 1;
            if let Some(Err(error)) = state.insert_result.take() {
                return Err(error);
            }
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
            state.last_updated = Some(product.clone());
            Ok(Versioned::new(product.clone(), new_event_id))
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

    impl PartnerProductAuthorizerFactory<FakeTx> for FakeAuthorizerFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl PartnerProductAuthorizer + 'tx {
            FakeAuthorizer {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl PartnerProductAuthorizer for FakeAuthorizer {
        async fn authorize(
            &mut self,
            _actor_id: UserId,
            _shop_id: ShopId,
        ) -> Result<(), PartnerProductAuthorizationError> {
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
    ) -> UpsertProductHandler<
        FakeUnitOfWork,
        FakeRepositoryFactory,
        FakeEventStoreFactory,
        FakeAuthorizerFactory,
    > {
        UpsertProductHandler::new(
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

    fn command() -> Result<UpsertProductCommand, url::ParseError> {
        Ok(UpsertProductCommand {
            shop_id: ShopId::new(),
            seller_id: ShopId::new(),
            shops_product_id: ShopsProductId::from("partner-product"),
            address: ProductAddress::default(),
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

    fn existing_product() -> Result<Product, url::ParseError> {
        let mut product = Product::create(NewProduct {
            id: ProductId::new(),
            shop_id: ShopId::new(),
            seller_id: ShopId::new(),
            shops_product_id: ShopsProductId::from("partner-product"),
            address: ProductAddress::default(),
            title: Some(Localized::new(Language::En, Title::from("Cabinet"))),
            description: Some(Localized::new(
                Language::En,
                Description::from("Old cabinet"),
            )),
            pricing: ProductPricing {
                price: Some(Price::new(MonetaryAmount::from(100_u64), Currency::Eur)),
                ..Default::default()
            },
            state: ProductState::Listed,
            url: Url::parse("https://shop.example/products/1")?,
            images: IndexSet::from([ProductImage {
                url: Url::parse("https://shop.example/products/1.jpg")?,
                prohibited_content: product_core::prohibited_content::ProhibitedContent::Unknown,
            }]),
            auction: ProductAuction::default(),
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

        assert!(matches!(result, Ok(UpsertProductResult::Created(_))));
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

        assert!(matches!(result, Ok(UpsertProductResult::Updated(_))));
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
            Some(Err(ProductRepositoryError::ShopProductAlreadyExists));

        let result = handler(&state)
            .execute(&context(Principal::System), command()?)
            .await;

        assert!(matches!(result, Ok(UpsertProductResult::Updated(_))));
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
        lock_state(&state).append_result =
            Some(Err(ProductEventStoreError::ProductEventAppendFailed));

        let result = handler(&state)
            .execute(&context(Principal::System), command()?)
            .await;

        assert!(matches!(
            result,
            Err(UpsertProductError::ProductEventStoreTemporarilyUnavailable { .. })
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_authorize_inside_transaction_and_not_commit_when_forbidden()
    -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).authorization_result =
            Some(Err(PartnerProductAuthorizationError::Forbidden));

        let result = handler(&state)
            .execute(&context(Principal::User(UserId::new())), command()?)
            .await;

        assert!(matches!(result, Err(UpsertProductError::Forbidden)));
        let state = lock_state(&state);
        assert_eq!(1, state.begin_count);
        assert_eq!(1, state.authorization_count);
        assert_eq!(0, state.insert_count);
        assert_eq!(0, state.commit_count);
        Ok(())
    }
}
