use crate::{
    ports::{
        PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
        PartnerProductListingAuthorizerFactory, ProductListingEventAppendError,
        ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
        ProductListingRepositoryError, ProductListingRepositoryFactory, ProductListingWriteEffects,
        stamp_product_listing_event,
    },
    product_listing_auction_patch::{
        ProductListingAuctionPatch, compose_product_listing_auction_patch,
        validate_product_listing_auction_patch,
    },
};
use application::{
    error::BoxError,
    operation_context::{
        CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
    },
    patch_field::PatchField,
    transaction::{Transaction, UnitOfWork},
};
use auction_service::ports::{
    AuctionReferenceValidationError, AuctionReferenceValidator, AuctionReferenceValidatorFactory,
};
use domain_primitives::change_outcome::ChangeOutcome;
use indexmap::IndexSet;
use money::Price;
use product_listing_core::{
    listing_availability::ListingAvailability,
    product_listing::{ChangeListingAvailabilityError, ChangeProductListingError, ProductListing},
    product_listing_id::{ProductListingId, ProductListingKey},
    product_listing_image::ProductListingImage,
    product_listing_price::ProductListingPrice,
};
use url::Url;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateProductListingCommand {
    pub price: PatchField<ProductListingPrice>,
    pub price_estimate_min: PatchField<Price>,
    pub price_estimate_max: PatchField<Price>,
    pub availability: PatchField<ListingAvailability>,
    pub url: PatchField<Url>,
    pub images: PatchField<IndexSet<ProductListingImage>>,
    /// Nested patches preserve omitted lot facts. `auctionId: null` clears membership only.
    pub auction: PatchField<ProductListingAuctionPatch>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateProductListingResult {
    pub product_listing_id: ProductListingId,
    pub outcome: ChangeOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateProductListingError {
    #[error("authenticated actor required to update product listing")]
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
    #[error("product listing not found")]
    NotFound,
    #[error("product listing is withdrawn")]
    ListingWithdrawn,
    #[error("product listing URL is required")]
    UrlRequired,
    #[error("product listing is invalid")]
    InvalidProductListing,
    #[error("product listing persistence failed")]
    PersistenceFailed,
    #[error("product listing event storage failed")]
    EventAppenderFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin update product listing transaction")]
    BeginTransactionFailed,
    #[error("failed to commit update product listing transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpdateProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        product_listing_id: ProductListingId,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError>;
    async fn execute_by_key(
        &self,
        context: &OperationContext,
        product_key: ProductListingKey,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError>;
}

pub struct UpdateProductListingHandler<U, R, E, A, V> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
    auction_references: V,
}
impl<U, R, E, A, V> UpdateProductListingHandler<U, R, E, A, V> {
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
        }
    }
}
enum UpdateTarget {
    Id(ProductListingId),
    Key(ProductListingKey),
}

impl<U, R, E, A, V> UpdateProductListingHandler<U, R, E, A, V>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    V: AuctionReferenceValidatorFactory<U::Tx>,
{
    async fn update(
        &self,
        context: &OperationContext,
        target: UpdateTarget,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductListingsWrite)
            .authorize::<UpdateProductListingError>()?;
        if matches!(command.url, PatchField::Clear) {
            return Err(UpdateProductListingError::UrlRequired);
        }
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateProductListingError::BeginTransactionFailed)?;
        let loaded = match target {
            UpdateTarget::Id(id) => self
                .products
                .in_transaction(&mut tx)
                .find_by_id(id)
                .await?
                .ok_or(UpdateProductListingError::NotFound)?,
            UpdateTarget::Key(key) => self
                .products
                .in_transaction(&mut tx)
                .find_by_key(&key)
                .await?
                .ok_or(UpdateProductListingError::NotFound)?,
        };
        if let Some(actor_id) = partner_actor(&context.principal) {
            self.authorizer
                .in_transaction(&mut tx)
                .authorize(actor_id, loaded.value.listing_source_id())
                .await?;
        }
        let mut product = loaded.value;
        let auction = match &command.auction {
            PatchField::Unchanged | PatchField::Clear => None,
            PatchField::Set(patch) => {
                validate_product_listing_auction_patch(product.auction(), patch)
                    .map_err(|_| UpdateProductListingError::InvalidProductListing)?;
                let auction = compose_product_listing_auction_patch(product.auction(), patch)
                    .map_err(|_| UpdateProductListingError::InvalidProductListing)?;
                if let PatchField::Set(auction_id) = &patch.auction_id {
                    self.auction_references
                        .in_transaction(&mut tx)
                        .validate(*auction_id, product.listing_source_id())
                        .await?;
                }
                Some(auction)
            }
        };
        apply_command(&mut product, command, auction)?;
        let event = product.take_pending_event_payload().map(|payload| {
            stamp_product_listing_event(product.id(), time::OffsetDateTime::now_utc(), payload)
        });
        let outcome = if let Some(event) = event {
            let effects = ProductListingWriteEffects::from(&event.payload);
            self.products
                .in_transaction(&mut tx)
                .update(&product, loaded.version, event.event_id, effects)
                .await?;
            self.events.in_transaction(&mut tx).append(&event).await?;
            ChangeOutcome::Changed
        } else {
            ChangeOutcome::Unchanged
        };
        let id = product.id();
        tx.commit()
            .await
            .map_err(|_| UpdateProductListingError::CommitTransactionFailed)?;
        Ok(UpdateProductListingResult {
            product_listing_id: id,
            outcome,
        })
    }
}

#[async_trait::async_trait]
impl<U, R, E, A, V> UpdateProductListingUseCase for UpdateProductListingHandler<U, R, E, A, V>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    V: AuctionReferenceValidatorFactory<U::Tx>,
{
    async fn execute(
        &self,
        context: &OperationContext,
        product_listing_id: ProductListingId,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError> {
        self.update(context, UpdateTarget::Id(product_listing_id), command)
            .await
    }
    async fn execute_by_key(
        &self,
        context: &OperationContext,
        product_key: ProductListingKey,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError> {
        self.update(context, UpdateTarget::Key(product_key), command)
            .await
    }
}

fn apply_command(
    product: &mut ProductListing,
    command: UpdateProductListingCommand,
    auction: Option<Option<product_listing_core::product_listing::ProductListingAuction>>,
) -> Result<(), UpdateProductListingError> {
    let mut pricing = product.pricing();
    apply_optional_patch(&mut pricing.price, command.price);
    apply_optional_patch(&mut pricing.price_estimate_min, command.price_estimate_min);
    apply_optional_patch(&mut pricing.price_estimate_max, command.price_estimate_max);
    product.replace_pricing(pricing)?;
    match command.availability {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            product.set_availability(value)?;
        }
        PatchField::Clear => {
            product.clear_availability()?;
        }
    }
    match command.url {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            product.change_url(value)?;
        }
        PatchField::Clear => return Err(UpdateProductListingError::UrlRequired),
    }
    match command.images {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            product.replace_images(value)?;
        }
        PatchField::Clear => {
            product.replace_images(Default::default())?;
        }
    }
    if let Some(auction) = auction {
        product.replace_auction(auction)?;
    }
    Ok(())
}
fn apply_optional_patch<T>(field: &mut Option<T>, patch: PatchField<T>) {
    match patch {
        PatchField::Unchanged => {}
        PatchField::Set(value) => *field = Some(value),
        PatchField::Clear => *field = None,
    }
}
fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => Some(*id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}
impl From<OperationAuthorizationError> for UpdateProductListingError {
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
impl From<PartnerProductListingAuthorizationError> for UpdateProductListingError {
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
impl From<AuctionReferenceValidationError> for UpdateProductListingError {
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
impl From<ChangeListingAvailabilityError> for UpdateProductListingError {
    fn from(_: ChangeListingAvailabilityError) -> Self {
        Self::ListingWithdrawn
    }
}
impl From<ChangeProductListingError> for UpdateProductListingError {
    fn from(error: ChangeProductListingError) -> Self {
        match error {
            ChangeProductListingError::ListingWithdrawn => Self::ListingWithdrawn,
            _ => Self::InvalidProductListing,
        }
    }
}
impl From<ProductListingRepositoryError> for UpdateProductListingError {
    fn from(_: ProductListingRepositoryError) -> Self {
        Self::PersistenceFailed
    }
}
impl From<ProductListingEventAppendError> for UpdateProductListingError {
    fn from(error: ProductListingEventAppendError) -> Self {
        Self::EventAppenderFailed {
            source: Box::new(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{ProductListingStorageVersion, VersionedProductListing};
    use application::operation_context::{CorrelationId, RequestId};
    use application::transaction::TransactionError;
    use auction_core::AuctionId;
    use domain_primitives::{event_id::EventId, versioned::Versioned};
    use listing_source_core::ListingSourceId;
    use money::{Currency, MonetaryAmount};
    use product_listing_core::{
        product_listing::{NewProductListing, ProductListingPricing},
        product_listing_slug_id::ProductListingSlugId,
        source_listing_id::SourceListingId,
    };
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Default)]
    struct State {
        begins: usize,
        commits: usize,
        rollbacks: usize,
        updates: usize,
        event_appends: usize,
        authorizations: usize,
        auction_validations: usize,
        finds: VecDeque<Option<VersionedProductListing>>,
        update_results: VecDeque<Result<(), ProductListingRepositoryError>>,
        event_results: VecDeque<Result<(), ProductListingEventAppendError>>,
        authorization_results: VecDeque<Result<(), PartnerProductListingAuthorizationError>>,
        auction_results: VecDeque<Result<(), AuctionReferenceValidationError>>,
        last_updated_lifecycle: Option<product_listing_core::listing_lifecycle::ListingLifecycle>,
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
            Ok(lock(&self.0).finds.pop_front().flatten())
        }

        async fn find_by_key(
            &mut self,
            _: &ProductListingKey,
        ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
            Ok(lock(&self.0).finds.pop_front().flatten())
        }

        async fn insert(
            &mut self,
            _: &ProductListing,
            _: EventId,
        ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
            Err(ProductListingRepositoryError::ProductListingInsertFailed)
        }

        async fn update(
            &mut self,
            product: &ProductListing,
            expected_version: ProductListingStorageVersion,
            _: EventId,
            _: ProductListingWriteEffects,
        ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
            let mut state = lock(&self.0);
            state.updates += 1;
            state.last_updated_lifecycle = Some(product.lifecycle());
            match state.update_results.pop_front().unwrap_or(Ok(())) {
                Ok(()) => Ok(Versioned::new(product.clone(), expected_version.next())),
                Err(error) => Err(error),
            }
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

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::User(UserId::new()),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn listing() -> ProductListing {
        ProductListing::create(NewProductListing {
            id: ProductListingId::new(),
            title_slug_id: ProductListingSlugId::raw("listing-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing slug: {error}")),
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
        })
        .unwrap_or_else(|error| panic!("valid listing: {error}"))
    }

    fn loaded_listing() -> VersionedProductListing {
        let mut listing = listing();
        listing.take_pending_event_payload();
        Versioned::new(listing, ProductListingStorageVersion::INITIAL)
    }

    fn loaded_listing_with_auction() -> VersionedProductListing {
        let mut listing = listing();
        let auction = product_listing_core::product_listing::ProductListingAuction::new(
            Some(AuctionId::new()),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap_or_else(|error| panic!("valid auction fixture: {error}"));
        listing
            .replace_auction(auction)
            .unwrap_or_else(|error| panic!("valid listing auction fixture: {error}"));
        listing.take_pending_event_payload();
        Versioned::new(listing, ProductListingStorageVersion::INITIAL)
    }

    fn withdrawn_listing() -> VersionedProductListing {
        let mut listing = listing();
        listing.take_pending_event_payload();
        listing
            .withdraw()
            .unwrap_or_else(|error| panic!("withdraw fixture: {error}"));
        listing.take_pending_event_payload();
        Versioned::new(listing, ProductListingStorageVersion::INITIAL)
    }

    fn price_update() -> UpdateProductListingCommand {
        UpdateProductListingCommand {
            price: PatchField::Set(ProductListingPrice::from(Price::new(
                MonetaryAmount::from(20u64),
                Currency::Eur,
            ))),
            ..Default::default()
        }
    }

    fn auction_update() -> UpdateProductListingCommand {
        UpdateProductListingCommand {
            auction: PatchField::Set(ProductListingAuctionPatch {
                auction_id: PatchField::Set(AuctionId::new()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn handler(
        state: &SharedState,
    ) -> UpdateProductListingHandler<
        UnitOfWorkFake,
        ProductsFake,
        EventsFake,
        AuthorizerFake,
        AuctionValidatorFake,
    > {
        UpdateProductListingHandler {
            unit_of_work: UnitOfWorkFake(Arc::clone(state)),
            products: ProductsFake(Arc::clone(state)),
            events: EventsFake(Arc::clone(state)),
            authorizer: AuthorizerFake(Arc::clone(state)),
            auction_references: AuctionValidatorFake(Arc::clone(state)),
        }
    }

    fn event_append_failure() -> ProductListingEventAppendError {
        ProductListingEventAppendError::ProductListingEventAppendFailed {
            source: Box::new(std::io::Error::other("event append failed")),
        }
    }

    #[tokio::test]
    async fn should_update_and_commit_on_first_attempt() {
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([Some(loaded_listing())]),
            ..Default::default()
        }));

        let result = handler(&state)
            .execute(&context(), ProductListingId::new(), price_update())
            .await;

        assert!(matches!(
            result,
            Ok(UpdateProductListingResult {
                outcome: ChangeOutcome::Changed,
                ..
            })
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 1, 0));
        assert_eq!(
            (state.updates, state.event_appends, state.authorizations),
            (1, 1, 1)
        );
    }

    #[tokio::test]
    async fn should_commit_without_persistence_for_noop_update() {
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([Some(loaded_listing())]),
            ..Default::default()
        }));

        let result = handler(&state)
            .execute(
                &context(),
                ProductListingId::new(),
                UpdateProductListingCommand::default(),
            )
            .await;

        assert!(matches!(
            result,
            Ok(UpdateProductListingResult {
                outcome: ChangeOutcome::Unchanged,
                ..
            })
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 1, 0));
        assert_eq!((state.updates, state.event_appends), (0, 0));
    }

    #[tokio::test]
    async fn should_reject_mutation_of_withdrawn_listing() {
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([Some(withdrawn_listing())]),
            ..Default::default()
        }));

        let result = handler(&state)
            .execute(&context(), ProductListingId::new(), price_update())
            .await;

        assert!(matches!(
            result,
            Err(UpdateProductListingError::ListingWithdrawn)
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 0, 1));
        assert_eq!((state.updates, state.event_appends), (0, 0));
    }

    #[tokio::test]
    async fn should_reject_anonymous_update_before_beginning_transaction() {
        let state = Arc::new(Mutex::new(State::default()));
        let mut context = context();
        context.principal = Principal::Anonymous;

        let result = handler(&state)
            .execute(&context, ProductListingId::new(), price_update())
            .await;

        assert!(matches!(
            result,
            Err(UpdateProductListingError::AuthenticatedActorRequired)
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (0, 0, 0));
    }

    #[tokio::test]
    async fn should_validate_auction_reference_before_update() {
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([Some(loaded_listing())]),
            auction_results: VecDeque::from([Err(AuctionReferenceValidationError::NotFound)]),
            ..Default::default()
        }));

        let result = handler(&state)
            .execute(&context(), ProductListingId::new(), auction_update())
            .await;

        assert!(matches!(
            result,
            Err(UpdateProductListingError::AuctionNotFound)
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 0, 1));
        assert_eq!(
            (
                state.auction_validations,
                state.updates,
                state.event_appends
            ),
            (1, 0, 0)
        );
    }

    #[tokio::test]
    async fn should_not_revalidate_unchanged_auction_for_lot_only_update() {
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([Some(loaded_listing_with_auction())]),
            ..Default::default()
        }));
        let command = UpdateProductListingCommand {
            auction: PatchField::Set(ProductListingAuctionPatch {
                bidding_opens: PatchField::Set(time::OffsetDateTime::UNIX_EPOCH),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = handler(&state)
            .execute(&context(), ProductListingId::new(), command)
            .await;

        assert!(matches!(
            result,
            Ok(UpdateProductListingResult {
                outcome: ChangeOutcome::Changed,
                ..
            })
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 1, 0));
        assert_eq!(
            (
                state.auction_validations,
                state.updates,
                state.event_appends
            ),
            (0, 1, 1)
        );
    }

    #[tokio::test]
    async fn should_not_commit_when_event_append_fails() {
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([Some(loaded_listing())]),
            event_results: VecDeque::from([Err(event_append_failure())]),
            ..Default::default()
        }));

        let result = handler(&state)
            .execute(&context(), ProductListingId::new(), price_update())
            .await;

        assert!(matches!(
            result,
            Err(UpdateProductListingError::EventAppenderFailed { .. })
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 0, 1));
        assert_eq!((state.updates, state.event_appends), (1, 1));
    }

    #[tokio::test]
    async fn should_stop_when_partner_authorization_fails() {
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([Some(loaded_listing())]),
            authorization_results: VecDeque::from([Err(
                PartnerProductListingAuthorizationError::Forbidden,
            )]),
            ..Default::default()
        }));

        let result = handler(&state)
            .execute(&context(), ProductListingId::new(), price_update())
            .await;

        assert!(matches!(result, Err(UpdateProductListingError::Forbidden)));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 0, 1));
        assert_eq!(
            (state.authorizations, state.updates, state.event_appends),
            (1, 0, 0)
        );
    }
}
