use crate::ports::{
    PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
    PartnerProductListingAuthorizerFactory, ProductListingEventAppendError,
    ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
    ProductListingRepositoryError, ProductListingRepositoryFactory, ProductListingWriteEffects,
    stamp_product_listing_event,
};
use crate::product_listing_title_slug_creation::{
    ProductListingTitleSlugGenerator, RandomProductListingTitleSlugGenerator,
    TitleSlugCollisionRetry, title_slug_collision_retry,
};
use crate::use_cases::{
    CreateProductListingResult, UpdateProductListingResult, UpsertProductListingResult,
    WithdrawProductListingResult,
};
use application::error::{BoxError, box_error};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use application::patch_field::PatchField;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::change_outcome::ChangeOutcome;
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use listing_source_service::ports::{
    ListingSourceReadError, WoocommerceSignatureVerification, WoocommerceSignatureVerifier,
    WoocommerceSource, WoocommerceSourceReader,
};
use localization::Localized;
use money::{MonetaryAmount, Price};
use product_listing_core::{
    description::Description,
    listing_availability::ListingAvailability,
    product_listing::{
        ChangeListingAvailabilityError, ChangeProductListingError, NewProductListing,
        ProductListing, ProductListingAuction, ProductListingPricing, RehydrateProductListingError,
    },
    product_listing_id::{ProductListingId, ProductListingKey},
    product_listing_image::ProductListingImage,
    source_listing_id::SourceListingId,
    title::Title,
};
use url::Url;
use user_core::user_id::UserId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WoocommerceProductEventKind {
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IngestWoocommerceProductListingCommand {
    pub listing_source_id: ListingSourceId,
    pub kind: WoocommerceProductEventKind,
    pub signature: Vec<u8>,
    pub raw_body: Vec<u8>,
    pub source_listing_id: SourceListingId,
    pub title: Option<String>,
    pub permalink: Option<Url>,
    pub description_html: Option<String>,
    pub short_description_html: Option<String>,
    pub price: Option<String>,
    pub status: Option<String>,
    pub stock_status: Option<String>,
    pub image_urls: IndexSet<Url>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IngestWoocommerceProductListingResult {
    Ignored,
    Upserted(UpsertProductListingResult),
    Withdrawn(WithdrawProductListingResult),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WoocommerceListingAction {
    Upsert(PatchField<ListingAvailability>),
    Withdraw,
    Ignore,
}

#[derive(Debug)]
struct WoocommerceListingData {
    source_listing_id: SourceListingId,
    title: Localized<localization::Language, Title>,
    description: Option<Localized<localization::Language, Description>>,
    price: PatchField<Price>,
    availability: PatchField<ListingAvailability>,
    url: Url,
    images: IndexSet<ProductListingImage>,
}

#[derive(Debug, thiserror::Error)]
pub enum IngestWoocommerceProductListingError {
    #[error("WooCommerce product title is missing")]
    MissingTitle,
    #[error("WooCommerce product URL is missing")]
    MissingUrl,
    #[error("WooCommerce product price is invalid")]
    InvalidPrice,
    #[error("listing source has no WooCommerce currency configured")]
    MissingListingSourceCurrency,
    #[error("listing source has no WooCommerce language configured")]
    MissingListingSourceLanguage,
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("actor may not ingest WooCommerce webhooks for this listing source")]
    ActorMayNotIngestForListingSource,
    #[error("listing source not found")]
    ListingSourceNotFound,
    #[error("WooCommerce webhook secret is not configured")]
    WebhookSecretNotConfigured,
    #[error("WooCommerce webhook signature is invalid")]
    InvalidSignature,
    #[error("partner ProductListing authorization is temporarily unavailable")]
    PartnerAuthorizationTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("partner ProductListing authorization failed internally")]
    PartnerAuthorizationInternal {
        #[source]
        source: BoxError,
    },
    #[error("temporary WooCommerce listing source read failure")]
    ListingSourceTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid WooCommerce listing source read model")]
    InvalidListingSourceReadModel {
        #[source]
        source: BoxError,
    },
    #[error("product listing is withdrawn")]
    ListingWithdrawn,
    #[error("WooCommerce product listing is invalid")]
    InvalidProductListing {
        #[source]
        source: BoxError,
    },
    #[error("product listing title slug generation was exhausted")]
    ProductListingTitleSlugGenerationExhausted,
    #[error("product listing persistence failed")]
    ProductListingPersistenceFailed,
    #[error("product listing event append failed")]
    ProductListingEventAppenderFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin WooCommerce product ingestion transaction")]
    BeginTransactionFailed,
    #[error("failed to commit WooCommerce product ingestion transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait IngestWoocommerceProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: IngestWoocommerceProductListingCommand,
    ) -> Result<IngestWoocommerceProductListingResult, IngestWoocommerceProductListingError>;
}

pub struct IngestWoocommerceProductListingHandler<
    U,
    R,
    E,
    A,
    S,
    V,
    G = RandomProductListingTitleSlugGenerator,
> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
    sources: S,
    signature_verifier: V,
    title_slug_generator: G,
}

enum WoocommerceIngestAttemptError {
    SourceListingInsertRace,
    TitleSlugCollision,
    Failed(IngestWoocommerceProductListingError),
}

impl From<IngestWoocommerceProductListingError> for WoocommerceIngestAttemptError {
    fn from(error: IngestWoocommerceProductListingError) -> Self {
        Self::Failed(error)
    }
}
impl From<ProductListingRepositoryError> for WoocommerceIngestAttemptError {
    fn from(error: ProductListingRepositoryError) -> Self {
        Self::Failed(error.into())
    }
}
impl From<ProductListingEventAppendError> for WoocommerceIngestAttemptError {
    fn from(error: ProductListingEventAppendError) -> Self {
        Self::Failed(error.into())
    }
}
impl From<PartnerProductListingAuthorizationError> for WoocommerceIngestAttemptError {
    fn from(error: PartnerProductListingAuthorizationError) -> Self {
        Self::Failed(error.into())
    }
}
impl From<ChangeListingAvailabilityError> for WoocommerceIngestAttemptError {
    fn from(error: ChangeListingAvailabilityError) -> Self {
        Self::Failed(error.into())
    }
}
impl From<ChangeProductListingError> for WoocommerceIngestAttemptError {
    fn from(error: ChangeProductListingError) -> Self {
        Self::Failed(error.into())
    }
}
impl From<RehydrateProductListingError> for WoocommerceIngestAttemptError {
    fn from(error: RehydrateProductListingError) -> Self {
        Self::Failed(error.into())
    }
}

impl<U, R, E, A, S, V>
    IngestWoocommerceProductListingHandler<U, R, E, A, S, V, RandomProductListingTitleSlugGenerator>
{
    pub fn new(
        unit_of_work: U,
        products: R,
        events: E,
        authorizer: A,
        sources: S,
        signature_verifier: V,
    ) -> Self {
        Self::with_title_slug_generator(
            unit_of_work,
            products,
            events,
            authorizer,
            sources,
            signature_verifier,
            RandomProductListingTitleSlugGenerator,
        )
    }
}

impl<U, R, E, A, S, V, G> IngestWoocommerceProductListingHandler<U, R, E, A, S, V, G> {
    pub(crate) fn with_title_slug_generator(
        unit_of_work: U,
        products: R,
        events: E,
        authorizer: A,
        sources: S,
        signature_verifier: V,
        title_slug_generator: G,
    ) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            authorizer,
            sources,
            signature_verifier,
            title_slug_generator,
        }
    }
}

impl<U, R, E, A, S, V, G> IngestWoocommerceProductListingHandler<U, R, E, A, S, V, G>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    S: WoocommerceSourceReader,
    V: WoocommerceSignatureVerifier,
    G: ProductListingTitleSlugGenerator,
{
    async fn validate_webhook(
        &self,
        listing_source_id: ListingSourceId,
        body: &[u8],
        signature: &[u8],
    ) -> Result<WoocommerceSource, IngestWoocommerceProductListingError> {
        let source = self
            .sources
            .find_by_id(listing_source_id)
            .await?
            .ok_or(IngestWoocommerceProductListingError::ListingSourceNotFound)?;
        match self
            .signature_verifier
            .verify(listing_source_id, body, signature)
            .await?
        {
            WoocommerceSignatureVerification::Valid => Ok(source),
            WoocommerceSignatureVerification::Invalid => {
                Err(IngestWoocommerceProductListingError::InvalidSignature)
            }
            WoocommerceSignatureVerification::SecretNotConfigured => {
                Err(IngestWoocommerceProductListingError::WebhookSecretNotConfigured)
            }
        }
    }

    async fn upsert(
        &self,
        tx: &mut U::Tx,
        source: &WoocommerceSource,
        data: WoocommerceListingData,
        new_product_listing_id: ProductListingId,
    ) -> Result<UpsertProductListingResult, WoocommerceIngestAttemptError> {
        let key = ProductListingKey::new(source.listing_source_id, data.source_listing_id.clone());
        let existing = self.products.in_transaction(tx).find_by_key(&key).await?;
        match existing {
            Some(loaded) => {
                let expected_version = loaded.version;
                let mut listing = loaded.value;
                listing.restore()?;
                match data.price {
                    PatchField::Unchanged => {}
                    PatchField::Set(price) => {
                        listing.set_price(price)?;
                    }
                    PatchField::Clear => {
                        listing.clear_price()?;
                    }
                }
                match data.availability {
                    PatchField::Unchanged => {}
                    PatchField::Set(availability) => {
                        listing.set_availability(availability)?;
                    }
                    PatchField::Clear => {
                        listing.clear_availability()?;
                    }
                }
                listing.change_url(data.url)?;
                listing.replace_images(data.images)?;
                let event = listing.take_pending_event_payload().map(|payload| {
                    stamp_product_listing_event(
                        listing.id(),
                        time::OffsetDateTime::now_utc(),
                        payload,
                    )
                });
                let outcome = if event.is_some() {
                    ChangeOutcome::Changed
                } else {
                    ChangeOutcome::Unchanged
                };
                if let Some(event) = event {
                    let effects = ProductListingWriteEffects::from(&event.payload);
                    listing = self
                        .products
                        .in_transaction(tx)
                        .update(&listing, expected_version, event.event_id, effects)
                        .await?
                        .value;
                    self.events.in_transaction(tx).append(&event).await?;
                }
                Ok(UpsertProductListingResult::Updated(
                    UpdateProductListingResult {
                        product_listing_id: listing.id(),
                        outcome,
                    },
                ))
            }
            None => {
                let title_slug_id = self
                    .title_slug_generator
                    .generate(data.title.payload.as_ref())
                    .map_err(
                        |_| IngestWoocommerceProductListingError::InvalidProductListing {
                            source: box_error(std::io::Error::other(
                                "invalid generated title slug",
                            )),
                        },
                    )?;
                let mut listing = ProductListing::create(NewProductListing {
                    id: new_product_listing_id,
                    title_slug_id,
                    listing_source_id: source.listing_source_id,
                    source_listing_id: data.source_listing_id,
                    title: Some(data.title),
                    description: data.description,
                    pricing: ProductListingPricing {
                        price: match data.price {
                            PatchField::Set(price) => Some(price),
                            PatchField::Unchanged | PatchField::Clear => None,
                        },
                        price_estimate_min: None,
                        price_estimate_max: None,
                    },
                    availability: match data.availability {
                        PatchField::Set(availability) => Some(availability),
                        PatchField::Unchanged | PatchField::Clear => None,
                    },
                    url: data.url,
                    images: data.images,
                    auction: ProductListingAuction::default(),
                })?;
                let event = stamp_product_listing_event(
                    listing.id(),
                    time::OffsetDateTime::now_utc(),
                    listing.take_pending_event_payload().ok_or_else(|| {
                        IngestWoocommerceProductListingError::InvalidProductListing {
                            source: box_error(std::io::Error::other(
                                "created listing has no event",
                            )),
                        }
                    })?,
                );
                let event_id = event.event_id;
                let persisted = self
                    .products
                    .in_transaction(tx)
                    .insert(&listing, event_id)
                    .await
                    .map_err(|error| match error {
                        ProductListingRepositoryError::SourceListingAlreadyExists => {
                            WoocommerceIngestAttemptError::SourceListingInsertRace
                        }
                        ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists => {
                            WoocommerceIngestAttemptError::TitleSlugCollision
                        }
                        error => WoocommerceIngestAttemptError::Failed(error.into()),
                    })?;
                self.events.in_transaction(tx).append(&event).await?;
                Ok(UpsertProductListingResult::Created(
                    CreateProductListingResult {
                        product_listing_id: persisted.value.id(),
                        product_listing_title_slug_id: persisted.value.title_slug_id().clone(),
                    },
                ))
            }
        }
    }

    async fn withdraw(
        &self,
        tx: &mut U::Tx,
        key: ProductListingKey,
    ) -> Result<Option<WithdrawProductListingResult>, IngestWoocommerceProductListingError> {
        let Some(loaded) = self.products.in_transaction(tx).find_by_key(&key).await? else {
            return Ok(None);
        };
        let expected_version = loaded.version;
        let mut listing = loaded.value;
        let outcome = listing.withdraw()?;
        let event = listing.take_pending_event_payload().map(|payload| {
            stamp_product_listing_event(listing.id(), time::OffsetDateTime::now_utc(), payload)
        });
        if let Some(event) = event {
            let effects = ProductListingWriteEffects::from(&event.payload);
            listing = self
                .products
                .in_transaction(tx)
                .update(&listing, expected_version, event.event_id, effects)
                .await?
                .value;
            self.events.in_transaction(tx).append(&event).await?;
        }
        Ok(Some(WithdrawProductListingResult {
            product_listing_id: listing.id(),
            outcome,
        }))
    }

    async fn execute_attempt(
        &self,
        context: &OperationContext,
        command: IngestWoocommerceProductListingCommand,
        new_product_listing_id: ProductListingId,
    ) -> Result<IngestWoocommerceProductListingResult, WoocommerceIngestAttemptError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductListingsWrite)
            .authorize::<IngestWoocommerceProductListingError>()?;
        let source = self
            .validate_webhook(
                command.listing_source_id,
                &command.raw_body,
                &command.signature,
            )
            .await?;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| IngestWoocommerceProductListingError::BeginTransactionFailed)?;
        if let Some(actor_id) = partner_actor(&context.principal) {
            self.authorizer
                .in_transaction(&mut tx)
                .authorize(actor_id, source.listing_source_id)
                .await?;
        }
        let result = match listing_action(
            command.kind,
            command.status.as_deref(),
            command.stock_status.as_deref(),
        ) {
            WoocommerceListingAction::Ignore => IngestWoocommerceProductListingResult::Ignored,
            WoocommerceListingAction::Withdraw => self
                .withdraw(
                    &mut tx,
                    ProductListingKey::new(source.listing_source_id, command.source_listing_id),
                )
                .await?
                .map(IngestWoocommerceProductListingResult::Withdrawn)
                .unwrap_or(IngestWoocommerceProductListingResult::Ignored),
            WoocommerceListingAction::Upsert(availability) => {
                IngestWoocommerceProductListingResult::Upserted(
                    self.upsert(
                        &mut tx,
                        &source,
                        listing_data(command, availability, &source)?,
                        new_product_listing_id,
                    )
                    .await?,
                )
            }
        };
        tx.commit()
            .await
            .map_err(|_| IngestWoocommerceProductListingError::CommitTransactionFailed)?;
        Ok(result)
    }
}

#[async_trait::async_trait]
impl<U, R, E, A, S, V, G> IngestWoocommerceProductListingUseCase
    for IngestWoocommerceProductListingHandler<U, R, E, A, S, V, G>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    S: WoocommerceSourceReader,
    V: WoocommerceSignatureVerifier,
    G: ProductListingTitleSlugGenerator,
{
    #[tracing::instrument(name = "ingest_woocommerce_product_listing", skip_all, fields(listing_source_id = %command.listing_source_id, source_listing_id = %command.source_listing_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: IngestWoocommerceProductListingCommand,
    ) -> Result<IngestWoocommerceProductListingResult, IngestWoocommerceProductListingError> {
        let new_product_listing_id = ProductListingId::new();
        let mut source_listing_races = 0;
        let mut title_slug_attempts = 0;
        loop {
            match self
                .execute_attempt(context, command.clone(), new_product_listing_id)
                .await
            {
                Ok(result) => return Ok(result),
                Err(WoocommerceIngestAttemptError::SourceListingInsertRace) => {
                    source_listing_races += 1;
                    if source_listing_races > 1 {
                        return Err(
                            IngestWoocommerceProductListingError::ProductListingPersistenceFailed,
                        );
                    }
                }
                Err(WoocommerceIngestAttemptError::TitleSlugCollision) => {
                    title_slug_attempts += 1;
                    if title_slug_collision_retry(title_slug_attempts, true)
                        == TitleSlugCollisionRetry::Exhausted
                    {
                        return Err(
                            IngestWoocommerceProductListingError::ProductListingTitleSlugGenerationExhausted,
                        );
                    }
                }
                Err(WoocommerceIngestAttemptError::Failed(error)) => return Err(error),
            }
        }
    }
}

fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}

fn listing_data(
    command: IngestWoocommerceProductListingCommand,
    availability: PatchField<ListingAvailability>,
    source: &WoocommerceSource,
) -> Result<WoocommerceListingData, IngestWoocommerceProductListingError> {
    let language = source
        .language
        .ok_or(IngestWoocommerceProductListingError::MissingListingSourceLanguage)?;
    let title = command
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or(IngestWoocommerceProductListingError::MissingTitle)?;
    let url = command
        .permalink
        .ok_or(IngestWoocommerceProductListingError::MissingUrl)?;
    let description = command
        .description_html
        .as_deref()
        .or(command.short_description_html.as_deref())
        .map(fallbacked_html_to_markdown)
        .filter(|value| !value.is_empty())
        .map(Description::from)
        .map(|value| Localized::new(language, value));
    let images = command
        .image_urls
        .into_iter()
        .map(ProductListingImage::new)
        .collect();
    Ok(WoocommerceListingData {
        source_listing_id: command.source_listing_id,
        title: Localized::new(language, Title::from(title)),
        description,
        price: match parse_price(command.price.as_deref(), source.currency)? {
            Some(price) => PatchField::Set(price),
            None => PatchField::Clear,
        },
        availability,
        url,
        images,
    })
}

fn parse_price(
    value: Option<&str>,
    currency: Option<money::Currency>,
) -> Result<Option<Price>, IngestWoocommerceProductListingError> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let currency =
        currency.ok_or(IngestWoocommerceProductListingError::MissingListingSourceCurrency)?;
    let (major, minor) = value.trim().split_once('.').unwrap_or((value.trim(), ""));
    if !major.chars().all(|value| value.is_ascii_digit())
        || !minor.chars().all(|value| value.is_ascii_digit())
    {
        return Err(IngestWoocommerceProductListingError::InvalidPrice);
    }
    let major = major
        .parse::<u64>()
        .map_err(|_| IngestWoocommerceProductListingError::InvalidPrice)?;
    let mut minor = minor.chars().take(2).collect::<String>();
    while minor.len() < 2 {
        minor.push('0');
    }
    let minor = minor
        .parse::<u64>()
        .map_err(|_| IngestWoocommerceProductListingError::InvalidPrice)?;
    Ok(Some(Price::new(
        MonetaryAmount::from(major * 100 + minor),
        currency,
    )))
}

fn fallbacked_html_to_markdown(html: &str) -> String {
    match html_to_markdown_rs::convert(html, None) {
        Ok(result) => result.content.unwrap_or_else(|| html.to_owned()),
        Err(_) => html.to_owned(),
    }
}

fn listing_action(
    kind: WoocommerceProductEventKind,
    status: Option<&str>,
    stock_status: Option<&str>,
) -> WoocommerceListingAction {
    if kind == WoocommerceProductEventKind::Delete {
        return WoocommerceListingAction::Withdraw;
    }

    match status {
        Some("trash" | "draft" | "pending" | "private") => WoocommerceListingAction::Withdraw,
        Some("publish") => WoocommerceListingAction::Upsert(match stock_status {
            Some("instock") => PatchField::Set(ListingAvailability::InStock),
            Some("outofstock") => PatchField::Set(ListingAvailability::OutOfStock),
            Some("onbackorder") => PatchField::Set(ListingAvailability::BackOrder),
            Some(_) | None => PatchField::Unchanged,
        }),
        Some(_) | None => WoocommerceListingAction::Ignore,
    }
}

impl From<OperationAuthorizationError> for IngestWoocommerceProductListingError {
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

impl From<PartnerProductListingAuthorizationError> for IngestWoocommerceProductListingError {
    fn from(error: PartnerProductListingAuthorizationError) -> Self {
        match error {
            PartnerProductListingAuthorizationError::ListingSourceNotFound => {
                Self::ListingSourceNotFound
            }
            PartnerProductListingAuthorizationError::Forbidden => {
                Self::ActorMayNotIngestForListingSource
            }
            PartnerProductListingAuthorizationError::TemporarilyUnavailable { source } => {
                Self::PartnerAuthorizationTemporarilyUnavailable { source }
            }
            PartnerProductListingAuthorizationError::Internal { source } => {
                Self::PartnerAuthorizationInternal { source }
            }
        }
    }
}

impl From<ListingSourceReadError> for IngestWoocommerceProductListingError {
    fn from(error: ListingSourceReadError) -> Self {
        match error {
            ListingSourceReadError::TemporarilyUnavailable { source } => {
                Self::ListingSourceTemporarilyUnavailable { source }
            }
            ListingSourceReadError::InvalidReadModel { source } => {
                Self::InvalidListingSourceReadModel { source }
            }
        }
    }
}

impl From<RehydrateProductListingError> for IngestWoocommerceProductListingError {
    fn from(error: RehydrateProductListingError) -> Self {
        Self::InvalidProductListing {
            source: box_error(error),
        }
    }
}
impl From<ChangeListingAvailabilityError> for IngestWoocommerceProductListingError {
    fn from(error: ChangeListingAvailabilityError) -> Self {
        match error {
            ChangeListingAvailabilityError::ListingWithdrawn => Self::ListingWithdrawn,
            error => Self::InvalidProductListing {
                source: box_error(error),
            },
        }
    }
}
impl From<ChangeProductListingError> for IngestWoocommerceProductListingError {
    fn from(error: ChangeProductListingError) -> Self {
        match error {
            ChangeProductListingError::ListingWithdrawn => Self::ListingWithdrawn,
            error => Self::InvalidProductListing {
                source: box_error(error),
            },
        }
    }
}
impl From<ProductListingRepositoryError> for IngestWoocommerceProductListingError {
    fn from(_: ProductListingRepositoryError) -> Self {
        Self::ProductListingPersistenceFailed
    }
}
impl From<ProductListingEventAppendError> for IngestWoocommerceProductListingError {
    fn from(error: ProductListingEventAppendError) -> Self {
        Self::ProductListingEventAppenderFailed {
            source: box_error(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{ProductListingStorageVersion, VersionedProductListing};
    use application::operation_context::{CorrelationId, RequestId};
    use application::transaction::TransactionError;
    use domain_primitives::{event_id::EventId, versioned::Versioned};
    use product_listing_core::product_listing_slug_id::ProductListingSlugId;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[test]
    fn should_preserve_existing_availability_for_unrecognized_stock_status() {
        assert_eq!(
            WoocommerceListingAction::Upsert(PatchField::Unchanged),
            listing_action(
                WoocommerceProductEventKind::Update,
                Some("publish"),
                Some("unknown"),
            )
        );
    }

    #[test]
    fn should_withdraw_deleted_product() {
        assert_eq!(
            WoocommerceListingAction::Withdraw,
            listing_action(WoocommerceProductEventKind::Delete, None, None)
        );
    }

    #[test]
    fn should_require_currency_only_when_price_exists() {
        assert!(matches!(parse_price(None, None), Ok(None)));
        assert!(matches!(
            parse_price(Some("42.00"), None),
            Err(IngestWoocommerceProductListingError::MissingListingSourceCurrency)
        ));
    }

    struct State {
        candidates: Vec<ProductListingSlugId>,
        begins: usize,
        commits: usize,
        rollbacks: usize,
        finds: VecDeque<Option<VersionedProductListing>>,
        inserts: VecDeque<Result<(), ProductListingRepositoryError>>,
        updates: usize,
        events: usize,
        event_results: VecDeque<Result<(), ProductListingEventAppendError>>,
        authorizations: usize,
        source: Option<WoocommerceSource>,
        signature: WoocommerceSignatureVerification,
        source_reads: usize,
        signature_checks: usize,
        commit_results: VecDeque<Result<(), TransactionError>>,
    }

    impl Default for State {
        fn default() -> Self {
            Self {
                candidates: Vec::new(),
                begins: 0,
                commits: 0,
                rollbacks: 0,
                finds: VecDeque::new(),
                inserts: VecDeque::new(),
                updates: 0,
                events: 0,
                event_results: VecDeque::new(),
                authorizations: 0,
                source: None,
                signature: WoocommerceSignatureVerification::Valid,
                source_reads: 0,
                signature_checks: 0,
                commit_results: VecDeque::new(),
            }
        }
    }

    type SharedState = Arc<Mutex<State>>;

    #[derive(Clone)]
    struct UowFake(SharedState);
    struct TxFake(SharedState, bool);
    #[derive(Clone)]
    struct ProductsFake(SharedState);
    struct RepositoryFake(SharedState);
    #[derive(Clone)]
    struct EventsFake(SharedState);
    struct EventAppenderFake(SharedState);
    #[derive(Clone)]
    struct AuthorizerFake(SharedState);
    struct AuthorizationFake(SharedState);
    #[derive(Clone)]
    struct SourcesFake(SharedState);
    #[derive(Clone)]
    struct SignatureVerifierFake(SharedState);
    #[derive(Clone)]
    struct GeneratorFake(SharedState);

    impl Drop for TxFake {
        fn drop(&mut self) {
            if !self.1 {
                lock(&self.0).rollbacks += 1;
            }
        }
    }

    fn lock(state: &SharedState) -> MutexGuard<'_, State> {
        match state.lock() {
            Ok(state) => state,
            Err(error) => error.into_inner(),
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for UowFake {
        type Tx = TxFake;

        async fn begin(&self) -> Result<TxFake, TransactionError> {
            lock(&self.0).begins += 1;
            Ok(TxFake(Arc::clone(&self.0), false))
        }
    }

    #[async_trait::async_trait]
    impl Transaction for TxFake {
        async fn commit(mut self) -> Result<(), TransactionError> {
            let result = lock(&self.0).commit_results.pop_front().unwrap_or(Ok(()));
            if result.is_ok() {
                self.1 = true;
                lock(&self.0).commits += 1;
            }
            result
        }
    }

    impl ProductListingRepositoryFactory<TxFake> for ProductsFake {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TxFake,
        ) -> impl ProductListingRepository + 'tx {
            RepositoryFake(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl ProductListingRepository for RepositoryFake {
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
            Ok(lock(&self.0).finds.pop_front().flatten())
        }

        async fn find_by_listing_source_and_url(
            &mut self,
            _: listing_source_core::ListingSourceId,
            _: &url::Url,
        ) -> Result<Vec<VersionedProductListing>, ProductListingRepositoryError> {
            Ok(vec![])
        }

        async fn insert(
            &mut self,
            listing: &ProductListing,
            _: EventId,
        ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
            match lock(&self.0).inserts.pop_front().unwrap_or(Ok(())) {
                Ok(()) => Ok(Versioned::new(
                    listing.clone(),
                    ProductListingStorageVersion::INITIAL,
                )),
                Err(error) => Err(error),
            }
        }

        async fn update(
            &mut self,
            listing: &ProductListing,
            expected_version: ProductListingStorageVersion,
            _: EventId,
            _: ProductListingWriteEffects,
        ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
            lock(&self.0).updates += 1;
            Ok(Versioned::new(listing.clone(), expected_version.next()))
        }
    }

    impl ProductListingEventAppenderFactory<TxFake> for EventsFake {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TxFake,
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
            state.events += 1;
            state.event_results.pop_front().unwrap_or(Ok(()))
        }
    }

    impl PartnerProductListingAuthorizerFactory<TxFake> for AuthorizerFake {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TxFake,
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
            lock(&self.0).authorizations += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl WoocommerceSourceReader for SourcesFake {
        async fn find_by_id(
            &self,
            _: ListingSourceId,
        ) -> Result<Option<WoocommerceSource>, ListingSourceReadError> {
            let mut state = lock(&self.0);
            state.source_reads += 1;
            Ok(state.source.clone())
        }
    }

    #[async_trait::async_trait]
    impl WoocommerceSignatureVerifier for SignatureVerifierFake {
        async fn verify(
            &self,
            _: ListingSourceId,
            _: &[u8],
            _: &[u8],
        ) -> Result<WoocommerceSignatureVerification, ListingSourceReadError> {
            let mut state = lock(&self.0);
            state.signature_checks += 1;
            Ok(state.signature)
        }
    }

    impl ProductListingTitleSlugGenerator for GeneratorFake {
        fn generate(
            &self,
            _: &str,
        ) -> Result<
            ProductListingSlugId,
            product_listing_core::product_listing_slug_id::InvalidProductListingSlugId,
        > {
            let candidate = ProductListingSlugId::from_title_and_suffix(
                "listing",
                &format!("{:06x}", lock(&self.0).candidates.len() + 1),
            )?;
            lock(&self.0).candidates.push(candidate.clone());
            Ok(candidate)
        }
    }

    fn state() -> SharedState {
        let listing_source_id = ListingSourceId::new();
        Arc::new(Mutex::new(State {
            source: Some(WoocommerceSource {
                listing_source_id,
                currency: Some(money::Currency::Eur),
                language: Some(localization::Language::En),
            }),
            signature: WoocommerceSignatureVerification::Valid,
            ..Default::default()
        }))
    }

    fn source_id(state: &SharedState) -> ListingSourceId {
        lock(state)
            .source
            .as_ref()
            .map(|source| source.listing_source_id)
            .unwrap_or_else(|| panic!("test source exists"))
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::User(UserId::new()),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn command(state: &SharedState) -> IngestWoocommerceProductListingCommand {
        IngestWoocommerceProductListingCommand {
            listing_source_id: source_id(state),
            kind: WoocommerceProductEventKind::Create,
            signature: vec![1],
            raw_body: vec![2],
            source_listing_id: SourceListingId::try_from("source-listing")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            title: Some("Listing".to_owned()),
            permalink: Some(
                Url::parse("https://example.com/listing")
                    .unwrap_or_else(|error| panic!("valid listing URL: {error}")),
            ),
            description_html: None,
            short_description_html: None,
            price: Some("42.00".to_owned()),
            status: Some("publish".to_owned()),
            stock_status: Some("instock".to_owned()),
            image_urls: IndexSet::new(),
        }
    }

    fn handler(
        state: &SharedState,
    ) -> IngestWoocommerceProductListingHandler<
        UowFake,
        ProductsFake,
        EventsFake,
        AuthorizerFake,
        SourcesFake,
        SignatureVerifierFake,
        GeneratorFake,
    > {
        IngestWoocommerceProductListingHandler::with_title_slug_generator(
            UowFake(Arc::clone(state)),
            ProductsFake(Arc::clone(state)),
            EventsFake(Arc::clone(state)),
            AuthorizerFake(Arc::clone(state)),
            SourcesFake(Arc::clone(state)),
            SignatureVerifierFake(Arc::clone(state)),
            GeneratorFake(Arc::clone(state)),
        )
    }

    fn existing_listing(listing_source_id: ListingSourceId) -> VersionedProductListing {
        let mut listing = ProductListing::create(NewProductListing {
            id: ProductListingId::new(),
            title_slug_id: ProductListingSlugId::from_title_and_suffix("listing", "000001")
                .unwrap_or_else(|error| panic!("valid title slug: {error}")),
            listing_source_id,
            source_listing_id: SourceListingId::try_from("source-listing")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            title: Some(Localized::new(
                localization::Language::En,
                Title::from("Listing"),
            )),
            description: None,
            pricing: ProductListingPricing {
                price: Some(Price::new(
                    MonetaryAmount::from(100_u64),
                    money::Currency::Eur,
                )),
                ..Default::default()
            },
            availability: None,
            url: Url::parse("https://example.com/old-listing")
                .unwrap_or_else(|error| panic!("valid old listing URL: {error}")),
            images: IndexSet::new(),
            auction: ProductListingAuction::default(),
        })
        .unwrap_or_else(|error| panic!("valid existing listing: {error}"));
        listing.take_pending_event_payload();
        Versioned::new(listing, ProductListingStorageVersion::INITIAL)
    }

    fn unchanged_listing(listing_source_id: ListingSourceId) -> VersionedProductListing {
        let mut listing = existing_listing(listing_source_id).value;
        listing
            .set_price(Price::new(
                MonetaryAmount::from(4_200_u64),
                money::Currency::Eur,
            ))
            .unwrap_or_else(|error| panic!("valid price update: {error}"));
        listing
            .set_availability(ListingAvailability::InStock)
            .unwrap_or_else(|error| panic!("valid availability update: {error}"));
        listing
            .change_url(
                Url::parse("https://example.com/listing")
                    .unwrap_or_else(|error| panic!("valid listing URL: {error}")),
            )
            .unwrap_or_else(|error| panic!("valid URL update: {error}"));
        listing.take_pending_event_payload();
        Versioned::new(listing, ProductListingStorageVersion::INITIAL)
    }

    #[tokio::test]
    async fn should_not_persist_or_append_when_existing_woocommerce_listing_is_unchanged() {
        let state = state();
        let listing_source_id = source_id(&state);
        lock(&state).finds = VecDeque::from([Some(unchanged_listing(listing_source_id))]);

        let result = handler(&state).execute(&context(), command(&state)).await;

        assert!(matches!(
            result,
            Ok(IngestWoocommerceProductListingResult::Upserted(
                UpsertProductListingResult::Updated(UpdateProductListingResult {
                    outcome: ChangeOutcome::Unchanged,
                    ..
                })
            ))
        ));
        let state = lock(&state);
        assert_eq!((state.updates, state.events), (0, 0));
    }

    #[tokio::test]
    async fn should_retry_title_slug_collision_then_commit_created_listing() {
        let state = state();
        lock(&state).inserts = VecDeque::from([
            Err(ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists),
            Ok(()),
        ]);

        assert!(matches!(
            handler(&state).execute(&context(), command(&state)).await,
            Ok(IngestWoocommerceProductListingResult::Upserted(
                UpsertProductListingResult::Created(_)
            ))
        ));

        let state = lock(&state);
        assert_eq!(
            (
                state.candidates.len(),
                state.begins,
                state.commits,
                state.rollbacks
            ),
            (2, 2, 1, 1)
        );
        assert_eq!(
            (
                state.events,
                state.authorizations,
                state.source_reads,
                state.signature_checks
            ),
            (1, 2, 2, 2)
        );
    }

    #[tokio::test]
    async fn should_exhaust_after_five_title_slug_collisions() {
        let state = state();
        lock(&state).inserts = VecDeque::from([
            Err(ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists),
            Err(ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists),
            Err(ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists),
            Err(ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists),
            Err(ProductListingRepositoryError::ProductListingTitleSlugAlreadyExists),
        ]);

        assert!(matches!(
            handler(&state).execute(&context(), command(&state)).await,
            Err(IngestWoocommerceProductListingError::ProductListingTitleSlugGenerationExhausted)
        ));

        let state = lock(&state);
        assert_eq!(
            (
                state.candidates.len(),
                state.begins,
                state.commits,
                state.rollbacks
            ),
            (5, 5, 0, 5)
        );
        assert_eq!(
            (
                state.events,
                state.authorizations,
                state.source_reads,
                state.signature_checks
            ),
            (0, 5, 5, 5)
        );
    }

    #[tokio::test]
    async fn should_rerun_source_key_race_then_update_winning_listing_without_new_candidate() {
        let state = state();
        let listing_source_id = source_id(&state);
        lock(&state).finds = VecDeque::from([None, Some(existing_listing(listing_source_id))]);
        lock(&state).inserts = VecDeque::from([Err(
            ProductListingRepositoryError::SourceListingAlreadyExists,
        )]);

        assert!(matches!(
            handler(&state).execute(&context(), command(&state)).await,
            Ok(IngestWoocommerceProductListingResult::Upserted(
                UpsertProductListingResult::Updated(_)
            ))
        ));

        let state = lock(&state);
        assert_eq!(
            (
                state.candidates.len(),
                state.begins,
                state.commits,
                state.rollbacks
            ),
            (1, 2, 1, 1)
        );
        assert_eq!(
            (
                state.updates,
                state.events,
                state.authorizations,
                state.source_reads,
                state.signature_checks
            ),
            (1, 1, 2, 2, 2)
        );
    }

    #[tokio::test]
    async fn should_update_existing_listing_without_generating_candidate() {
        let state = state();
        let listing_source_id = source_id(&state);
        lock(&state).finds = VecDeque::from([Some(existing_listing(listing_source_id))]);

        assert!(matches!(
            handler(&state).execute(&context(), command(&state)).await,
            Ok(IngestWoocommerceProductListingResult::Upserted(
                UpsertProductListingResult::Updated(_)
            ))
        ));

        let state = lock(&state);
        assert_eq!(
            (
                state.candidates.len(),
                state.begins,
                state.commits,
                state.rollbacks
            ),
            (0, 1, 1, 0)
        );
        assert_eq!(
            (state.updates, state.events, state.authorizations),
            (1, 1, 1)
        );
    }

    #[tokio::test]
    async fn should_not_generate_candidate_for_ignored_event() {
        let state = state();
        let mut command = command(&state);
        command.status = Some("future".to_owned());

        assert!(matches!(
            handler(&state).execute(&context(), command).await,
            Ok(IngestWoocommerceProductListingResult::Ignored)
        ));

        let state = lock(&state);
        assert_eq!(
            (
                state.candidates.len(),
                state.begins,
                state.commits,
                state.rollbacks,
                state.events
            ),
            (0, 1, 1, 0, 0)
        );
    }

    #[tokio::test]
    async fn should_withdraw_existing_listing_without_generating_candidate() {
        let state = state();
        let listing_source_id = source_id(&state);
        lock(&state).finds = VecDeque::from([Some(existing_listing(listing_source_id))]);
        let mut command = command(&state);
        command.kind = WoocommerceProductEventKind::Delete;

        assert!(matches!(
            handler(&state).execute(&context(), command).await,
            Ok(IngestWoocommerceProductListingResult::Withdrawn(_))
        ));

        let state = lock(&state);
        assert_eq!(
            (
                state.candidates.len(),
                state.begins,
                state.commits,
                state.rollbacks
            ),
            (0, 1, 1, 0)
        );
        assert_eq!(
            (state.updates, state.events, state.authorizations),
            (1, 1, 1)
        );
    }

    #[tokio::test]
    async fn should_not_generate_candidate_for_invalid_signature() {
        let state = state();
        lock(&state).signature = WoocommerceSignatureVerification::Invalid;

        assert!(matches!(
            handler(&state).execute(&context(), command(&state)).await,
            Err(IngestWoocommerceProductListingError::InvalidSignature)
        ));

        let state = lock(&state);
        assert_eq!(
            (
                state.candidates.len(),
                state.begins,
                state.commits,
                state.rollbacks
            ),
            (0, 0, 0, 0)
        );
        assert_eq!((state.source_reads, state.signature_checks), (1, 1));
    }

    #[tokio::test]
    async fn should_not_generate_candidate_when_source_is_missing() {
        let state = state();
        let mut command = command(&state);
        lock(&state).source = None;
        command.listing_source_id = ListingSourceId::new();

        assert!(matches!(
            handler(&state).execute(&context(), command).await,
            Err(IngestWoocommerceProductListingError::ListingSourceNotFound)
        ));

        let state = lock(&state);
        assert_eq!(
            (
                state.candidates.len(),
                state.begins,
                state.commits,
                state.rollbacks
            ),
            (0, 0, 0, 0)
        );
        assert_eq!((state.source_reads, state.signature_checks), (1, 0));
    }

    #[tokio::test]
    async fn should_not_generate_candidate_when_unauthorized() {
        let state = state();
        let context = OperationContext {
            principal: Principal::Anonymous,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        };

        assert!(matches!(
            handler(&state).execute(&context, command(&state)).await,
            Err(IngestWoocommerceProductListingError::AuthenticatedActorRequired)
        ));

        let state = lock(&state);
        assert_eq!(
            (
                state.candidates.len(),
                state.begins,
                state.commits,
                state.rollbacks
            ),
            (0, 0, 0, 0)
        );
        assert_eq!((state.source_reads, state.signature_checks), (0, 0));
    }

    #[tokio::test]
    async fn should_not_retry_unrelated_repository_error() {
        let state = state();
        lock(&state).inserts = VecDeque::from([Err(
            ProductListingRepositoryError::ProductListingInsertFailed,
        )]);

        assert!(matches!(
            handler(&state).execute(&context(), command(&state)).await,
            Err(IngestWoocommerceProductListingError::ProductListingPersistenceFailed)
        ));

        let state = lock(&state);
        assert_eq!(
            (
                state.candidates.len(),
                state.begins,
                state.commits,
                state.rollbacks,
                state.events
            ),
            (1, 1, 0, 1, 0)
        );
    }

    #[tokio::test]
    async fn should_not_retry_event_appender_error() {
        let state = state();
        lock(&state).event_results = VecDeque::from([Err(
            ProductListingEventAppendError::ProductListingEventAppendFailed {
                source: application::error::static_error("event append failed"),
            },
        )]);

        assert!(matches!(
            handler(&state).execute(&context(), command(&state)).await,
            Err(IngestWoocommerceProductListingError::ProductListingEventAppenderFailed { .. })
        ));

        let state = lock(&state);
        assert_eq!(
            (
                state.candidates.len(),
                state.begins,
                state.commits,
                state.rollbacks,
                state.events
            ),
            (1, 1, 0, 1, 1)
        );
    }

    #[tokio::test]
    async fn should_not_retry_commit_error() {
        let state = state();
        lock(&state).commit_results = VecDeque::from([Err(TransactionError::CommitFailed)]);

        assert!(matches!(
            handler(&state).execute(&context(), command(&state)).await,
            Err(IngestWoocommerceProductListingError::CommitTransactionFailed)
        ));

        let state = lock(&state);
        assert_eq!(
            (
                state.candidates.len(),
                state.begins,
                state.commits,
                state.rollbacks,
                state.events
            ),
            (1, 1, 0, 1, 1)
        );
    }
}
