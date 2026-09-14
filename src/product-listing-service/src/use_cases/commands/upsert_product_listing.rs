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
                        if let Some(auction_id) = auction
                            .as_ref()
                            .and_then(product_listing_core::product_listing::ProductListingAuction::auction_id)
                        {
                            self.auction_references
                                .in_transaction(&mut tx)
                                .validate(auction_id, product.listing_source_id())
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
                        if let Some(auction_id) = auction
                            .as_ref()
                            .and_then(product_listing_core::product_listing::ProductListingAuction::auction_id)
                        {
                            self.auction_references
                                .in_transaction(&mut tx)
                                .validate(auction_id, command.listing_source_id)
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
        let mut races = 0;
        let mut slug_attempts = 0;
        loop {
            match self.execute_attempt(context, command.clone(), id).await {
                Ok(result) => return Ok(result),
                Err(AttemptError::SourceRace) if races == 0 => races += 1,
                Err(AttemptError::SlugRace)
                    if title_slug_collision_retry(slug_attempts, true)
                        == TitleSlugCollisionRetry::Retry =>
                {
                    slug_attempts += 1
                }
                Err(AttemptError::SourceRace | AttemptError::SlugRace) => {
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
