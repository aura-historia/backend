use crate::use_cases::{
    UpsertProductListingCommand, UpsertProductListingError, UpsertProductListingResult,
    UpsertProductListingUseCase, WithdrawProductListingError, WithdrawProductListingResult,
    WithdrawProductListingUseCase,
};
use application::error::BoxError;
use application::operation_context::{CredentialCapability, OperationContext, Principal};
use application::transaction::{Transaction, UnitOfWork};
use indexmap::IndexSet;
use localization::Localized;
use money::{MonetaryAmount, Price};
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing::ProductListingAddress;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::prohibited_content::ProhibitedContent;
use product_listing_core::shop_listing_id::ShopListingId;
use product_listing_core::title::Title;
use shop_core::partner_status::ShopPartnerStatus;
use shop_core::shop_id::ShopId;
use shop_service::ports::{
    PartnerShopReadError, PartnerShopReader, PartnerShopReaderFactory, WoocommerceWebhookShop,
    WoocommerceWebhookShopReadError, WoocommerceWebhookShopReader,
    WoocommerceWebhookShopReaderFactory, WoocommerceWebhookSignatureVerification,
    WoocommerceWebhookSignatureVerifier, WoocommerceWebhookSignatureVerifierFactory,
};
use shop_service::use_cases::CheckUserPartnerShopRequest;
use url::Url;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
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
    Upserted(UpsertProductListingResult),
    Withdrawn(WithdrawProductListingResult),
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
    #[error("temporary partner shop membership read failure")]
    PartnerMembershipTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid partner shop membership read model")]
    InvalidPartnerMembershipReadModel {
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
    #[error("product listing upsert failed")]
    ProductListingUpsertFailed {
        #[source]
        source: UpsertProductListingError,
    },
    #[error("product listing withdrawal failed")]
    ProductListingWithdrawalFailed {
        #[source]
        source: WithdrawProductListingError,
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
pub struct IngestWoocommerceProductListingHandler<U, M, S, V, P, W> {
    unit_of_work: U,
    memberships: M,
    shops: S,
    signature_verifier: V,
    products: P,
    withdrawals: W,
}
impl<U, M, S, V, P, W> IngestWoocommerceProductListingHandler<U, M, S, V, P, W> {
    pub fn new(
        unit_of_work: U,
        memberships: M,
        shops: S,
        signature_verifier: V,
        products: P,
        withdrawals: W,
    ) -> Self {
        Self {
            unit_of_work,
            memberships,
            shops,
            signature_verifier,
            products,
            withdrawals,
        }
    }
}
impl<U, M, S, V, P, W> IngestWoocommerceProductListingHandler<U, M, S, V, P, W>
where
    U: UnitOfWork,
    M: PartnerShopReaderFactory<U::Tx>,
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
    ) -> Result<(UserId, WoocommerceWebhookShop), IngestWoocommerceProductListingError> {
        let actor_id = actor_id(context)?;
        if !self
            .memberships
            .in_transaction(tx)
            .is_user_partner_of_shop(&CheckUserPartnerShopRequest {
                user_id: actor_id,
                shop_id,
            })
            .await?
        {
            return Err(IngestWoocommerceProductListingError::ActorMayNotIngestForShop);
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
            WoocommerceWebhookSignatureVerification::Valid => Ok((actor_id, shop)),
            WoocommerceWebhookSignatureVerification::Invalid => {
                Err(IngestWoocommerceProductListingError::InvalidSignature)
            }
            WoocommerceWebhookSignatureVerification::SecretNotConfigured => {
                Err(IngestWoocommerceProductListingError::WebhookSecretNotConfigured)
            }
        }
    }
}
#[async_trait::async_trait]
impl<U, M, S, V, P, W> IngestWoocommerceProductListingUseCase
    for IngestWoocommerceProductListingHandler<U, M, S, V, P, W>
where
    U: UnitOfWork,
    M: PartnerShopReaderFactory<U::Tx>,
    S: WoocommerceWebhookShopReaderFactory<U::Tx>,
    V: WoocommerceWebhookSignatureVerifierFactory<U::Tx>,
    P: UpsertProductListingUseCase,
    W: WithdrawProductListingUseCase,
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
        let (_, shop) = self
            .validate_webhook(
                &mut tx,
                context,
                command.shop_id,
                &command.raw_body,
                &command.signature,
            )
            .await?;
        tx.commit()
            .await
            .map_err(|_| IngestWoocommerceProductListingError::CommitTransactionFailed)?;
        match command.kind {
            WoocommerceProductEventKind::Delete => self
                .withdrawals
                .execute_by_key(
                    context,
                    product_listing_core::product_listing_id::ProductListingKey::new(
                        shop.shop_id,
                        command.shop_listing_id,
                    ),
                )
                .await
                .map(IngestWoocommerceProductListingResult::Withdrawn)
                .map_err(|source| {
                    IngestWoocommerceProductListingError::ProductListingWithdrawalFailed { source }
                }),
            WoocommerceProductEventKind::Create | WoocommerceProductEventKind::Update => {
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
                let price = parse_price(command.price.as_deref(), shop.currency)?;
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
                self.products
                    .execute(
                        context,
                        UpsertProductListingCommand {
                            shop_id: shop.shop_id,
                            seller_id: shop.shop_id,
                            shop_listing_id: command.shop_listing_id,
                            address: ProductListingAddress::default(),
                            title: Some(Localized::new(language, Title::from(title))),
                            description,
                            price,
                            price_estimate_min: None,
                            price_estimate_max: None,
                            availability: product_availability(
                                command.status.as_deref(),
                                command.stock_status.as_deref(),
                            ),
                            url: Some(url),
                            images,
                            auction_start: None,
                            auction_end: None,
                        },
                    )
                    .await
                    .map(IngestWoocommerceProductListingResult::Upserted)
                    .map_err(|source| {
                        IngestWoocommerceProductListingError::ProductListingUpsertFailed { source }
                    })
            }
        }
    }
}
fn actor_id(context: &OperationContext) -> Result<UserId, IngestWoocommerceProductListingError> {
    match &context.principal {
        Principal::User(id) => Ok(*id),
        Principal::DelegatedUser {
            user_id,
            capabilities,
        } if capabilities.contains(&CredentialCapability::PartnerShopsWrite) => Ok(*user_id),
        Principal::Anonymous => {
            Err(IngestWoocommerceProductListingError::AuthenticatedActorRequired)
        }
        Principal::DelegatedUser { .. } | Principal::Service(_) | Principal::System => {
            Err(IngestWoocommerceProductListingError::ActorMayNotIngestForShop)
        }
    }
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
fn product_availability(
    status: Option<&str>,
    stock_status: Option<&str>,
) -> Option<ListingAvailability> {
    match status {
        Some("publish") if stock_status == Some("outofstock") => {
            Some(ListingAvailability::OutOfStock)
        }
        Some("publish") if stock_status == Some("onbackorder") => {
            Some(ListingAvailability::BackOrder)
        }
        Some("publish") => Some(ListingAvailability::InStock),
        _ => None,
    }
}
impl From<PartnerShopReadError> for IngestWoocommerceProductListingError {
    fn from(error: PartnerShopReadError) -> Self {
        match error {
            PartnerShopReadError::TemporarilyUnavailable { source } => {
                Self::PartnerMembershipTemporarilyUnavailable { source }
            }
            PartnerShopReadError::InvalidReadModel { source }
            | PartnerShopReadError::Internal { source } => {
                Self::InvalidPartnerMembershipReadModel { source }
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
