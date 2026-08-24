use crate::ports::{
    PartnerProductAuthorizer, PartnerProductAuthorizerFactory, ProductEventStore,
    ProductEventStoreFactory, ProductRepository, ProductRepositoryFactory,
};
use crate::use_cases::{
    UpdateProductError, UpdateProductResult, UpsertProductError, UpsertProductResult,
};
use application::error::BoxError;
use application::operation_context::{CredentialCapability, OperationContext, Principal};
use application::transaction::{Transaction, UnitOfWork};
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use indexmap::IndexSet;
use localization::Localized;
use money::{MonetaryAmount, Price};
use product_listing_core::description::Description;
use product_listing_core::product::{
    NewProduct, Product, ProductAddress, ProductAuction, ProductPricing, ProductSaleValuation,
};
use product_listing_core::product_id::{ProductId, ProductKey};
use product_listing_core::product_image::ProductImage;
use product_listing_core::product_state::ProductState;
use product_listing_core::prohibited_content::ProhibitedContent;
use product_listing_core::shops_product_id::ShopsProductId;
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
pub struct IngestWoocommerceProductCommand {
    pub shop_id: ShopId,
    pub kind: WoocommerceProductEventKind,
    pub signature: Vec<u8>,
    pub raw_body: Vec<u8>,
    pub shops_product_id: ShopsProductId,
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
pub enum IngestWoocommerceProductResult {
    Upserted(UpsertProductResult),
    Removed(UpdateProductResult),
}

#[derive(Debug, thiserror::Error)]
pub enum IngestWoocommerceProductError {
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
    #[error("product upsert failed")]
    ProductUpsertFailed {
        #[source]
        source: UpsertProductError,
    },
    #[error("product removal failed")]
    ProductRemovalFailed {
        #[source]
        source: UpdateProductError,
    },
    #[error("failed to begin WooCommerce product ingestion transaction")]
    BeginTransactionFailed,
    #[error("failed to commit WooCommerce product ingestion transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait IngestWoocommerceProductUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: IngestWoocommerceProductCommand,
    ) -> Result<IngestWoocommerceProductResult, IngestWoocommerceProductError>;
}

struct CanonicalWoocommerceProduct {
    shop_id: ShopId,
    shops_product_id: ShopsProductId,
    title: Localized<localization::Language, Title>,
    description: Option<Localized<localization::Language, Description>>,
    price: Option<Price>,
    state: ProductState,
    url: Url,
    images: IndexSet<ProductImage>,
}

pub struct IngestWoocommerceProductHandler<U, M, S, V, R, E, A, F> {
    unit_of_work: U,
    memberships: M,
    shops: S,
    signature_verifier: V,
    products: R,
    events: E,
    authorizer: A,
    fx_rates: F,
}

impl<U, M, S, V, R, E, A, F> IngestWoocommerceProductHandler<U, M, S, V, R, E, A, F> {
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_fx_rates(
        unit_of_work: U,
        memberships: M,
        shops: S,
        signature_verifier: V,
        products: R,
        events: E,
        authorizer: A,
        fx_rates: F,
    ) -> Self {
        Self {
            unit_of_work,
            memberships,
            shops,
            signature_verifier,
            products,
            events,
            authorizer,
            fx_rates,
        }
    }
}

impl<U, M, S, V, R, E, A, F> IngestWoocommerceProductHandler<U, M, S, V, R, E, A, F>
where
    U: UnitOfWork,
    M: PartnerShopReaderFactory<U::Tx>,
    S: WoocommerceWebhookShopReaderFactory<U::Tx>,
    V: WoocommerceWebhookSignatureVerifierFactory<U::Tx>,
    R: ProductRepositoryFactory<U::Tx>,
    E: ProductEventStoreFactory<U::Tx>,
    A: PartnerProductAuthorizerFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    async fn validate_webhook(
        &self,
        tx: &mut U::Tx,
        context: &OperationContext,
        shop_id: ShopId,
        body: &[u8],
        signature: &[u8],
    ) -> Result<(UserId, WoocommerceWebhookShop), IngestWoocommerceProductError> {
        let actor_id = actor_id(context)?;
        let is_partner = self
            .memberships
            .in_transaction(tx)
            .is_user_partner_of_shop(&CheckUserPartnerShopRequest {
                user_id: actor_id,
                shop_id,
            })
            .await?;
        if !is_partner {
            return Err(IngestWoocommerceProductError::ActorMayNotIngestForShop);
        }

        let shop = WoocommerceWebhookShopReaderFactory::in_transaction(&self.shops, tx)
            .find_for_webhook(shop_id)
            .await?
            .ok_or(IngestWoocommerceProductError::ShopNotFound)?;
        if shop.partner_status != ShopPartnerStatus::Partnered {
            return Err(IngestWoocommerceProductError::ShopNotPartnered);
        }

        match self
            .signature_verifier
            .verifier_in_transaction(tx)
            .verify(shop_id, body, signature)
            .await?
        {
            WoocommerceWebhookSignatureVerification::Valid => Ok((actor_id, shop)),
            WoocommerceWebhookSignatureVerification::Invalid => {
                Err(IngestWoocommerceProductError::InvalidSignature)
            }
            WoocommerceWebhookSignatureVerification::SecretNotConfigured => {
                Err(IngestWoocommerceProductError::WebhookSecretNotConfigured)
            }
        }
    }

    async fn persist_upsert(
        &self,
        tx: &mut U::Tx,
        context: &OperationContext,
        actor_id: UserId,
        product: CanonicalWoocommerceProduct,
    ) -> Result<UpsertProductResult, UpsertProductError> {
        let CanonicalWoocommerceProduct {
            shop_id,
            shops_product_id,
            title,
            description,
            price,
            state,
            url,
            images,
        } = product;
        context
            .require()
            .credential_capability(CredentialCapability::ProductsWrite)
            .authorize::<UpsertProductError>()?;
        self.authorizer
            .in_transaction(tx)
            .authorize(actor_id, shop_id)
            .await?;

        let key = ProductKey::new(shop_id, shops_product_id.clone());
        let existing = self.products.in_transaction(tx).find_by_key(&key).await?;
        match existing {
            Some(loaded) => {
                let expected_event_id = loaded.version;
                let mut product = loaded.value;
                if let Some(price) = price {
                    let mut pricing = product.pricing();
                    pricing.price = Some(price);
                    product.replace_pricing(pricing);
                }
                let sale_valuation =
                    if state == ProductState::Sold && product.state() != ProductState::Sold {
                        let sold_at = time::OffsetDateTime::now_utc();
                        Some(sale_valuation(&self.fx_rates, tx, sold_at).await?)
                    } else {
                        None
                    };
                apply_state(&mut product, state, sale_valuation)?;
                product.change_url(url);
                product.replace_images(images);
                let events = product.take_pending_events();
                let event_id = events.last().map(|event| event.event_id);

                if let Some(new_event_id) = event_id {
                    product = self
                        .products
                        .in_transaction(tx)
                        .update(&product, expected_event_id, new_event_id)
                        .await?
                        .value;
                    for event in &events {
                        self.events.in_transaction(tx).append(event).await?;
                    }
                }

                Ok(UpsertProductResult::Updated(UpdateProductResult {
                    product_id: product.id(),
                    event_id,
                }))
            }
            None => {
                let sale_valuation = if state == ProductState::Sold {
                    let sold_at = time::OffsetDateTime::now_utc();
                    Some(sale_valuation(&self.fx_rates, tx, sold_at).await?)
                } else {
                    None
                };
                let product = Product::create(NewProduct {
                    id: ProductId::new(),
                    shop_id,
                    seller_id: shop_id,
                    shops_product_id,
                    address: ProductAddress::default(),
                    title: Some(title),
                    description,
                    pricing: ProductPricing {
                        price,
                        ..Default::default()
                    },
                    sale_valuation,
                    state,
                    url,
                    images,
                    auction: ProductAuction::default(),
                })?;
                let event_id = product
                    .pending_events()
                    .last()
                    .map(|event| event.event_id)
                    .ok_or(UpsertProductError::InvalidProductState)?;
                let persisted = self
                    .products
                    .in_transaction(tx)
                    .insert(&product, event_id)
                    .await?;
                for event in product.pending_events() {
                    self.events.in_transaction(tx).append(event).await?;
                }

                Ok(UpsertProductResult::Created(
                    crate::use_cases::CreateProductResult {
                        product_id: persisted.value.id(),
                        product_slug_id: persisted.value.slug_id().clone(),
                        event_id,
                    },
                ))
            }
        }
    }

    async fn persist_removal(
        &self,
        tx: &mut U::Tx,
        context: &OperationContext,
        actor_id: UserId,
        shop_id: ShopId,
        shops_product_id: ShopsProductId,
    ) -> Result<UpdateProductResult, UpdateProductError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductsWrite)
            .authorize::<UpdateProductError>()?;
        self.authorizer
            .in_transaction(tx)
            .authorize(actor_id, shop_id)
            .await?;

        let key = ProductKey::new(shop_id, shops_product_id);
        let loaded = self
            .products
            .in_transaction(tx)
            .find_by_key(&key)
            .await?
            .ok_or(UpdateProductError::ProductNotFound)?;
        let expected_event_id = loaded.version;
        let mut product = loaded.value;
        product.mark_removed()?;
        let events = product.take_pending_events();
        let event_id = events.last().map(|event| event.event_id);

        if let Some(new_event_id) = event_id {
            product = self
                .products
                .in_transaction(tx)
                .update(&product, expected_event_id, new_event_id)
                .await?
                .value;
            for event in &events {
                self.events.in_transaction(tx).append(event).await?;
            }
        }

        Ok(UpdateProductResult {
            product_id: product.id(),
            event_id,
        })
    }
}

#[async_trait::async_trait]
impl<U, M, S, V, R, E, A, F> IngestWoocommerceProductUseCase
    for IngestWoocommerceProductHandler<U, M, S, V, R, E, A, F>
where
    U: UnitOfWork,
    M: PartnerShopReaderFactory<U::Tx>,
    S: WoocommerceWebhookShopReaderFactory<U::Tx>,
    V: WoocommerceWebhookSignatureVerifierFactory<U::Tx>,
    R: ProductRepositoryFactory<U::Tx>,
    E: ProductEventStoreFactory<U::Tx>,
    A: PartnerProductAuthorizerFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "ingest_woocommerce_product",
        skip_all,
        fields(
            shop_id = %command.shop_id,
            shops_product_id = %command.shops_product_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: IngestWoocommerceProductCommand,
    ) -> Result<IngestWoocommerceProductResult, IngestWoocommerceProductError> {
        let IngestWoocommerceProductCommand {
            shop_id,
            kind,
            signature,
            raw_body,
            shops_product_id,
            title,
            permalink,
            description_html,
            short_description_html,
            price,
            status,
            stock_status,
            image_urls,
        } = command;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| IngestWoocommerceProductError::BeginTransactionFailed)?;
        let (actor_id, shop) = self
            .validate_webhook(&mut tx, context, shop_id, &raw_body, &signature)
            .await?;

        let result = match kind {
            WoocommerceProductEventKind::Delete => self
                .persist_removal(&mut tx, context, actor_id, shop.shop_id, shops_product_id)
                .await
                .map(IngestWoocommerceProductResult::Removed)
                .map_err(|source| IngestWoocommerceProductError::ProductRemovalFailed { source }),
            WoocommerceProductEventKind::Create | WoocommerceProductEventKind::Update => {
                let language = shop
                    .language
                    .ok_or(IngestWoocommerceProductError::MissingShopLanguage)?;
                let title = title
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or(IngestWoocommerceProductError::MissingTitle)?;
                let url = permalink.ok_or(IngestWoocommerceProductError::MissingUrl)?;
                let price = parse_price(price.as_deref(), shop.currency)?;
                let description = description_html
                    .as_deref()
                    .or(short_description_html.as_deref())
                    .map(fallbacked_html_to_markdown)
                    .filter(|value| !value.is_empty())
                    .map(Description::from)
                    .map(|value| Localized::new(language, value));
                let images = image_urls
                    .into_iter()
                    .map(|url| ProductImage {
                        url,
                        prohibited_content: ProhibitedContent::Unknown,
                    })
                    .collect();
                self.persist_upsert(
                    &mut tx,
                    context,
                    actor_id,
                    CanonicalWoocommerceProduct {
                        shop_id: shop.shop_id,
                        shops_product_id,
                        title: Localized::new(language, Title::from(title)),
                        description,
                        price,
                        state: product_state(status.as_deref(), stock_status.as_deref()),
                        url,
                        images,
                    },
                )
                .await
                .map(IngestWoocommerceProductResult::Upserted)
                .map_err(|source| IngestWoocommerceProductError::ProductUpsertFailed { source })
            }
        }?;
        tx.commit()
            .await
            .map_err(|_| IngestWoocommerceProductError::CommitTransactionFailed)?;
        Ok(result)
    }
}

fn apply_state(
    product: &mut Product,
    state: ProductState,
    sale_valuation: Option<ProductSaleValuation>,
) -> Result<(), UpsertProductError> {
    if product.state() == state {
        return Ok(());
    }
    match state {
        ProductState::Listed => product.mark_listed()?,
        ProductState::Available => product.mark_available()?,
        ProductState::Reserved => product.mark_reserved()?,
        ProductState::Sold => {
            product.mark_sold(sale_valuation.ok_or(UpsertProductError::SaleFxSnapshotMissing)?)?
        }
        ProductState::Removed => product.mark_removed()?,
        ProductState::Unknown => product.mark_unknown()?,
    };
    Ok(())
}

async fn sale_valuation<Tx, F>(
    fx_rates: &F,
    tx: &mut Tx,
    sold_at: time::OffsetDateTime,
) -> Result<ProductSaleValuation, UpsertProductError>
where
    F: FxRateSnapshotRepositoryFactory<Tx>,
{
    let mut repository = fx_rates.in_transaction(tx);
    let snapshot = repository
        .find_latest_at_or_before(sold_at)
        .await
        .map_err(|error| match error {
            FxRateSnapshotRepositoryError::InsertFailed { source }
            | FxRateSnapshotRepositoryError::ReadFailed { source } => {
                UpsertProductError::SaleFxSnapshotUnavailable { source }
            }
            FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
                UpsertProductError::SaleFxSnapshotInvalid { source }
            }
            FxRateSnapshotRepositoryError::CapturedAtNotMonotonic => {
                UpsertProductError::SaleFxSnapshotMissing
            }
        })?
        .ok_or(UpsertProductError::SaleFxSnapshotMissing)?;
    Ok(ProductSaleValuation {
        sold_at,
        fx_rate_id: snapshot.id(),
    })
}

fn actor_id(context: &OperationContext) -> Result<UserId, IngestWoocommerceProductError> {
    match &context.principal {
        Principal::User(user_id) => Ok(*user_id),
        Principal::DelegatedUser {
            user_id,
            capabilities,
        } => {
            if capabilities.contains(&CredentialCapability::ProductsWrite) {
                Ok(*user_id)
            } else {
                Err(IngestWoocommerceProductError::ActorMayNotIngestForShop)
            }
        }
        Principal::Anonymous => Err(IngestWoocommerceProductError::AuthenticatedActorRequired),
        Principal::Service(_) | Principal::System => {
            Err(IngestWoocommerceProductError::ActorMayNotIngestForShop)
        }
    }
}

impl From<PartnerShopReadError> for IngestWoocommerceProductError {
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

#[cfg(test)]
#[derive(Clone, Copy)]
struct MissingFxRateSnapshotFactory;

#[cfg(test)]
struct MissingFxRateSnapshotRepository;

#[cfg(test)]
impl<Tx> FxRateSnapshotRepositoryFactory<Tx> for MissingFxRateSnapshotFactory {
    fn in_transaction<'tx>(&'tx self, _tx: &'tx mut Tx) -> impl FxRateSnapshotRepository + 'tx {
        MissingFxRateSnapshotRepository
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl FxRateSnapshotRepository for MissingFxRateSnapshotRepository {
    async fn find_latest(
        &mut self,
    ) -> Result<Option<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
        Ok(None)
    }

    async fn find_latest_at_or_before(
        &mut self,
        _timestamp: time::OffsetDateTime,
    ) -> Result<Option<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
        Ok(None)
    }

    async fn find_by_id(
        &mut self,
        _id: fxrate_core::FxRateId,
    ) -> Result<Option<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
        Ok(None)
    }

    async fn find_by_ids(
        &mut self,
        _ids: &[fxrate_core::FxRateId],
    ) -> Result<Vec<fxrate_core::FxRateSnapshot>, FxRateSnapshotRepositoryError> {
        Ok(Vec::new())
    }

    async fn insert(
        &mut self,
        _snapshot: &fxrate_core::NewFxRateSnapshot,
        _source_event_id: &str,
    ) -> Result<fxrate_service::ports::FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError>
    {
        Ok(fxrate_service::ports::FxRateSnapshotInsertOutcome::Duplicate)
    }
}

#[cfg(test)]
impl<U, M, S, V, R, E, A>
    IngestWoocommerceProductHandler<U, M, S, V, R, E, A, MissingFxRateSnapshotFactory>
{
    fn new(
        unit_of_work: U,
        memberships: M,
        shops: S,
        signature_verifier: V,
        products: R,
        events: E,
        authorizer: A,
    ) -> Self {
        Self::new_with_fx_rates(
            unit_of_work,
            memberships,
            shops,
            signature_verifier,
            products,
            events,
            authorizer,
            MissingFxRateSnapshotFactory,
        )
    }
}

impl From<WoocommerceWebhookShopReadError> for IngestWoocommerceProductError {
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

fn parse_price(
    value: Option<&str>,
    currency: Option<money::Currency>,
) -> Result<Option<Price>, IngestWoocommerceProductError> {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let currency = currency.ok_or(IngestWoocommerceProductError::MissingShopCurrency)?;
    let value = value.trim();
    let (major, minor) = value.split_once('.').unwrap_or((value, ""));
    if !major.chars().all(|value| value.is_ascii_digit())
        || !minor.chars().all(|value| value.is_ascii_digit())
    {
        return Err(IngestWoocommerceProductError::InvalidPrice);
    }
    let major = major
        .parse::<u64>()
        .map_err(|_| IngestWoocommerceProductError::InvalidPrice)?;
    let mut minor = minor.chars().take(2).collect::<String>();
    while minor.len() < 2 {
        minor.push('0');
    }
    let minor = minor
        .parse::<u64>()
        .map_err(|_| IngestWoocommerceProductError::InvalidPrice)?;
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

fn product_state(status: Option<&str>, stock_status: Option<&str>) -> ProductState {
    match status {
        Some("publish") if stock_status == Some("outofstock") => ProductState::Sold,
        Some("publish") => ProductState::Available,
        Some("draft") | Some("pending") | Some("private") => ProductState::Listed,
        Some("trash") => ProductState::Removed,
        _ => ProductState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_parse_woocommerce_price_with_truncated_minor_digits() {
        let price = parse_price(Some("42.699"), Some(money::Currency::Eur));
        assert!(matches!(
            price,
            Ok(Some(value)) if value.monetary_amount == MonetaryAmount::from(4_269_u64)
        ));
    }

    #[test]
    fn should_map_woocommerce_stocked_status_to_available() {
        assert_eq!(
            ProductState::Available,
            product_state(Some("publish"), Some("instock"))
        );
        assert_eq!(
            ProductState::Sold,
            product_state(Some("publish"), Some("outofstock"))
        );
    }

    use application::operation_context::{CorrelationId, RequestId};
    use application::transaction::TransactionError;
    use domain_primitives::event_id::EventId;
    use domain_primitives::versioned::Versioned;
    use localization::Language;
    use money::Currency;
    use product_listing_core::product::ProductDomainEvent;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Default)]
    struct FakeState {
        begin_count: usize,
        commit_count: usize,
        membership_reads: usize,
        shop_reads: usize,
        signature_checks: usize,
        authorization_count: usize,
        product_lookups: usize,
        product_inserts: usize,
        event_appends: usize,
        partner: bool,
        shop: Option<WoocommerceWebhookShop>,
    }

    type SharedState = Arc<Mutex<FakeState>>;

    #[derive(Clone)]
    struct FakeUnitOfWork {
        state: SharedState,
    }

    struct FakeTx {
        state: SharedState,
    }

    #[derive(Clone, Copy)]
    struct FakeMembershipFactory;

    #[derive(Clone, Copy)]
    struct FakeShopFactory;

    #[derive(Clone, Copy)]
    struct FakeProductRepositoryFactory;

    #[derive(Clone, Copy)]
    struct FakeProductEventStoreFactory;

    #[derive(Clone, Copy)]
    struct FakeAuthorizerFactory;

    struct FakeMembershipReader {
        state: SharedState,
    }

    struct FakeShopReader {
        state: SharedState,
    }

    struct FakeSignatureVerifier {
        state: SharedState,
    }

    struct FakeProductRepository {
        state: SharedState,
    }

    struct FakeProductEventStore {
        state: SharedState,
    }

    struct FakeAuthorizer {
        state: SharedState,
    }

    fn state(shop_id: ShopId, partner: bool) -> SharedState {
        Arc::new(Mutex::new(FakeState {
            partner,
            shop: Some(WoocommerceWebhookShop {
                shop_id,
                partner_status: ShopPartnerStatus::Partnered,
                currency: Some(Currency::Eur),
                language: Some(Language::En),
            }),
            ..Default::default()
        }))
    }

    fn lock_state(state: &SharedState) -> MutexGuard<'_, FakeState> {
        match state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            lock_state(&self.state).begin_count += 1;
            Ok(FakeTx {
                state: Arc::clone(&self.state),
            })
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            lock_state(&self.state).commit_count += 1;
            Ok(())
        }
    }

    impl PartnerShopReaderFactory<FakeTx> for FakeMembershipFactory {
        fn in_transaction<'tx>(&'tx self, tx: &'tx mut FakeTx) -> impl PartnerShopReader + 'tx {
            FakeMembershipReader {
                state: Arc::clone(&tx.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl PartnerShopReader for FakeMembershipReader {
        async fn is_user_partner_of_shop(
            &mut self,
            _request: &CheckUserPartnerShopRequest,
        ) -> Result<bool, PartnerShopReadError> {
            let mut state = lock_state(&self.state);
            state.membership_reads += 1;
            Ok(state.partner)
        }

        async fn list_summaries_for_user(
            &mut self,
            _user_id: UserId,
        ) -> Result<Vec<shop_service::use_cases::ShopSummary>, PartnerShopReadError> {
            Ok(Vec::new())
        }
    }

    impl WoocommerceWebhookShopReaderFactory<FakeTx> for FakeShopFactory {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut FakeTx,
        ) -> impl WoocommerceWebhookShopReader + 'tx {
            FakeShopReader {
                state: Arc::clone(&tx.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl WoocommerceWebhookShopReader for FakeShopReader {
        async fn find_for_webhook(
            &mut self,
            _shop_id: ShopId,
        ) -> Result<Option<WoocommerceWebhookShop>, WoocommerceWebhookShopReadError> {
            let mut state = lock_state(&self.state);
            state.shop_reads += 1;
            Ok(state.shop.clone())
        }
    }

    impl WoocommerceWebhookSignatureVerifierFactory<FakeTx> for FakeShopFactory {
        fn verifier_in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut FakeTx,
        ) -> impl WoocommerceWebhookSignatureVerifier + 'tx {
            FakeSignatureVerifier {
                state: Arc::clone(&tx.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl WoocommerceWebhookSignatureVerifier for FakeSignatureVerifier {
        async fn verify(
            &mut self,
            _shop_id: ShopId,
            _body: &[u8],
            _signature: &[u8],
        ) -> Result<WoocommerceWebhookSignatureVerification, WoocommerceWebhookShopReadError>
        {
            lock_state(&self.state).signature_checks += 1;
            Ok(WoocommerceWebhookSignatureVerification::Valid)
        }
    }

    impl ProductRepositoryFactory<FakeTx> for FakeProductRepositoryFactory {
        fn in_transaction<'tx>(&'tx self, tx: &'tx mut FakeTx) -> impl ProductRepository + 'tx {
            FakeProductRepository {
                state: Arc::clone(&tx.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductRepository for FakeProductRepository {
        async fn find_by_id(
            &mut self,
            _id: ProductId,
        ) -> Result<Option<Versioned<Product, EventId>>, crate::ports::ProductRepositoryError>
        {
            Ok(None)
        }

        async fn find_by_key(
            &mut self,
            _key: &ProductKey,
        ) -> Result<Option<Versioned<Product, EventId>>, crate::ports::ProductRepositoryError>
        {
            lock_state(&self.state).product_lookups += 1;
            Ok(None)
        }

        async fn insert(
            &mut self,
            product: &Product,
            current_event_id: EventId,
        ) -> Result<Versioned<Product, EventId>, crate::ports::ProductRepositoryError> {
            lock_state(&self.state).product_inserts += 1;
            Ok(Versioned::new(product.clone(), current_event_id))
        }

        async fn update(
            &mut self,
            product: &Product,
            _expected_event_id: EventId,
            new_event_id: EventId,
        ) -> Result<Versioned<Product, EventId>, crate::ports::ProductRepositoryError> {
            Ok(Versioned::new(product.clone(), new_event_id))
        }
    }

    impl ProductEventStoreFactory<FakeTx> for FakeProductEventStoreFactory {
        fn in_transaction<'tx>(&'tx self, tx: &'tx mut FakeTx) -> impl ProductEventStore + 'tx {
            FakeProductEventStore {
                state: Arc::clone(&tx.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductEventStore for FakeProductEventStore {
        async fn append(
            &mut self,
            _event: &ProductDomainEvent,
        ) -> Result<(), crate::ports::ProductEventStoreError> {
            lock_state(&self.state).event_appends += 1;
            Ok(())
        }

        async fn find_current_event_id(
            &mut self,
            _product_id: ProductId,
        ) -> Result<Option<EventId>, crate::ports::ProductEventStoreError> {
            Ok(None)
        }
    }

    impl PartnerProductAuthorizerFactory<FakeTx> for FakeAuthorizerFactory {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut FakeTx,
        ) -> impl PartnerProductAuthorizer + 'tx {
            FakeAuthorizer {
                state: Arc::clone(&tx.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl PartnerProductAuthorizer for FakeAuthorizer {
        async fn authorize(
            &mut self,
            _actor_id: UserId,
            _shop_id: ShopId,
        ) -> Result<(), crate::ports::PartnerProductAuthorizationError> {
            lock_state(&self.state).authorization_count += 1;
            Ok(())
        }
    }

    fn handler(
        state: &SharedState,
    ) -> IngestWoocommerceProductHandler<
        FakeUnitOfWork,
        FakeMembershipFactory,
        FakeShopFactory,
        FakeShopFactory,
        FakeProductRepositoryFactory,
        FakeProductEventStoreFactory,
        FakeAuthorizerFactory,
        MissingFxRateSnapshotFactory,
    > {
        IngestWoocommerceProductHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(state),
            },
            FakeMembershipFactory,
            FakeShopFactory,
            FakeShopFactory,
            FakeProductRepositoryFactory,
            FakeProductEventStoreFactory,
            FakeAuthorizerFactory,
        )
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::User(UserId::new()),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn command(shop_id: ShopId) -> Result<IngestWoocommerceProductCommand, url::ParseError> {
        Ok(IngestWoocommerceProductCommand {
            shop_id,
            kind: WoocommerceProductEventKind::Create,
            signature: b"signature".to_vec(),
            raw_body: b"body".to_vec(),
            shops_product_id: ShopsProductId::from("product"),
            title: Some("Cabinet".to_owned()),
            permalink: Some(Url::parse("https://shop.example/products/1")?),
            description_html: Some("A cabinet".to_owned()),
            short_description_html: None,
            price: Some("42.00".to_owned()),
            status: Some("publish".to_owned()),
            stock_status: Some("instock".to_owned()),
            image_urls: IndexSet::new(),
        })
    }

    #[tokio::test]
    async fn should_validate_and_persist_in_one_transaction()
    -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = ShopId::new();
        let state = state(shop_id, true);

        let result = handler(&state)
            .execute(&context(), command(shop_id)?)
            .await?;

        assert!(matches!(
            result,
            IngestWoocommerceProductResult::Upserted(_)
        ));
        let state = lock_state(&state);
        assert_eq!(1, state.begin_count);
        assert_eq!(1, state.commit_count);
        assert_eq!(1, state.membership_reads);
        assert_eq!(1, state.shop_reads);
        assert_eq!(1, state.signature_checks);
        assert_eq!(1, state.authorization_count);
        assert_eq!(1, state.product_lookups);
        assert_eq!(1, state.product_inserts);
        assert_eq!(1, state.event_appends);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_when_partner_membership_is_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let shop_id = ShopId::new();
        let state = state(shop_id, false);

        let result = handler(&state).execute(&context(), command(shop_id)?).await;

        assert!(matches!(
            result,
            Err(IngestWoocommerceProductError::ActorMayNotIngestForShop)
        ));
        let state = lock_state(&state);
        assert_eq!(1, state.begin_count);
        assert_eq!(0, state.commit_count);
        assert_eq!(1, state.membership_reads);
        assert_eq!(0, state.shop_reads);
        assert_eq!(0, state.signature_checks);
        assert_eq!(0, state.product_lookups);
        Ok(())
    }
}
