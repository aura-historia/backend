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
    product_listing_title_slug_creation::{
        ProductListingTitleSlugGenerator, RandomProductListingTitleSlugGenerator,
        TitleSlugCollisionRetry, title_slug_collision_retry,
    },
    use_cases::{CreateProductListingResult, UpdateProductListingResult},
};
use application::{
    error::{BoxError, box_error},
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
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use money::Price;
use product_listing_core::{
    description::Description,
    listing_availability::ListingAvailability,
    product_listing::{
        ChangeListingAvailabilityError, ChangeProductListingError, NewProductListing,
        ProductListing, ProductListingPricing, RehydrateProductListingError,
    },
    product_listing_id::{ProductListingId, ProductListingKey},
    product_listing_image::ProductListingImage,
    product_listing_price::ProductListingPrice,
    source_listing_id::SourceListingId,
    title::Title,
};
use url::Url;
use user_core::user_id::UserId;

const MISSING_PRODUCT_URL: &str = "https://not-provided.invalid";
#[derive(Debug, Clone, PartialEq)]
pub struct UpsertProductListingCommand {
    pub listing_source_id: ListingSourceId,
    pub source_listing_id: SourceListingId,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub price: PatchField<ProductListingPrice>,
    pub price_estimate_min: PatchField<Price>,
    pub price_estimate_max: PatchField<Price>,
    pub availability: PatchField<ListingAvailability>,
    pub url: Option<Url>,
    pub images: PatchField<IndexSet<ProductListingImage>>,
    pub auction: PatchField<ProductListingAuctionPatch>,
}
#[derive(Debug, Clone, PartialEq)]
pub enum UpsertProductListingResult {
    Created(CreateProductListingResult),
    Updated(UpdateProductListingResult),
}
#[derive(Debug, thiserror::Error)]
pub enum UpsertProductListingError {
    #[error("authenticated actor required to upsert product listing")]
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
    #[error("product listing is withdrawn")]
    ListingWithdrawn,
    #[error("product listing is invalid")]
    InvalidProductListing {
        #[source]
        source: BoxError,
    },
    #[error("product listing title slug generation was exhausted")]
    ProductListingTitleSlugGenerationExhausted,
    #[error("product listing persistence failed")]
    PersistenceFailed,
    #[error("product listing event append failed")]
    EventAppenderFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin upsert product listing transaction")]
    BeginTransactionFailed,
    #[error("failed to commit upsert product listing transaction")]
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
pub struct UpsertProductListingHandler<U, R, E, A, V, G = RandomProductListingTitleSlugGenerator> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
    auction_references: V,
    title_slug_generator: G,
}
impl<U, R, E, A, V> UpsertProductListingHandler<U, R, E, A, V> {
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
enum AttemptError {
    SourceRace,
    SlugRace,
    Failed(UpsertProductListingError),
}
impl<U, R, E, A, V, G> UpsertProductListingHandler<U, R, E, A, V, G>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    V: AuctionReferenceValidatorFactory<U::Tx>,
    G: ProductListingTitleSlugGenerator,
{
    async fn execute_attempt(
        &self,
        context: &OperationContext,
        command: UpsertProductListingCommand,
        id: ProductListingId,
    ) -> Result<UpsertProductListingResult, AttemptError> {
        let mut tx =
            self.unit_of_work.begin().await.map_err(|_| {
                AttemptError::Failed(UpsertProductListingError::BeginTransactionFailed)
            })?;
        if let Some(actor) = partner_actor(&context.principal) {
            self.authorizer
                .in_transaction(&mut tx)
                .authorize(actor, command.listing_source_id)
                .await
                .map_err(|e| AttemptError::Failed(e.into()))?;
        }
        let key =
            ProductListingKey::new(command.listing_source_id, command.source_listing_id.clone());
        let found = {
            let mut repository = self.products.in_transaction(&mut tx);
            repository
                .find_by_key(&key)
                .await
                .map_err(|e| AttemptError::Failed(e.into()))?
        };
        let result = match found {
            Some(loaded) => {
                let mut product = loaded.value;
                product
                    .restore()
                    .map_err(|e| AttemptError::Failed(e.into()))?;
                let auction = match &command.auction {
                    PatchField::Unchanged | PatchField::Clear => None,
                    PatchField::Set(patch) => {
                        validate_product_listing_auction_patch(product.auction(), patch).map_err(
                            |e| {
                                AttemptError::Failed(
                                    UpsertProductListingError::InvalidProductListing {
                                        source: box_error(e),
                                    },
                                )
                            },
                        )?;
                        let auction =
                            compose_product_listing_auction_patch(product.auction(), patch)
                                .map_err(|e| {
                                    AttemptError::Failed(
                                        UpsertProductListingError::InvalidProductListing {
                                            source: box_error(e),
                                        },
                                    )
                                })?;
                        if let PatchField::Set(auction_id) = &patch.auction_id {
                            self.auction_references
                                .in_transaction(&mut tx)
                                .validate(*auction_id, product.listing_source_id())
                                .await
                                .map_err(|e| AttemptError::Failed(e.into()))?;
                        }
                        Some(auction)
                    }
                };
                apply_update(&mut product, &command, auction).map_err(AttemptError::Failed)?;
                let event = product.take_pending_event_payload().map(|payload| {
                    stamp_product_listing_event(
                        product.id(),
                        time::OffsetDateTime::now_utc(),
                        payload,
                    )
                });
                let outcome = if let Some(event) = event {
                    let effects = ProductListingWriteEffects::from(&event.payload);
                    self.products
                        .in_transaction(&mut tx)
                        .update(&product, loaded.version, event.event_id, effects)
                        .await
                        .map_err(|e| AttemptError::Failed(e.into()))?;
                    self.events
                        .in_transaction(&mut tx)
                        .append(&event)
                        .await
                        .map_err(|e| AttemptError::Failed(e.into()))?;
                    ChangeOutcome::Changed
                } else {
                    ChangeOutcome::Unchanged
                };
                Ok(UpsertProductListingResult::Updated(
                    UpdateProductListingResult {
                        product_listing_id: product.id(),
                        outcome,
                    },
                ))
            }
            None => {
                let title_slug_id = self
                    .title_slug_generator
                    .generate(command.title.as_ref().map_or("", |v| v.payload.as_ref()))
                    .map_err(|_| {
                        AttemptError::Failed(UpsertProductListingError::InvalidProductListing {
                            source: box_error(std::io::Error::other(
                                "invalid generated title slug",
                            )),
                        })
                    })?;
                let auction = match &command.auction {
                    PatchField::Unchanged | PatchField::Clear => None,
                    PatchField::Set(patch) => {
                        validate_product_listing_auction_patch(None, patch).map_err(|e| {
                            AttemptError::Failed(UpsertProductListingError::InvalidProductListing {
                                source: box_error(e),
                            })
                        })?;
                        let auction =
                            compose_product_listing_auction_patch(None, patch).map_err(|e| {
                                AttemptError::Failed(
                                    UpsertProductListingError::InvalidProductListing {
                                        source: box_error(e),
                                    },
                                )
                            })?;
                        if let PatchField::Set(auction_id) = &patch.auction_id {
                            self.auction_references
                                .in_transaction(&mut tx)
                                .validate(*auction_id, command.listing_source_id)
                                .await
                                .map_err(|e| AttemptError::Failed(e.into()))?;
                        }
                        auction
                    }
                };
                let url = match command.url.clone() {
                    Some(url) => url,
                    None => Url::parse(MISSING_PRODUCT_URL).map_err(|error| {
                        AttemptError::Failed(UpsertProductListingError::InvalidProductListing {
                            source: box_error(error),
                        })
                    })?,
                };
                let mut product = ProductListing::create(NewProductListing {
                    id,
                    title_slug_id,
                    listing_source_id: command.listing_source_id,
                    source_listing_id: command.source_listing_id.clone(),
                    title: command.title.clone(),
                    description: command.description.clone(),
                    pricing: ProductListingPricing {
                        price: into_option(command.price.clone()),
                        price_estimate_min: into_option(command.price_estimate_min.clone()),
                        price_estimate_max: into_option(command.price_estimate_max.clone()),
                    },
                    availability: into_option(command.availability.clone()),
                    url,
                    images: into_images(command.images.clone()),
                    auction,
                })
                .map_err(|e| AttemptError::Failed(e.into()))?;
                let event = stamp_product_listing_event(
                    product.id(),
                    time::OffsetDateTime::now_utc(),
                    product.take_pending_event_payload().ok_or_else(|| {
                        AttemptError::Failed(UpsertProductListingError::PersistenceFailed)
                    })?,
                );
                let persisted = self
                    .products
                    .in_transaction(&mut tx)
                    .insert(&product, event.event_id)
                    .await
                    .map_err(|e| match e {
                        ProductListingRepositoryError::SourceListingAlreadyExists => {
                            AttemptError::SourceRace
                        }
                        ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists => {
                            AttemptError::SlugRace
                        }
                        error => AttemptError::Failed(error.into()),
                    })?;
                self.events
                    .in_transaction(&mut tx)
                    .append(&event)
                    .await
                    .map_err(|e| AttemptError::Failed(e.into()))?;
                Ok(UpsertProductListingResult::Created(
                    CreateProductListingResult {
                        product_listing_id: persisted.value.id(),
                        product_listing_title_slug_id: persisted.value.title_slug_id().clone(),
                    },
                ))
            }
        }?;
        tx.commit().await.map_err(|_| {
            AttemptError::Failed(UpsertProductListingError::CommitTransactionFailed)
        })?;
        Ok(result)
    }
}
#[async_trait::async_trait]
impl<U, R, E, A, V, G> UpsertProductListingUseCase for UpsertProductListingHandler<U, R, E, A, V, G>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    V: AuctionReferenceValidatorFactory<U::Tx>,
    G: ProductListingTitleSlugGenerator,
{
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpsertProductListingCommand,
    ) -> Result<UpsertProductListingResult, UpsertProductListingError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductListingsWrite)
            .authorize::<UpsertProductListingError>()?;
        let id = ProductListingId::new();
        let mut source_race_retried = false;
        let mut slug_attempts = 0;
        loop {
            match self.execute_attempt(context, command.clone(), id).await {
                Ok(result) => return Ok(result),
                Err(AttemptError::SourceRace) if !source_race_retried => {
                    source_race_retried = true;
                    continue;
                }
                Err(AttemptError::SourceRace) => {
                    return Err(UpsertProductListingError::PersistenceFailed);
                }
                Err(AttemptError::SlugRace) => {
                    slug_attempts += 1;
                    if title_slug_collision_retry(slug_attempts, true)
                        == TitleSlugCollisionRetry::Retry
                    {
                        continue;
                    }
                    return Err(
                        UpsertProductListingError::ProductListingTitleSlugGenerationExhausted,
                    );
                }
                Err(AttemptError::Failed(error)) => return Err(error),
            }
        }
    }
}
fn apply_update(
    product: &mut ProductListing,
    command: &UpsertProductListingCommand,
    auction: Option<Option<product_listing_core::product_listing::ProductListingAuction>>,
) -> Result<(), UpsertProductListingError> {
    let mut pricing = product.pricing();
    apply_option(&mut pricing.price, command.price.clone());
    apply_option(
        &mut pricing.price_estimate_min,
        command.price_estimate_min.clone(),
    );
    apply_option(
        &mut pricing.price_estimate_max,
        command.price_estimate_max.clone(),
    );
    product.replace_pricing(pricing)?;
    match command.availability.clone() {
        PatchField::Unchanged => {}
        PatchField::Set(v) => {
            product.set_availability(v)?;
        }
        PatchField::Clear => {
            product.clear_availability()?;
        }
    };
    if let Some(url) = &command.url {
        product.change_url(url.clone())?;
    }
    match &command.images {
        PatchField::Unchanged => {}
        PatchField::Set(v) => {
            product.replace_images(v.clone())?;
        }
        PatchField::Clear => {
            product.replace_images(IndexSet::new())?;
        }
    };
    if let Some(auction) = auction {
        product.replace_auction(auction)?;
    }
    Ok(())
}
fn apply_option<T>(field: &mut Option<T>, patch: PatchField<T>) {
    match patch {
        PatchField::Unchanged => {}
        PatchField::Set(value) => *field = Some(value),
        PatchField::Clear => *field = None,
    }
}
fn into_option<T>(patch: PatchField<T>) -> Option<T> {
    match patch {
        PatchField::Set(v) => Some(v),
        PatchField::Unchanged | PatchField::Clear => None,
    }
}
fn into_images<T: Eq + std::hash::Hash>(patch: PatchField<IndexSet<T>>) -> IndexSet<T> {
    match patch {
        PatchField::Set(v) => v,
        PatchField::Unchanged | PatchField::Clear => IndexSet::new(),
    }
}
fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => Some(*id),
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
impl From<AuctionReferenceValidationError> for UpsertProductListingError {
    fn from(error: AuctionReferenceValidationError) -> Self {
        match error {
            AuctionReferenceValidationError::NotFound => Self::AuctionNotFound,
            AuctionReferenceValidationError::ListingSourceMismatch => Self::AuctionSourceMismatch,
            AuctionReferenceValidationError::TemporarilyUnavailable { source } => {
                Self::AuctionReferenceTemporarilyUnavailable { source }
            }
            AuctionReferenceValidationError::InvalidPersistedState { source } => {
                Self::InvalidProductListing { source }
            }
        }
    }
}
impl From<ChangeListingAvailabilityError> for UpsertProductListingError {
    fn from(_: ChangeListingAvailabilityError) -> Self {
        Self::ListingWithdrawn
    }
}
impl From<ChangeProductListingError> for UpsertProductListingError {
    fn from(error: ChangeProductListingError) -> Self {
        match error {
            ChangeProductListingError::ListingWithdrawn => Self::ListingWithdrawn,
            error => Self::InvalidProductListing {
                source: box_error(error),
            },
        }
    }
}
impl From<RehydrateProductListingError> for UpsertProductListingError {
    fn from(error: RehydrateProductListingError) -> Self {
        Self::InvalidProductListing {
            source: box_error(error),
        }
    }
}
impl From<ProductListingRepositoryError> for UpsertProductListingError {
    fn from(_: ProductListingRepositoryError) -> Self {
        Self::PersistenceFailed
    }
}
impl From<ProductListingEventAppendError> for UpsertProductListingError {
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
    use crate::product_listing_title_slug_creation::MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS;
    use application::operation_context::{CorrelationId, RequestId};
    use application::transaction::TransactionError;
    use auction_core::AuctionId;
    use domain_primitives::{event_id::EventId, versioned::Versioned};
    use listing_source_core::ListingSourceId;
    use product_listing_core::{
        product_listing::{NewProductListing, ProductListingPricing},
        product_listing_slug_id::{InvalidProductListingSlugId, ProductListingSlugId},
        source_listing_id::SourceListingId,
    };
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Default)]
    struct State {
        begins: usize,
        commits: usize,
        rollbacks: usize,
        generated_candidates: usize,
        inserts: usize,
        updates: usize,
        event_appends: usize,
        authorizations: usize,
        auction_validations: usize,
        finds: VecDeque<Option<VersionedProductListing>>,
        insert_results: VecDeque<Result<(), ProductListingRepositoryError>>,
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

    fn command() -> UpsertProductListingCommand {
        UpsertProductListingCommand {
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("source-listing")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            title: None,
            description: None,
            price: PatchField::Unchanged,
            price_estimate_min: PatchField::Unchanged,
            price_estimate_max: PatchField::Unchanged,
            availability: PatchField::Unchanged,
            url: None,
            images: PatchField::Unchanged,
            auction: PatchField::Unchanged,
        }
    }

    fn command_with_auction() -> UpsertProductListingCommand {
        UpsertProductListingCommand {
            auction: PatchField::Set(ProductListingAuctionPatch {
                auction_id: PatchField::Set(AuctionId::new()),
                ..Default::default()
            }),
            ..command()
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

    fn handler(
        state: &SharedState,
    ) -> UpsertProductListingHandler<
        UnitOfWorkFake,
        ProductsFake,
        EventsFake,
        AuthorizerFake,
        AuctionValidatorFake,
        GeneratorFake,
    > {
        UpsertProductListingHandler {
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
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([None]),
            ..Default::default()
        }));

        let result = handler(&state).execute(&context(), command()).await;

        assert!(matches!(result, Ok(UpsertProductListingResult::Created(_))));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 1, 0));
        assert_eq!(
            (
                state.generated_candidates,
                state.inserts,
                state.updates,
                state.event_appends,
                state.authorizations
            ),
            (1, 1, 0, 1, 1)
        );
    }

    #[tokio::test]
    async fn should_exhaust_after_configured_title_slug_collisions() {
        let state = Arc::new(Mutex::new(State {
            finds: (0..MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS)
                .map(|_| None)
                .collect(),
            insert_results: (0..MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS)
                .map(|_| Err(ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists))
                .collect(),
            ..Default::default()
        }));

        let result = handler(&state).execute(&context(), command()).await;

        assert!(matches!(
            result,
            Err(UpsertProductListingError::ProductListingTitleSlugGenerationExhausted)
        ));
        let state = lock(&state);
        assert_eq!(
            (
                state.generated_candidates,
                state.begins,
                state.commits,
                state.rollbacks,
                state.inserts,
                state.event_appends
            ),
            (
                MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS,
                MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS,
                0,
                MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS,
                MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS,
                0
            )
        );
    }

    #[tokio::test]
    async fn should_retry_source_race_once_and_revalidate_authorization_and_auction() {
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([None, Some(loaded_listing())]),
            insert_results: VecDeque::from([Err(
                ProductListingRepositoryError::SourceListingAlreadyExists,
            )]),
            ..Default::default()
        }));

        let result = handler(&state)
            .execute(&context(), command_with_auction())
            .await;

        assert!(matches!(
            result,
            Ok(UpsertProductListingResult::Updated(
                UpdateProductListingResult {
                    outcome: ChangeOutcome::Changed,
                    ..
                }
            ))
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (2, 1, 1));
        assert_eq!(
            (
                state.generated_candidates,
                state.inserts,
                state.updates,
                state.event_appends,
                state.authorizations,
                state.auction_validations
            ),
            (1, 1, 1, 1, 2, 2)
        );
    }

    #[tokio::test]
    async fn should_classify_exhausted_source_race_as_persistence_failure() {
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([None, None]),
            insert_results: VecDeque::from([
                Err(ProductListingRepositoryError::SourceListingAlreadyExists),
                Err(ProductListingRepositoryError::SourceListingAlreadyExists),
            ]),
            ..Default::default()
        }));

        let result = handler(&state).execute(&context(), command()).await;

        assert!(matches!(
            result,
            Err(UpsertProductListingError::PersistenceFailed)
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (2, 0, 2));
        assert_eq!(
            (
                state.generated_candidates,
                state.inserts,
                state.event_appends
            ),
            (2, 2, 0)
        );
    }

    #[tokio::test]
    async fn should_keep_source_race_and_slug_collision_accounting_independent() {
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([None, None, None]),
            insert_results: VecDeque::from([
                Err(ProductListingRepositoryError::SourceListingAlreadyExists),
                Err(ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists),
                Ok(()),
            ]),
            ..Default::default()
        }));

        let result = handler(&state).execute(&context(), command()).await;

        assert!(matches!(result, Ok(UpsertProductListingResult::Created(_))));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (3, 1, 2));
        assert_eq!(
            (
                state.generated_candidates,
                state.inserts,
                state.event_appends,
                state.authorizations
            ),
            (3, 3, 1, 3)
        );
    }

    #[tokio::test]
    async fn should_not_retry_unrelated_persistence_failure() {
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([None]),
            insert_results: VecDeque::from([Err(
                ProductListingRepositoryError::ProductListingInsertFailed,
            )]),
            ..Default::default()
        }));

        let result = handler(&state).execute(&context(), command()).await;

        assert!(matches!(
            result,
            Err(UpsertProductListingError::PersistenceFailed)
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 0, 1));
        assert_eq!((state.generated_candidates, state.inserts), (1, 1));
    }

    #[tokio::test]
    async fn should_not_commit_when_event_append_fails() {
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([None]),
            event_results: VecDeque::from([Err(event_append_failure())]),
            ..Default::default()
        }));

        let result = handler(&state).execute(&context(), command()).await;

        assert!(matches!(
            result,
            Err(UpsertProductListingError::EventAppenderFailed { .. })
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 0, 1));
        assert_eq!((state.inserts, state.event_appends), (1, 1));
    }

    #[tokio::test]
    async fn should_commit_without_persistence_for_noop_update() {
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([Some(loaded_listing())]),
            ..Default::default()
        }));

        let result = handler(&state).execute(&context(), command()).await;

        assert!(matches!(
            result,
            Ok(UpsertProductListingResult::Updated(
                UpdateProductListingResult {
                    outcome: ChangeOutcome::Unchanged,
                    ..
                }
            ))
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 1, 0));
        assert_eq!((state.updates, state.event_appends), (0, 0));
    }

    #[tokio::test]
    async fn should_not_revalidate_unchanged_auction_for_lot_only_update() {
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([Some(loaded_listing_with_auction())]),
            ..Default::default()
        }));
        let command = UpsertProductListingCommand {
            auction: PatchField::Set(ProductListingAuctionPatch {
                bidding_opens: PatchField::Set(time::OffsetDateTime::UNIX_EPOCH),
                ..Default::default()
            }),
            ..command()
        };

        let result = handler(&state).execute(&context(), command).await;

        assert!(matches!(
            result,
            Ok(UpsertProductListingResult::Updated(
                UpdateProductListingResult {
                    outcome: ChangeOutcome::Changed,
                    ..
                }
            ))
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
    async fn should_restore_withdrawn_listing_and_commit_one_event() {
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([Some(withdrawn_listing())]),
            ..Default::default()
        }));

        let result = handler(&state).execute(&context(), command()).await;

        assert!(matches!(
            result,
            Ok(UpsertProductListingResult::Updated(
                UpdateProductListingResult {
                    outcome: ChangeOutcome::Changed,
                    ..
                }
            ))
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (1, 1, 0));
        assert_eq!((state.updates, state.event_appends), (1, 1));
        assert_eq!(
            state.last_updated_lifecycle,
            Some(product_listing_core::listing_lifecycle::ListingLifecycle::Active)
        );
    }

    #[tokio::test]
    async fn should_validate_auction_reference_before_new_listing_insert() {
        let state = Arc::new(Mutex::new(State {
            finds: VecDeque::from([None]),
            auction_results: VecDeque::from([Err(AuctionReferenceValidationError::NotFound)]),
            ..Default::default()
        }));

        let result = handler(&state)
            .execute(&context(), command_with_auction())
            .await;

        assert!(matches!(
            result,
            Err(UpsertProductListingError::AuctionNotFound)
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
    async fn should_reject_anonymous_upsert_before_beginning_transaction() {
        let state = Arc::new(Mutex::new(State::default()));
        let mut context = context();
        context.principal = Principal::Anonymous;

        let result = handler(&state).execute(&context, command()).await;

        assert!(matches!(
            result,
            Err(UpsertProductListingError::AuthenticatedActorRequired)
        ));
        let state = lock(&state);
        assert_eq!((state.begins, state.commits, state.rollbacks), (0, 0, 0));
        assert_eq!((state.generated_candidates, state.inserts), (0, 0));
    }
}
