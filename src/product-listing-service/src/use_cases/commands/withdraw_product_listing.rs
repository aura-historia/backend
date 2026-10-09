use crate::ports::{
    PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
    PartnerProductListingAuthorizerFactory, ProductListingEventAppendError,
    ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
    ProductListingRepositoryError, ProductListingRepositoryFactory, ProductListingWriteEffects,
    stamp_product_listing_event,
};
use application::error::{BoxError, box_error};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::{change_outcome::ChangeOutcome, event_id::EventId};
use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct WithdrawProductListingResult {
    pub product_listing_id: ProductListingId,
    pub outcome: ChangeOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum WithdrawProductListingError {
    #[error("authenticated actor required to withdraw product listing")]
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
    #[error("product listing not found")]
    NotFound,
    #[error("product listing persistence failed")]
    PersistenceFailed,
    #[error("product listing event storage failed")]
    EventAppenderFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin withdraw product listing transaction")]
    BeginTransactionFailed(#[source] application::error::BoxError),
    #[error("failed to commit withdraw product listing transaction")]
    CommitTransactionFailed(#[source] application::error::BoxError),
}

#[async_trait::async_trait]
pub trait WithdrawProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        product_listing_id: ProductListingId,
    ) -> Result<WithdrawProductListingResult, WithdrawProductListingError>;
    async fn execute_by_key(
        &self,
        context: &OperationContext,
        product_key: ProductListingKey,
    ) -> Result<WithdrawProductListingResult, WithdrawProductListingError>;
}

pub struct WithdrawProductListingHandler<U, R, E, A> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
}
impl<U, R, E, A> WithdrawProductListingHandler<U, R, E, A> {
    pub fn new(unit_of_work: U, products: R, events: E, authorizer: A) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            authorizer,
        }
    }
}

pub(crate) enum WithdrawTarget {
    Id(ProductListingId),
    Key(ProductListingKey),
}

impl<U, R, E, A> WithdrawProductListingHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
{
    async fn withdraw(
        &self,
        context: &OperationContext,
        target: WithdrawTarget,
    ) -> Result<WithdrawProductListingResult, WithdrawProductListingError> {
        validate_preconditions(context)?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            WithdrawProductListingError::BeginTransactionFailed(Box::new(source))
        })?;
        let (result, current_event_id) = self.apply_in_tx(&mut tx, context, target).await?;
        tx.commit().await.map_err(|source| {
            WithdrawProductListingError::CommitTransactionFailed(Box::new(source))
        })?;
        tracing::info!(event = "product_listing.withdrawn", actor_type = context.principal.kind(), actor_id = %context.principal.label(), product_listing_id = %result.product_listing_id, event_id = ?current_event_id, outcome = "success");
        Ok(result)
    }

    pub(crate) async fn apply_in_tx(
        &self,
        tx: &mut U::Tx,
        context: &OperationContext,
        target: WithdrawTarget,
    ) -> Result<(WithdrawProductListingResult, Option<EventId>), WithdrawProductListingError> {
        validate_preconditions(context)?;
        let loaded = match target {
            WithdrawTarget::Id(id) => {
                let loaded = self
                    .products
                    .in_transaction(tx)
                    .find_by_id(id)
                    .await?
                    .ok_or(WithdrawProductListingError::NotFound)?;
                if let Some(actor_id) = partner_actor(&context.principal) {
                    self.authorizer
                        .in_transaction(tx)
                        .authorize(actor_id, loaded.value.listing_source_id())
                        .await?;
                }
                loaded
            }
            WithdrawTarget::Key(key) => {
                if let Some(actor_id) = partner_actor(&context.principal) {
                    self.authorizer
                        .in_transaction(tx)
                        .authorize(actor_id, key.listing_source_id)
                        .await?;
                }
                self.products
                    .in_transaction(tx)
                    .find_by_key(&key)
                    .await?
                    .ok_or(WithdrawProductListingError::NotFound)?
            }
        };
        let expected_version = loaded.version;
        let mut product = loaded.value;
        let outcome = product
            .withdraw()
            .map_err(|_| WithdrawProductListingError::PersistenceFailed)?;
        let event = product.take_pending_event_payload().map(|payload| {
            stamp_product_listing_event(product.id(), time::OffsetDateTime::now_utc(), payload)
        });
        let current_event_id = event.as_ref().map(|event| event.event_id);
        if let Some(event) = event {
            let effects = ProductListingWriteEffects::from(&event.payload);
            product = self
                .products
                .in_transaction(tx)
                .update(&product, expected_version, event.event_id, effects)
                .await?
                .value;
            self.events.in_transaction(tx).append(&event).await?;
        }
        Ok((
            WithdrawProductListingResult {
                product_listing_id: product.id(),
                outcome,
            },
            current_event_id,
        ))
    }
}

#[async_trait::async_trait]
impl<U, R, E, A> WithdrawProductListingUseCase for WithdrawProductListingHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
{
    #[tracing::instrument(name = "withdraw_product_listing", skip_all, fields(product_listing_id = %product_listing_id, principal_type = context.principal.kind(), actor_id = tracing::field::Empty, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        product_listing_id: ProductListingId,
    ) -> Result<WithdrawProductListingResult, WithdrawProductListingError> {
        self.withdraw(context, WithdrawTarget::Id(product_listing_id))
            .await
    }

    #[tracing::instrument(name = "withdraw_product_listing_by_key", skip_all, fields(listing_source_id = %product_key.listing_source_id, source_listing_id = %product_key.source_listing_id, principal_type = context.principal.kind(), actor_id = tracing::field::Empty, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute_by_key(
        &self,
        context: &OperationContext,
        product_key: ProductListingKey,
    ) -> Result<WithdrawProductListingResult, WithdrawProductListingError> {
        self.withdraw(context, WithdrawTarget::Key(product_key))
            .await
    }
}

fn validate_preconditions(context: &OperationContext) -> Result<(), WithdrawProductListingError> {
    context
        .require()
        .credential_capability(CredentialCapability::ProductListingsWrite)
        .authorize::<WithdrawProductListingError>()
}

fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}
impl From<OperationAuthorizationError> for WithdrawProductListingError {
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
impl From<PartnerProductListingAuthorizationError> for WithdrawProductListingError {
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
impl From<ProductListingRepositoryError> for WithdrawProductListingError {
    fn from(error: ProductListingRepositoryError) -> Self {
        match error {
            ProductListingRepositoryError::ProductListingLookupByIdFailed
            | ProductListingRepositoryError::ProductListingLookupByKeyFailed { .. }
            | ProductListingRepositoryError::ProductListingInsertFailed
            | ProductListingRepositoryError::ProductListingUpdateFailed
            | ProductListingRepositoryError::ConcurrencyConflict
            | ProductListingRepositoryError::SourceListingAlreadyExists
            | ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists
            | ProductListingRepositoryError::InvalidProductListingSlugPersisted
            | ProductListingRepositoryError::InvalidSourceListingIdPersisted
            | ProductListingRepositoryError::IncompleteTitlePersisted
            | ProductListingRepositoryError::InvalidTitleLanguagePersisted
            | ProductListingRepositoryError::IncompleteDescriptionPersisted
            | ProductListingRepositoryError::InvalidDescriptionLanguagePersisted
            | ProductListingRepositoryError::InvalidProductListingPricePersisted
            | ProductListingRepositoryError::InvalidProductListingPriceKindPersisted
            | ProductListingRepositoryError::IncompletePricePersisted
            | ProductListingRepositoryError::NegativePriceAmountPersisted
            | ProductListingRepositoryError::InvalidPriceCurrencyPersisted
            | ProductListingRepositoryError::InvalidListingAvailabilityPersisted
            | ProductListingRepositoryError::InvalidListingLifecyclePersisted
            | ProductListingRepositoryError::InvalidProductListingUrlPersisted
            | ProductListingRepositoryError::InvalidProductListingImagesPersisted
            | ProductListingRepositoryError::InvalidProductListingImageUrlPersisted
            | ProductListingRepositoryError::InvalidAggregateStatePersisted => {
                Self::PersistenceFailed
            }
        }
    }
}
impl From<ProductListingEventAppendError> for WithdrawProductListingError {
    fn from(error: ProductListingEventAppendError) -> Self {
        Self::EventAppenderFailed {
            source: box_error(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{ProductListingStorageVersion, VersionedProductListing};
    use application::{
        operation_context::{CorrelationId, RequestId},
        transaction::TransactionError,
    };
    use domain_primitives::versioned::Versioned;
    use listing_source_core::ListingSourceId;
    use product_listing_core::{
        product_listing::{NewProductListing, ProductListing, ProductListingPricing},
        product_listing_slug_id::ProductListingSlugId,
        source_listing_id::SourceListingId,
    };
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex, MutexGuard},
    };
    use url::Url;

    #[derive(Default)]
    struct State {
        begins: usize,
        commits: usize,
        rollbacks: usize,
        operations: Vec<&'static str>,
        finds: VecDeque<Option<VersionedProductListing>>,
        authorization_error: bool,
        event_error: bool,
    }
    type SharedState = Arc<Mutex<State>>;

    fn lock(state: &SharedState) -> MutexGuard<'_, State> {
        state.lock().unwrap_or_else(|error| error.into_inner())
    }

    struct UnitOfWorkFake(SharedState);
    struct TransactionFake {
        state: SharedState,
        committed: bool,
    }
    impl Drop for TransactionFake {
        fn drop(&mut self) {
            if !self.committed {
                lock(&self.state).rollbacks += 1;
            }
        }
    }
    #[async_trait::async_trait]
    impl UnitOfWork for UnitOfWorkFake {
        type Tx = TransactionFake;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            lock(&self.0).begins += 1;
            Ok(TransactionFake {
                state: Arc::clone(&self.0),
                committed: false,
            })
        }
    }
    #[async_trait::async_trait]
    impl Transaction for TransactionFake {
        async fn commit(mut self) -> Result<(), TransactionError> {
            self.committed = true;
            lock(&self.state).commits += 1;
            Ok(())
        }
    }

    struct ProductsFake(SharedState);
    struct ProductRepositoryFake(SharedState);
    impl ProductListingRepositoryFactory<TransactionFake> for ProductsFake {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TransactionFake,
        ) -> impl ProductListingRepository + 'tx {
            ProductRepositoryFake(Arc::clone(&self.0))
        }
    }
    #[async_trait::async_trait]
    impl ProductListingRepository for ProductRepositoryFake {
        async fn find_by_id(
            &mut self,
            _: ProductListingId,
        ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
            let mut state = lock(&self.0);
            state.operations.push("find_by_id");
            Ok(state.finds.pop_front().flatten())
        }

        async fn find_by_key(
            &mut self,
            _: &ProductListingKey,
        ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
            let mut state = lock(&self.0);
            state.operations.push("find_by_key");
            Ok(state.finds.pop_front().flatten())
        }

        async fn insert(
            &mut self,
            _: &ProductListing,
            _: EventId,
        ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
            panic!("withdraw must not insert")
        }

        async fn update(
            &mut self,
            product: &ProductListing,
            version: ProductListingStorageVersion,
            _: EventId,
            _: ProductListingWriteEffects,
        ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
            lock(&self.0).operations.push("update");
            Ok(Versioned::new(product.clone(), version.next()))
        }
    }

    struct EventsFake(SharedState);
    struct EventAppenderFake(SharedState);
    impl ProductListingEventAppenderFactory<TransactionFake> for EventsFake {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TransactionFake,
        ) -> impl ProductListingEventAppender + 'tx {
            EventAppenderFake(Arc::clone(&self.0))
        }
    }
    #[async_trait::async_trait]
    impl ProductListingEventAppender for EventAppenderFake {
        async fn append(
            &mut self,
            _: &crate::ports::product_listing_event_appender::ProductListingEvent,
        ) -> Result<(), ProductListingEventAppendError> {
            let mut state = lock(&self.0);
            state.operations.push("append");
            if state.event_error {
                return Err(
                    ProductListingEventAppendError::ProductListingEventAppendFailed {
                        source: Box::new(std::io::Error::other("event append failed")),
                    },
                );
            }
            Ok(())
        }
    }

    struct AuthorizerFake(SharedState);
    struct AuthorizationFake(SharedState);
    impl PartnerProductListingAuthorizerFactory<TransactionFake> for AuthorizerFake {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TransactionFake,
        ) -> impl PartnerProductListingAuthorizer + 'tx {
            AuthorizationFake(Arc::clone(&self.0))
        }
    }
    #[async_trait::async_trait]
    impl PartnerProductListingAuthorizer for AuthorizationFake {
        async fn authorize(
            &mut self,
            _: UserId,
            _: ListingSourceId,
        ) -> Result<(), PartnerProductListingAuthorizationError> {
            let mut state = lock(&self.0);
            state.operations.push("authorize");
            if state.authorization_error {
                return Err(PartnerProductListingAuthorizationError::Forbidden);
            }
            Ok(())
        }
    }

    fn handler(
        state: &SharedState,
    ) -> WithdrawProductListingHandler<UnitOfWorkFake, ProductsFake, EventsFake, AuthorizerFake>
    {
        WithdrawProductListingHandler::new(
            UnitOfWorkFake(Arc::clone(state)),
            ProductsFake(Arc::clone(state)),
            EventsFake(Arc::clone(state)),
            AuthorizerFake(Arc::clone(state)),
        )
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::User(UserId::new()),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn loaded_listing() -> VersionedProductListing {
        let mut product = ProductListing::create(NewProductListing {
            id: ProductListingId::new(),
            title_slug_id: ProductListingSlugId::raw("listing-a1b2c3").expect("valid slug"),
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("source-listing")
                .expect("valid source ID"),
            title: None,
            description: None,
            pricing: ProductListingPricing::default(),
            availability: None,
            url: Url::parse("https://example.com/listing").expect("valid URL"),
            images: Default::default(),
            auction: None,
        })
        .expect("valid listing");
        product.take_pending_event_payload();
        Versioned::new(product, ProductListingStorageVersion::INITIAL)
    }

    fn key() -> ProductListingKey {
        ProductListingKey::new(
            ListingSourceId::new(),
            SourceListingId::try_from("source-listing").expect("valid source ID"),
        )
    }

    #[tokio::test]
    async fn should_apply_in_supplied_transaction_without_committing() {
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([Some(loaded_listing())]),
            ..Default::default()
        }));
        let handler = handler(&state);
        let mut tx = handler
            .unit_of_work
            .begin()
            .await
            .expect("begin transaction");

        let result = handler
            .apply_in_tx(
                &mut tx,
                &context(),
                WithdrawTarget::Id(ProductListingId::new()),
            )
            .await;

        assert!(matches!(
            result,
            Ok((
                WithdrawProductListingResult {
                    outcome: ChangeOutcome::Changed,
                    ..
                },
                Some(_)
            ))
        ));
        {
            let state = lock(&state);
            assert_eq!((state.begins, state.commits, state.rollbacks), (1, 0, 0));
            assert_eq!(
                state.operations,
                ["find_by_id", "authorize", "update", "append"]
            );
        }
        tx.commit().await.expect("commit transaction");
        assert_eq!(lock(&state).commits, 1);
    }

    #[tokio::test]
    async fn should_reject_anonymous_actor_before_begin_and_in_supplied_transaction() {
        let state = Arc::new(Mutex::new(State::default()));
        let handler = handler(&state);
        let mut anonymous = context();
        anonymous.principal = Principal::Anonymous;

        let public_result = handler.execute(&anonymous, ProductListingId::new()).await;
        assert!(matches!(
            public_result,
            Err(WithdrawProductListingError::AuthenticatedActorRequired)
        ));
        assert_eq!(lock(&state).begins, 0);

        let mut tx = handler
            .unit_of_work
            .begin()
            .await
            .expect("begin transaction");
        let internal_result = handler
            .apply_in_tx(
                &mut tx,
                &anonymous,
                WithdrawTarget::Id(ProductListingId::new()),
            )
            .await;
        assert!(matches!(
            internal_result,
            Err(WithdrawProductListingError::AuthenticatedActorRequired)
        ));
        assert!(lock(&state).operations.is_empty());
    }

    #[tokio::test]
    async fn should_preserve_id_and_key_authorization_order() {
        let missing = Arc::new(Mutex::new(State::default()));
        let id_result = handler(&missing)
            .execute(&context(), ProductListingId::new())
            .await;
        assert!(matches!(
            id_result,
            Err(WithdrawProductListingError::NotFound)
        ));
        assert_eq!(lock(&missing).operations, ["find_by_id"]);

        let denied = Arc::new(Mutex::new(State {
            authorization_error: true,
            ..Default::default()
        }));
        let key_result = handler(&denied).execute_by_key(&context(), key()).await;
        assert!(matches!(
            key_result,
            Err(WithdrawProductListingError::Forbidden)
        ));
        assert_eq!(lock(&denied).operations, ["authorize"]);

        let missing_key = Arc::new(Mutex::new(State::default()));
        let key_result = handler(&missing_key)
            .execute_by_key(&context(), key())
            .await;
        assert!(matches!(
            key_result,
            Err(WithdrawProductListingError::NotFound)
        ));
        assert_eq!(lock(&missing_key).operations, ["authorize", "find_by_key"]);
    }

    #[tokio::test]
    async fn should_not_commit_if_event_append_fails() {
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([Some(loaded_listing())]),
            event_error: true,
            ..Default::default()
        }));

        let result = handler(&state)
            .execute(&context(), ProductListingId::new())
            .await;

        assert!(matches!(
            result,
            Err(WithdrawProductListingError::EventAppenderFailed { .. })
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 0, 1));
        assert_eq!(
            state.operations,
            ["find_by_id", "authorize", "update", "append"]
        );
    }
}
