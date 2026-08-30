use crate::ports::{
    PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
    PartnerProductListingAuthorizerFactory, ProductListingEventStore,
    ProductListingEventStoreError, ProductListingEventStoreFactory, ProductListingRepository,
    ProductListingRepositoryError, ProductListingRepositoryFactory, stamp_product_listing_events,
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
    product_listing_slug_id::ProductListingSlugId,
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
    #[error("product listing event storage failed")]
    ProductListingEventStoreFailed,
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
impl From<ProductListingEventStoreError> for WoocommerceIngestAttemptError {
    fn from(error: ProductListingEventStoreError) -> Self {
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
    pub fn with_title_slug_generator(
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
    E: ProductListingEventStoreFactory<U::Tx>,
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
        title_slug_id: ProductListingSlugId,
    ) -> Result<UpsertProductListingResult, WoocommerceIngestAttemptError> {
        let key = ProductListingKey::new(source.listing_source_id, data.source_listing_id.clone());
        let existing = self.products.in_transaction(tx).find_by_key(&key).await?;
        match existing {
            Some(loaded) => {
                let expected_event_id = loaded.version;
                let mut listing = loaded.value;
                listing.restore();
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
                let events = stamp_product_listing_events(
                    listing.id(),
                    time::OffsetDateTime::now_utc(),
                    listing.take_pending_event_payloads(),
                );
                let event_id = events.last().map(|event| event.event_id);
                if let Some(new_event_id) = event_id {
                    listing = self
                        .products
                        .in_transaction(tx)
                        .update(&listing, expected_event_id, new_event_id)
                        .await?
                        .value;
                    for event in &events {
                        self.events.in_transaction(tx).append(event).await?;
                    }
                }
                Ok(UpsertProductListingResult::Updated(
                    UpdateProductListingResult {
                        product_listing_id: listing.id(),
                        event_id,
                    },
                ))
            }
            None => {
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
                let events = stamp_product_listing_events(
                    listing.id(),
                    time::OffsetDateTime::now_utc(),
                    listing.take_pending_event_payloads(),
                );
                let event_id = events.last().map(|event| event.event_id).ok_or_else(|| {
                    IngestWoocommerceProductListingError::InvalidProductListing {
                        source: box_error(std::io::Error::other("created listing has no event")),
                    }
                })?;
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
                for event in &events {
                    self.events.in_transaction(tx).append(event).await?;
                }
                Ok(UpsertProductListingResult::Created(
                    CreateProductListingResult {
                        product_listing_id: persisted.value.id(),
                        product_listing_title_slug_id: persisted.value.title_slug_id().clone(),
                        event_id,
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
        let expected_event_id = loaded.version;
        let mut listing = loaded.value;
        listing.withdraw();
        let events = stamp_product_listing_events(
            listing.id(),
            time::OffsetDateTime::now_utc(),
            listing.take_pending_event_payloads(),
        );
        let event_id = events
            .last()
            .map(|event| event.event_id)
            .unwrap_or(expected_event_id);
        if !events.is_empty() {
            listing = self
                .products
                .in_transaction(tx)
                .update(&listing, expected_event_id, event_id)
                .await?
                .value;
            for event in &events {
                self.events.in_transaction(tx).append(event).await?;
            }
        }
        Ok(Some(WithdrawProductListingResult {
            product_listing_id: listing.id(),
            event_id,
        }))
    }

    async fn execute_attempt(
        &self,
        context: &OperationContext,
        command: IngestWoocommerceProductListingCommand,
        new_product_listing_id: ProductListingId,
        title_slug_id: ProductListingSlugId,
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
                        title_slug_id,
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
    E: ProductListingEventStoreFactory<U::Tx>,
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
            let title_slug_id = self
                .title_slug_generator
                .generate(command.title.as_deref().unwrap_or_default())
                .map_err(
                    |_| IngestWoocommerceProductListingError::InvalidProductListing {
                        source: box_error(std::io::Error::other("invalid generated title slug")),
                    },
                )?;
            match self
                .execute_attempt(
                    context,
                    command.clone(),
                    new_product_listing_id,
                    title_slug_id,
                )
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
impl From<ProductListingEventStoreError> for IngestWoocommerceProductListingError {
    fn from(_: ProductListingEventStoreError) -> Self {
        Self::ProductListingEventStoreFailed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
