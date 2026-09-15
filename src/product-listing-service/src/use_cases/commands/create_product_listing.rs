use crate::{
    ports::{
        PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
        PartnerProductListingAuthorizerFactory, ProductListingEventAppendError,
        ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
        ProductListingRepositoryError, ProductListingRepositoryFactory,
        stamp_product_listing_event,
    },
    product_listing_auction_patch::{
        ProductListingAuctionPatch, compose_product_listing_auction_patch,
        validate_product_listing_auction_patch,
    },
    product_listing_title_slug_creation::{
        MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS, ProductListingTitleSlugGenerator,
        RandomProductListingTitleSlugGenerator, TitleSlugCollisionRetry,
        title_slug_collision_retry,
    },
};
use application::{
    error::BoxError,
    operation_context::{
        CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
    },
    transaction::{Transaction, UnitOfWork},
};
use auction_service::ports::{
    AuctionReferenceValidationError, AuctionReferenceValidator, AuctionReferenceValidatorFactory,
};

use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use product_listing_core::{
    description::Description,
    listing_availability::ListingAvailability,
    product_listing::{
        NewProductListing, ProductListing, ProductListingAuction, ProductListingPricing,
        RehydrateProductListingError,
    },
    product_listing_id::ProductListingId,
    product_listing_image::ProductListingImage,
    product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId,
    title::Title,
};
use url::Url;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateProductListingCommand {
    pub listing_source_id: ListingSourceId,
    pub source_listing_id: SourceListingId,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductListingPricing,
    pub availability: Option<ListingAvailability>,
    pub url: Url,
    pub images: IndexSet<ProductListingImage>,
    pub auction: Option<ProductListingAuctionPatch>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateProductListingResult {
    pub product_listing_id: ProductListingId,
    pub product_listing_title_slug_id: ProductListingSlugId,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateProductListingError {
    #[error("authenticated actor required to create product listing")]
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
    #[error("auction not found")]
    AuctionNotFound,
    #[error("auction belongs to another listing source")]
    AuctionSourceMismatch,
    #[error("auction reference validation is temporarily unavailable")]
    AuctionReferenceTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("product listing already exists for source listing identity")]
    SourceListingAlreadyExists,
    #[error("product listing title slug already exists")]
    ProductListingTitleSlugAlreadyExists,
    #[error("product listing title slug generation was exhausted")]
    ProductListingTitleSlugGenerationExhausted,
    #[error("new product listing is invalid")]
    InvalidProductListing,
    #[error("created product listing did not record a domain event")]
    CreatedEventMissing,
    #[error("product listing persistence failed")]
    PersistenceFailed,
    #[error("product listing event append failed")]
    EventAppenderFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin create product listing transaction")]
    BeginTransactionFailed,
    #[error("failed to commit create product listing transaction")]
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

pub struct CreateProductListingHandler<U, R, E, A, V, G = RandomProductListingTitleSlugGenerator> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
    auction_references: V,
    title_slug_generator: G,
}

impl<U, R, E, A, V> CreateProductListingHandler<U, R, E, A, V> {
    pub fn new(
        unit_of_work: U,
        products: R,
        events: E,
        authorizer: A,
        auction_references: V,
    ) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            authorizer,
            auction_references,
            title_slug_generator: RandomProductListingTitleSlugGenerator,
        }
    }
}

impl<U, R, E, A, V, G> CreateProductListingHandler<U, R, E, A, V, G>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    V: AuctionReferenceValidatorFactory<U::Tx>,
    G: ProductListingTitleSlugGenerator,
{
    async fn persist_attempt(
        &self,
        context: &OperationContext,
        command: &CreateProductListingCommand,
        product_listing_id: ProductListingId,
        title_slug_id: ProductListingSlugId,
    ) -> Result<CreateProductListingResult, CreateProductListingError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CreateProductListingError::BeginTransactionFailed)?;
        if let Some(actor_id) = partner_actor(&context.principal) {
            self.authorizer
                .in_transaction(&mut tx)
                .authorize(actor_id, command.listing_source_id)
                .await?;
        }
        let auction = command
            .auction
            .as_ref()
            .map(|patch| {
                validate_product_listing_auction_patch(None, patch)
                    .and_then(|_| compose_product_listing_auction_patch(None, patch))
            })
            .transpose()
            .map_err(|_| CreateProductListingError::InvalidProductListing)?
            .flatten();
        if let Some(auction_id) = auction.as_ref().and_then(ProductListingAuction::auction_id) {
            self.auction_references
                .in_transaction(&mut tx)
                .validate(auction_id, command.listing_source_id)
                .await?;
        }
        let mut product = ProductListing::create(NewProductListing {
            id: product_listing_id,
            title_slug_id,
            listing_source_id: command.listing_source_id,
            source_listing_id: command.source_listing_id.clone(),
            title: command.title.clone(),
            description: command.description.clone(),
            pricing: command.pricing,
            availability: command.availability,
            url: command.url.clone(),
            images: command.images.clone(),
            auction,
        })?;
        let event = stamp_product_listing_event(
            product.id(),
            time::OffsetDateTime::now_utc(),
            product
                .take_pending_event_payload()
                .ok_or(CreateProductListingError::CreatedEventMissing)?,
        );
        let event_id = event.event_id;
        let persisted = self
            .products
            .in_transaction(&mut tx)
            .insert(&product, event_id)
            .await?;
        self.events.in_transaction(&mut tx).append(&event).await?;
        tx.commit()
            .await
            .map_err(|_| CreateProductListingError::CommitTransactionFailed)?;
        Ok(CreateProductListingResult {
            product_listing_id: persisted.value.id(),
            product_listing_title_slug_id: persisted.value.title_slug_id().clone(),
        })
    }
}

#[async_trait::async_trait]
impl<U, R, E, A, V, G> CreateProductListingUseCase for CreateProductListingHandler<U, R, E, A, V, G>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    V: AuctionReferenceValidatorFactory<U::Tx>,
    G: ProductListingTitleSlugGenerator,
{
    #[tracing::instrument(name = "create_product_listing", skip_all, fields(listing_source_id = %command.listing_source_id, source_listing_id = %command.source_listing_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateProductListingCommand,
    ) -> Result<CreateProductListingResult, CreateProductListingError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductListingsWrite)
            .authorize::<CreateProductListingError>()?;
        let product_listing_id = ProductListingId::new();
        for attempt in 1..=MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS {
            let title_slug_id = self
                .title_slug_generator
                .generate(
                    command
                        .title
                        .as_ref()
                        .map_or("", |title| title.payload.as_ref()),
                )
                .map_err(|_| CreateProductListingError::InvalidProductListing)?;
            match self
                .persist_attempt(context, &command, product_listing_id, title_slug_id)
                .await
            {
                Ok(result) => return Ok(result),
                Err(CreateProductListingError::ProductListingTitleSlugAlreadyExists)
                    if title_slug_collision_retry(attempt, true)
                        == TitleSlugCollisionRetry::Retry =>
                {
                    continue;
                }
                Err(CreateProductListingError::ProductListingTitleSlugAlreadyExists) => {
                    return Err(
                        CreateProductListingError::ProductListingTitleSlugGenerationExhausted,
                    );
                }
                Err(error) => return Err(error),
            }
        }
        Err(CreateProductListingError::ProductListingTitleSlugGenerationExhausted)
    }
}

fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => Some(*id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
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
impl From<PartnerProductListingAuthorizationError> for CreateProductListingError {
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
impl From<AuctionReferenceValidationError> for CreateProductListingError {
    fn from(error: AuctionReferenceValidationError) -> Self {
        match error {
            AuctionReferenceValidationError::NotFound => Self::AuctionNotFound,
            AuctionReferenceValidationError::ListingSourceMismatch => Self::AuctionSourceMismatch,
            AuctionReferenceValidationError::TemporarilyUnavailable { source } => {
                Self::AuctionReferenceTemporarilyUnavailable { source }
            }
            AuctionReferenceValidationError::InvalidPersistedState { .. } => {
                Self::PersistenceFailed
            }
        }
    }
}
impl From<RehydrateProductListingError> for CreateProductListingError {
    fn from(_: RehydrateProductListingError) -> Self {
        Self::InvalidProductListing
    }
}
impl From<ProductListingRepositoryError> for CreateProductListingError {
    fn from(error: ProductListingRepositoryError) -> Self {
        match error {
            ProductListingRepositoryError::SourceListingAlreadyExists => {
                Self::SourceListingAlreadyExists
            }
            ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists => {
                Self::ProductListingTitleSlugAlreadyExists
            }
            _ => Self::PersistenceFailed,
        }
    }
}
impl From<ProductListingEventAppendError> for CreateProductListingError {
    fn from(error: ProductListingEventAppendError) -> Self {
        Self::EventAppenderFailed {
            source: Box::new(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        ProductListingStorageVersion, ProductListingWriteEffects, VersionedProductListing,
    };
    use application::operation_context::{CorrelationId, RequestId};
    use application::patch_field::PatchField;
    use application::transaction::TransactionError;
    use auction_core::AuctionId;
    use domain_primitives::{event_id::EventId, versioned::Versioned};
    use product_listing_core::{
        product_listing_id::ProductListingKey,
        product_listing_slug_id::{InvalidProductListingSlugId, ProductListingSlugId},
    };
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Default)]
    struct State {
        begins: usize,
        commits: usize,
        rollbacks: usize,
        inserts: usize,
        updates: usize,
        event_appends: usize,
        authorizations: usize,
        auction_validations: usize,
        generated_candidates: usize,
        insert_results: VecDeque<Result<(), ProductListingRepositoryError>>,
        event_results: VecDeque<Result<(), ProductListingEventAppendError>>,
        authorization_results: VecDeque<Result<(), PartnerProductListingAuthorizationError>>,
        auction_results: VecDeque<Result<(), AuctionReferenceValidationError>>,
    }

    type SharedState = Arc<Mutex<State>>;

    #[derive(Clone)]
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

    #[derive(Clone)]
    struct ProductsFake(SharedState);
    struct ProductRepositoryFake(SharedState);
    #[derive(Clone)]
    struct EventsFake(SharedState);
    struct EventAppenderFake(SharedState);
    #[derive(Clone)]
    struct AuthorizerFake(SharedState);
    struct AuthorizationFake(SharedState);
    #[derive(Clone)]
    struct AuctionValidatorFake(SharedState);
    struct AuctionValidationFake(SharedState);
    #[derive(Clone)]
    struct GeneratorFake(SharedState);

    fn lock(state: &SharedState) -> MutexGuard<'_, State> {
        match state.lock() {
            Ok(state) => state,
            Err(error) => error.into_inner(),
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
            Ok(None)
        }

        async fn find_by_key(
            &mut self,
            _: &ProductListingKey,
        ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
            Ok(None)
        }

        async fn insert(
            &mut self,
            product: &ProductListing,
            _: EventId,
        ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
            let mut state = lock(&self.0);
            state.inserts += 1;
            match state.insert_results.pop_front().unwrap_or(Ok(())) {
                Ok(()) => Ok(Versioned::new(
                    product.clone(),
                    ProductListingStorageVersion::INITIAL,
                )),
                Err(error) => Err(error),
            }
        }

        async fn update(
            &mut self,
            product: &ProductListing,
            expected_version: ProductListingStorageVersion,
            _: EventId,
            _: ProductListingWriteEffects,
        ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
            lock(&self.0).updates += 1;
            Ok(Versioned::new(product.clone(), expected_version.next()))
        }
    }

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
            state.event_appends += 1;
            state.event_results.pop_front().unwrap_or(Ok(()))
        }
    }

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
            state.authorizations += 1;
            state.authorization_results.pop_front().unwrap_or(Ok(()))
        }
    }

    impl AuctionReferenceValidatorFactory<TransactionFake> for AuctionValidatorFake {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TransactionFake,
        ) -> impl AuctionReferenceValidator + 'tx {
            AuctionValidationFake(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl AuctionReferenceValidator for AuctionValidationFake {
        async fn validate(
            &mut self,
            _: AuctionId,
            _: ListingSourceId,
        ) -> Result<(), AuctionReferenceValidationError> {
            let mut state = lock(&self.0);
            state.auction_validations += 1;
            state.auction_results.pop_front().unwrap_or(Ok(()))
        }
    }

    impl ProductListingTitleSlugGenerator for GeneratorFake {
        fn generate(&self, _: &str) -> Result<ProductListingSlugId, InvalidProductListingSlugId> {
            let mut state = lock(&self.0);
            let suffix = format!("{:06x}", state.generated_candidates + 1);
            state.generated_candidates += 1;
            ProductListingSlugId::from_title_and_suffix("listing", &suffix)
        }
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::User(UserId::new()),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn command() -> CreateProductListingCommand {
        CreateProductListingCommand {
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("source-listing")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            title: None,
            description: None,
            pricing: ProductListingPricing::default(),
            availability: None,
            url: Url::parse("https://example.com/listing")
                .unwrap_or_else(|error| panic!("valid URL: {error}")),
            images: IndexSet::new(),
            auction: None,
        }
    }

    fn command_with_auction() -> CreateProductListingCommand {
        CreateProductListingCommand {
            auction: Some(ProductListingAuctionPatch {
                auction_id: PatchField::Set(AuctionId::new()),
                ..Default::default()
            }),
            ..command()
        }
    }

    fn handler(
        state: &SharedState,
    ) -> CreateProductListingHandler<
        UnitOfWorkFake,
        ProductsFake,
        EventsFake,
        AuthorizerFake,
        AuctionValidatorFake,
        GeneratorFake,
    > {
        CreateProductListingHandler {
            unit_of_work: UnitOfWorkFake(Arc::clone(state)),
            products: ProductsFake(Arc::clone(state)),
            events: EventsFake(Arc::clone(state)),
            authorizer: AuthorizerFake(Arc::clone(state)),
            auction_references: AuctionValidatorFake(Arc::clone(state)),
            title_slug_generator: GeneratorFake(Arc::clone(state)),
        }
    }

    fn event_append_failure() -> ProductListingEventAppendError {
        ProductListingEventAppendError::ProductListingEventAppendFailed {
            source: Box::new(std::io::Error::other("event append failed")),
        }
    }

    #[tokio::test]
    async fn should_create_and_commit_on_first_attempt() {
        let state = Arc::new(Mutex::new(State::default()));

        let result = handler(&state).execute(&context(), command()).await;

        assert!(matches!(result, Ok(CreateProductListingResult { .. })));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 1, 0));
        assert_eq!(
            (
                state.generated_candidates,
                state.inserts,
                state.event_appends,
                state.authorizations
            ),
            (1, 1, 1, 1)
        );
    }

    #[tokio::test]
    async fn should_exhaust_after_configured_title_slug_collisions() {
        let state = Arc::new(Mutex::new(State {
            insert_results: (0..MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS)
                .map(|_| Err(ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists))
                .collect(),
            ..Default::default()
        }));

        let result = handler(&state).execute(&context(), command()).await;

        assert!(matches!(
            result,
            Err(CreateProductListingError::ProductListingTitleSlugGenerationExhausted)
        ));
        let state = lock(&state);
        assert_eq!(
            (
                state.generated_candidates,
                state.begins,
                state.commits,
                state.rollbacks,
                state.inserts,
                state.event_appends,
                state.authorizations
            ),
            (
                MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS,
                MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS,
                0,
                MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS,
                MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS,
                0,
                MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS
            )
        );
    }

    #[tokio::test]
    async fn should_not_retry_unrelated_persistence_failure() {
        let state = Arc::new(Mutex::new(State {
            insert_results: VecDeque::from([Err(
                ProductListingRepositoryError::ProductListingInsertFailed,
            )]),
            ..Default::default()
        }));

        let result = handler(&state).execute(&context(), command()).await;

        assert!(matches!(
            result,
            Err(CreateProductListingError::PersistenceFailed)
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 0, 1));
        assert_eq!((state.generated_candidates, state.inserts), (1, 1));
        assert_eq!((state.event_appends, state.authorizations), (0, 1));
    }

    #[tokio::test]
    async fn should_not_commit_when_event_append_fails() {
        let state = Arc::new(Mutex::new(State {
            event_results: VecDeque::from([Err(event_append_failure())]),
            ..Default::default()
        }));

        let result = handler(&state).execute(&context(), command()).await;

        assert!(matches!(
            result,
            Err(CreateProductListingError::EventAppenderFailed { .. })
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 0, 1));
        assert_eq!((state.inserts, state.event_appends), (1, 1));
    }

    #[tokio::test]
    async fn should_validate_auction_inside_creation_transaction() {
        let state = Arc::new(Mutex::new(State {
            auction_results: VecDeque::from([Err(AuctionReferenceValidationError::NotFound)]),
            ..Default::default()
        }));

        let result = handler(&state)
            .execute(&context(), command_with_auction())
            .await;

        assert!(matches!(
            result,
            Err(CreateProductListingError::AuctionNotFound)
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 0, 1));
        assert_eq!(
            (
                state.auction_validations,
                state.inserts,
                state.event_appends
            ),
            (1, 0, 0)
        );
    }

    #[tokio::test]
    async fn should_reject_anonymous_creation_before_beginning_transaction() {
        let state = Arc::new(Mutex::new(State::default()));
        let mut context = context();
        context.principal = Principal::Anonymous;

        let result = handler(&state).execute(&context, command()).await;

        assert!(matches!(
            result,
            Err(CreateProductListingError::AuthenticatedActorRequired)
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (0, 0, 0));
        assert_eq!((state.generated_candidates, state.inserts), (0, 0));
    }
}
