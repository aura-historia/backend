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
    BeginTransactionFailed(#[source] application::error::BoxError),
    #[error("failed to commit create product listing transaction")]
    CommitTransactionFailed(#[source] application::error::BoxError),
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

/// A slug collision requires the caller to discard the transaction before retrying.
pub(crate) enum CreateProductListingApplyError {
    SlugCollision,
    Failed(CreateProductListingError),
}

impl From<CreateProductListingError> for CreateProductListingApplyError {
    fn from(error: CreateProductListingError) -> Self {
        Self::Failed(error)
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
    pub(crate) fn generate_title_slug_id(
        &self,
        command: &CreateProductListingCommand,
    ) -> Result<ProductListingSlugId, CreateProductListingError> {
        self.title_slug_generator
            .generate(
                command
                    .title
                    .as_ref()
                    .map_or("", |title| title.payload.as_ref()),
            )
            .map_err(|_| CreateProductListingError::InvalidProductListing)
    }

    pub(crate) async fn apply_in_transaction(
        &self,
        tx: &mut U::Tx,
        context: &OperationContext,
        command: &CreateProductListingCommand,
        product_listing_id: ProductListingId,
        title_slug_id: ProductListingSlugId,
    ) -> Result<CreateProductListingResult, CreateProductListingApplyError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductListingsWrite)
            .authorize::<CreateProductListingError>()?;
        if let Some(actor_id) = partner_actor(&context.principal) {
            self.authorizer
                .in_transaction(tx)
                .authorize(actor_id, command.listing_source_id)
                .await
                .map_err(CreateProductListingError::from)?;
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
                .in_transaction(tx)
                .validate(auction_id, command.listing_source_id)
                .await
                .map_err(CreateProductListingError::from)?;
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
        })
        .map_err(CreateProductListingError::from)?;
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
            .in_transaction(tx)
            .insert(&product, event_id)
            .await
            .map_err(|error| match error {
                ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists => {
                    CreateProductListingApplyError::SlugCollision
                }
                error => CreateProductListingApplyError::Failed(error.into()),
            })?;
        self.events
            .in_transaction(tx)
            .append(&event)
            .await
            .map_err(CreateProductListingError::from)?;
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
            let title_slug_id = self.generate_title_slug_id(&command)?;
            let mut tx = self.unit_of_work.begin().await.map_err(|source| {
                CreateProductListingError::BeginTransactionFailed(Box::new(source))
            })?;
            match self
                .apply_in_transaction(
                    &mut tx,
                    context,
                    &command,
                    product_listing_id,
                    title_slug_id,
                )
                .await
            {
                Ok(result) => {
                    tx.commit().await.map_err(|source| {
                        CreateProductListingError::CommitTransactionFailed(Box::new(source))
                    })?;
                    return Ok(result);
                }
                Err(CreateProductListingApplyError::SlugCollision)
                    if title_slug_collision_retry(attempt, true)
                        == TitleSlugCollisionRetry::Retry =>
                {
                    continue;
                }
                Err(CreateProductListingApplyError::SlugCollision) => {
                    return Err(
                        CreateProductListingError::ProductListingTitleSlugGenerationExhausted,
                    );
                }
                Err(CreateProductListingApplyError::Failed(error)) => return Err(error),
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
        listings: Vec<(ProductListing, EventId)>,
        events: Vec<crate::ports::product_listing_event_appender::ProductListingEvent>,
        follow_up_writes: Vec<ProductListingId>,
    }

    type SharedState = Arc<Mutex<State>>;

    #[derive(Clone)]
    struct UnitOfWorkFake(SharedState);

    struct TransactionFake {
        state: SharedState,
        committed: bool,
        staged_listings: Vec<(ProductListing, EventId)>,
        staged_events: Vec<crate::ports::product_listing_event_appender::ProductListingEvent>,
        staged_follow_up_writes: Vec<ProductListingId>,
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
    struct ProductRepositoryFake<'tx>(SharedState, &'tx mut TransactionFake);
    #[derive(Clone)]
    struct EventsFake(SharedState);
    struct EventAppenderFake<'tx>(SharedState, &'tx mut TransactionFake);
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
                staged_listings: Vec::new(),
                staged_events: Vec::new(),
                staged_follow_up_writes: Vec::new(),
            })
        }
    }

    #[async_trait::async_trait]
    impl Transaction for TransactionFake {
        async fn commit(mut self) -> Result<(), TransactionError> {
            {
                let mut state = lock(&self.state);
                state.listings.append(&mut self.staged_listings);
                state.events.append(&mut self.staged_events);
                state
                    .follow_up_writes
                    .append(&mut self.staged_follow_up_writes);
                state.commits += 1;
            }
            self.committed = true;
            Ok(())
        }
    }

    impl ProductListingRepositoryFactory<TransactionFake> for ProductsFake {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut TransactionFake,
        ) -> impl ProductListingRepository + 'tx {
            ProductRepositoryFake(Arc::clone(&self.0), tx)
        }
    }

    #[async_trait::async_trait]
    impl ProductListingRepository for ProductRepositoryFake<'_> {
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
            event_id: EventId,
        ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
            let mut state = lock(&self.0);
            state.inserts += 1;
            match state.insert_results.pop_front().unwrap_or(Ok(())) {
                Ok(()) => {
                    self.1.staged_listings.push((product.clone(), event_id));
                    Ok(Versioned::new(
                        product.clone(),
                        ProductListingStorageVersion::INITIAL,
                    ))
                }
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
            tx: &'tx mut TransactionFake,
        ) -> impl ProductListingEventAppender + 'tx {
            EventAppenderFake(Arc::clone(&self.0), tx)
        }
    }

    #[async_trait::async_trait]
    impl ProductListingEventAppender for EventAppenderFake<'_> {
        async fn append(
            &mut self,
            event: &crate::ports::product_listing_event_appender::ProductListingEvent,
        ) -> Result<(), ProductListingEventAppendError> {
            let mut state = lock(&self.0);
            state.event_appends += 1;
            state.event_results.pop_front().unwrap_or(Ok(()))?;
            self.1.staged_events.push(event.clone());
            Ok(())
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

    type TestHandler = CreateProductListingHandler<
        UnitOfWorkFake,
        ProductsFake,
        EventsFake,
        AuthorizerFake,
        AuctionValidatorFake,
        GeneratorFake,
    >;

    fn handler(state: &SharedState) -> TestHandler {
        CreateProductListingHandler {
            unit_of_work: UnitOfWorkFake(Arc::clone(state)),
            products: ProductsFake(Arc::clone(state)),
            events: EventsFake(Arc::clone(state)),
            authorizer: AuthorizerFake(Arc::clone(state)),
            auction_references: AuctionValidatorFake(Arc::clone(state)),
            title_slug_generator: GeneratorFake(Arc::clone(state)),
        }
    }

    struct FollowUpWriterFake;

    struct FollowUpWriteFake<'tx>(&'tx mut TransactionFake);

    impl FollowUpWriterFake {
        fn in_transaction<'tx>(&self, tx: &'tx mut TransactionFake) -> FollowUpWriteFake<'tx> {
            FollowUpWriteFake(tx)
        }
    }

    impl FollowUpWriteFake<'_> {
        fn write(&mut self, product_listing_id: ProductListingId) {
            self.0.staged_follow_up_writes.push(product_listing_id);
        }
    }

    enum OwnerOutcome {
        Commit,
        Drop,
        Fail,
    }

    struct ServiceInternalOwner {
        creation: TestHandler,
        follow_up: FollowUpWriterFake,
    }

    impl ServiceInternalOwner {
        async fn execute(
            &self,
            context: &OperationContext,
            command: &CreateProductListingCommand,
            outcome: OwnerOutcome,
        ) -> Result<Option<CreateProductListingResult>, &'static str> {
            let slug = self
                .creation
                .generate_title_slug_id(command)
                .map_err(|_| "slug")?;
            let mut tx = self
                .creation
                .unit_of_work
                .begin()
                .await
                .map_err(|_| "begin")?;
            let result = self
                .creation
                .apply_in_transaction(&mut tx, context, command, ProductListingId::new(), slug)
                .await
                .map_err(|_| "apply")?;
            self.follow_up
                .in_transaction(&mut tx)
                .write(result.product_listing_id);
            match outcome {
                OwnerOutcome::Commit => {
                    tx.commit().await.map_err(|_| "commit")?;
                    Ok(Some(result))
                }
                OwnerOutcome::Drop => {
                    drop(tx);
                    Ok(None)
                }
                OwnerOutcome::Fail => Err("later service step failed"),
            }
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
    async fn should_apply_creation_without_committing_callers_transaction() {
        let state = Arc::new(Mutex::new(State::default()));
        let handler = handler(&state);
        let mut tx = handler
            .unit_of_work
            .begin()
            .await
            .expect("begin transaction");
        let command = command();
        let slug = handler
            .generate_title_slug_id(&command)
            .expect("generate slug");

        let result = handler
            .apply_in_transaction(&mut tx, &context(), &command, ProductListingId::new(), slug)
            .await;

        assert!(matches!(result, Ok(CreateProductListingResult { .. })));
        {
            let state = lock(&state);
            assert_eq!((state.begins, state.commits, state.rollbacks), (1, 0, 0));
            assert_eq!((state.inserts, state.event_appends), (1, 1));
        }
        tx.commit().await.expect("commit caller transaction");
        assert_eq!(lock(&state).commits, 1);
    }

    #[tokio::test]
    async fn should_return_typed_slug_collision_without_committing() {
        let state = Arc::new(Mutex::new(State {
            insert_results: VecDeque::from([Err(
                ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists,
            )]),
            ..Default::default()
        }));
        let handler = handler(&state);
        let mut tx = handler
            .unit_of_work
            .begin()
            .await
            .expect("begin transaction");
        let command = command();
        let slug = handler
            .generate_title_slug_id(&command)
            .expect("generate slug");

        let result = handler
            .apply_in_transaction(&mut tx, &context(), &command, ProductListingId::new(), slug)
            .await;

        assert!(matches!(
            result,
            Err(CreateProductListingApplyError::SlugCollision)
        ));
        drop(tx);
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 0, 1));
        assert_eq!((state.inserts, state.event_appends), (1, 0));
    }

    #[tokio::test]
    async fn should_commit_listing_event_and_follow_up_write_together() {
        let state = Arc::new(Mutex::new(State::default()));
        let owner = ServiceInternalOwner {
            creation: handler(&state),
            follow_up: FollowUpWriterFake,
        };

        let result = owner
            .execute(&context(), &command(), OwnerOutcome::Commit)
            .await
            .expect("owner succeeds")
            .expect("committed result");

        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 1, 0));
        assert_eq!((state.inserts, state.event_appends), (1, 1));
        assert_eq!(state.listings.len(), 1);
        assert_eq!(state.events.len(), 1);
        assert_eq!(state.follow_up_writes.len(), 1);
        assert_eq!(state.listings[0].0.id(), result.product_listing_id);
        assert_eq!(state.listings[0].1, state.events[0].event_id);
        assert_eq!(state.events[0].aggregate_id, result.product_listing_id);
        assert!(matches!(
            state.events[0].payload,
            product_listing_core::product_listing_event::ProductListingEventPayload::Discovered(_)
        ));
        assert_eq!(state.follow_up_writes[0], result.product_listing_id);
    }

    #[tokio::test]
    async fn should_discard_all_staged_writes_on_drop_or_later_failure() {
        for outcome in [OwnerOutcome::Drop, OwnerOutcome::Fail] {
            let state = Arc::new(Mutex::new(State::default()));
            let owner = ServiceInternalOwner {
                creation: handler(&state),
                follow_up: FollowUpWriterFake,
            };

            let result = owner.execute(&context(), &command(), outcome).await;

            assert!(matches!(
                result,
                Ok(None) | Err("later service step failed")
            ));
            let state = lock(&state);
            assert_eq!((state.begins, state.commits, state.rollbacks), (1, 0, 1));
            assert_eq!((state.inserts, state.event_appends), (1, 1));
            assert!(state.listings.is_empty());
            assert!(state.events.is_empty());
            assert!(state.follow_up_writes.is_empty());
        }
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
