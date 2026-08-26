use crate::ports::{
    PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
    PartnerProductListingAuthorizerFactory, ProductListingEventStore,
    ProductListingEventStoreError, ProductListingEventStoreFactory, ProductListingRepository,
    ProductListingRepositoryError, ProductListingRepositoryFactory, stamp_product_listing_events,
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
use localization::Localized;
use money::{MonetaryAmount, Price};
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing::{
    ChangeListingAvailabilityError, ChangeProductListingError, NewProductListing, ProductListing,
    ProductListingAddress, ProductListingAuction, ProductListingPricing,
    RehydrateProductListingError,
};
use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::prohibited_content::ProhibitedContent;
use product_listing_core::shop_listing_id::ShopListingId;
use product_listing_core::title::Title;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop_id::ShopId;
use shop_service::ports::{
    WoocommerceWebhookShop, WoocommerceWebhookShopReadError, WoocommerceWebhookShopReader,
    WoocommerceWebhookShopReaderFactory, WoocommerceWebhookSignatureVerification,
    WoocommerceWebhookSignatureVerifier, WoocommerceWebhookSignatureVerifierFactory,
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
    pub shop_id: ShopId,
    pub kind: WoocommerceProductEventKind,
    pub signature: Vec<u8>,
    pub raw_body: Vec<u8>,
    pub shop_listing_id: ShopListingId,
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
    shop_listing_id: ShopListingId,
    title: Localized<localization::Language, Title>,
    description: Option<Localized<localization::Language, Description>>,
    price: Option<Price>,
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
    #[error("shop has no WooCommerce currency configured")]
    MissingShopCurrency,
    #[error("shop has no WooCommerce language configured")]
    MissingShopLanguage,
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("actor may not ingest WooCommerce webhooks for this shop")]
    ActorMayNotIngestForShop,
    #[error("shop not found")]
    ShopNotFound,
    #[error("shop is not partnered")]
    ShopNotPartnered,
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
    #[error("temporary WooCommerce webhook shop read failure")]
    WebhookShopTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid WooCommerce webhook shop read model")]
    InvalidWebhookShopReadModel {
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

pub struct IngestWoocommerceProductListingHandler<U, R, E, A, S, V> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
    shops: S,
    signature_verifier: V,
}

impl<U, R, E, A, S, V> IngestWoocommerceProductListingHandler<U, R, E, A, S, V> {
    pub fn new(
        unit_of_work: U,
        products: R,
        events: E,
        authorizer: A,
        shops: S,
        signature_verifier: V,
    ) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            authorizer,
            shops,
            signature_verifier,
        }
    }
}

impl<U, R, E, A, S, V> IngestWoocommerceProductListingHandler<U, R, E, A, S, V>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    S: WoocommerceWebhookShopReaderFactory<U::Tx>,
    V: WoocommerceWebhookSignatureVerifierFactory<U::Tx>,
{
    async fn validate_webhook(
        &self,
        tx: &mut U::Tx,
        context: &OperationContext,
        shop_id: ShopId,
        body: &[u8],
        signature: &[u8],
    ) -> Result<WoocommerceWebhookShop, IngestWoocommerceProductListingError> {
        if let Some(actor_id) = partner_actor(&context.principal) {
            self.authorizer
                .in_transaction(tx)
                .authorize(actor_id, shop_id)
                .await?;
        }
        let shop = self
            .shops
            .in_transaction(tx)
            .find_for_webhook(shop_id)
            .await?
            .ok_or(IngestWoocommerceProductListingError::ShopNotFound)?;
        if shop.partner_status != ShopPartnerStatus::Partnered {
            return Err(IngestWoocommerceProductListingError::ShopNotPartnered);
        }
        match self
            .signature_verifier
            .verifier_in_transaction(tx)
            .verify(shop_id, body, signature)
            .await?
        {
            WoocommerceWebhookSignatureVerification::Valid => Ok(shop),
            WoocommerceWebhookSignatureVerification::Invalid => {
                Err(IngestWoocommerceProductListingError::InvalidSignature)
            }
            WoocommerceWebhookSignatureVerification::SecretNotConfigured => {
                Err(IngestWoocommerceProductListingError::WebhookSecretNotConfigured)
            }
        }
    }

    async fn upsert(
        &self,
        tx: &mut U::Tx,
        shop: &WoocommerceWebhookShop,
        data: WoocommerceListingData,
    ) -> Result<UpsertProductListingResult, IngestWoocommerceProductListingError> {
        let key = ProductListingKey::new(shop.shop_id, data.shop_listing_id.clone());
        let existing = self.products.in_transaction(tx).find_by_key(&key).await?;
        match existing {
            Some(loaded) => {
                let expected_event_id = loaded.version;
                let mut listing = loaded.value;
                listing.restore();
                let pricing = ProductListingPricing {
                    price: data.price.or(listing.pricing().price),
                    price_estimate_min: listing.pricing().price_estimate_min,
                    price_estimate_max: listing.pricing().price_estimate_max,
                };
                listing.replace_pricing(pricing)?;
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
                    id: ProductListingId::new(),
                    shop_id: shop.shop_id,
                    seller_id: shop.shop_id,
                    shop_listing_id: data.shop_listing_id,
                    address: ProductListingAddress::default(),
                    title: Some(data.title),
                    description: data.description,
                    pricing: ProductListingPricing {
                        price: data.price,
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
                    .await?;
                for event in &events {
                    self.events.in_transaction(tx).append(event).await?;
                }
                Ok(UpsertProductListingResult::Created(
                    CreateProductListingResult {
                        product_listing_id: persisted.value.id(),
                        product_listing_slug_id: persisted.value.slug_id().clone(),
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
}

#[async_trait::async_trait]
impl<U, R, E, A, S, V> IngestWoocommerceProductListingUseCase
    for IngestWoocommerceProductListingHandler<U, R, E, A, S, V>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
    S: WoocommerceWebhookShopReaderFactory<U::Tx>,
    V: WoocommerceWebhookSignatureVerifierFactory<U::Tx>,
{
    #[tracing::instrument(name = "ingest_woocommerce_product_listing", skip_all, fields(shop_id = %command.shop_id, shop_listing_id = %command.shop_listing_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: IngestWoocommerceProductListingCommand,
    ) -> Result<IngestWoocommerceProductListingResult, IngestWoocommerceProductListingError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| IngestWoocommerceProductListingError::BeginTransactionFailed)?;
        context
            .require()
            .credential_capability(CredentialCapability::ProductListingsWrite)
            .authorize::<IngestWoocommerceProductListingError>()?;
        let shop = self
            .validate_webhook(
                &mut tx,
                context,
                command.shop_id,
                &command.raw_body,
                &command.signature,
            )
            .await?;
        let result = match listing_action(
            command.kind,
            command.status.as_deref(),
            command.stock_status.as_deref(),
        ) {
            WoocommerceListingAction::Ignore => IngestWoocommerceProductListingResult::Ignored,
            WoocommerceListingAction::Withdraw => self
                .withdraw(
                    &mut tx,
                    ProductListingKey::new(shop.shop_id, command.shop_listing_id),
                )
                .await?
                .map(IngestWoocommerceProductListingResult::Withdrawn)
                .unwrap_or(IngestWoocommerceProductListingResult::Ignored),
            WoocommerceListingAction::Upsert(availability) => {
                IngestWoocommerceProductListingResult::Upserted(
                    self.upsert(&mut tx, &shop, listing_data(command, availability, &shop)?)
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

fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}

fn listing_data(
    command: IngestWoocommerceProductListingCommand,
    availability: PatchField<ListingAvailability>,
    shop: &WoocommerceWebhookShop,
) -> Result<WoocommerceListingData, IngestWoocommerceProductListingError> {
    let language = shop
        .language
        .ok_or(IngestWoocommerceProductListingError::MissingShopLanguage)?;
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
        .map(|url| ProductListingImage {
            url,
            prohibited_content: ProhibitedContent::Unknown,
        })
        .collect();
    Ok(WoocommerceListingData {
        shop_listing_id: command.shop_listing_id,
        title: Localized::new(language, Title::from(title)),
        description,
        price: parse_price(command.price.as_deref(), shop.currency)?,
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
    let currency = currency.ok_or(IngestWoocommerceProductListingError::MissingShopCurrency)?;
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
            Some(_) | None => PatchField::Clear,
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
            PartnerProductListingAuthorizationError::ShopNotFound => Self::ShopNotFound,
            PartnerProductListingAuthorizationError::Forbidden => Self::ActorMayNotIngestForShop,
            PartnerProductListingAuthorizationError::TemporarilyUnavailable { source } => {
                Self::PartnerAuthorizationTemporarilyUnavailable { source }
            }
            PartnerProductListingAuthorizationError::Internal { source } => {
                Self::PartnerAuthorizationInternal { source }
            }
        }
    }
}

impl From<WoocommerceWebhookShopReadError> for IngestWoocommerceProductListingError {
    fn from(error: WoocommerceWebhookShopReadError) -> Self {
        match error {
            WoocommerceWebhookShopReadError::TemporarilyUnavailable { source } => {
                Self::WebhookShopTemporarilyUnavailable { source }
            }
            WoocommerceWebhookShopReadError::InvalidReadModel { source } => {
                Self::InvalidWebhookShopReadModel { source }
            }
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

impl From<ChangeListingAvailabilityError> for IngestWoocommerceProductListingError {
    fn from(_: ChangeListingAvailabilityError) -> Self {
        Self::ListingWithdrawn
    }
}

impl From<ChangeProductListingError> for IngestWoocommerceProductListingError {
    fn from(_: ChangeProductListingError) -> Self {
        Self::ListingWithdrawn
    }
}

impl From<RehydrateProductListingError> for IngestWoocommerceProductListingError {
    fn from(error: RehydrateProductListingError) -> Self {
        Self::InvalidProductListing {
            source: box_error(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_map_published_stock_statuses_without_creating_sale_observations() {
        let cases = [
            (
                Some("instock"),
                PatchField::Set(ListingAvailability::InStock),
            ),
            (
                Some("outofstock"),
                PatchField::Set(ListingAvailability::OutOfStock),
            ),
            (
                Some("onbackorder"),
                PatchField::Set(ListingAvailability::BackOrder),
            ),
            (None, PatchField::Clear),
            (Some("unsupported"), PatchField::Clear),
        ];

        for (stock_status, expected) in cases {
            assert!(matches!(
                listing_action(WoocommerceProductEventKind::Update, Some("publish"), stock_status),
                WoocommerceListingAction::Upsert(actual) if actual == expected
            ));
        }
    }

    #[test]
    fn should_withdraw_only_authoritative_non_public_observations() {
        for status in ["trash", "draft", "pending", "private"] {
            assert!(matches!(
                listing_action(WoocommerceProductEventKind::Update, Some(status), None),
                WoocommerceListingAction::Withdraw
            ));
        }
        assert!(matches!(
            listing_action(WoocommerceProductEventKind::Delete, None, None),
            WoocommerceListingAction::Withdraw
        ));
    }

    #[test]
    fn should_ignore_missing_or_unsupported_status() {
        assert!(matches!(
            listing_action(WoocommerceProductEventKind::Update, None, None),
            WoocommerceListingAction::Ignore
        ));
        assert!(matches!(
            listing_action(
                WoocommerceProductEventKind::Update,
                Some("future-status"),
                Some("instock")
            ),
            WoocommerceListingAction::Ignore
        ));
    }
}
