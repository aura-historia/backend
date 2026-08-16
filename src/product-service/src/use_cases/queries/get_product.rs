use crate::ports::{
    PersonalizedProductDetailsReadModel, ProductDetailsReadError, ProductDetailsReadRequest,
    ProductDetailsReader, ProductDetailsReaderFactory,
};
use common::currency::domain::Currency;
use common::error::boxed::{BoxError, box_error};
use common::event_id::EventId;
use common::fx_rate_id::FxRateId;
use common::language::domain::Language;
use common::localized::Localized;
use common::operation_context::{OperationContext, Principal};
use common::personalized::Personalized;
use common::product_id::ProductId;
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_slug_id::ProductSlugId;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;
use fxrate_core::{FxRateSnapshot, FxRateSnapshotError, RoundingMode};
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use indexmap::IndexSet;
use notification_service::ports::product_notifications_reader::{
    ProductNotificationsReadError, ProductNotificationsReader,
};
use product_core::description::Description;
use product_core::product::{ProductAddress, ProductAuction, ProductPricing, ProductSaleValuation};
use product_core::product_image::ProductImage;
use product_core::title::Title;
use product_core::user_state::{NotificationUserState, ProductUserState};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub enum ProductLookup {
    ById(ProductId),
    BySlug {
        shop_slug_id: ShopSlugId,
        product_slug_id: ProductSlugId,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct GetProductRequest {
    pub lookup: ProductLookup,
    pub language: Language,
    pub currency: Currency,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductPricingPresentation {
    pub source: ProductPricing,
    pub display: DisplayProductPricing,
    pub valuation: ProductPricingValuation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayProductPricing {
    pub price: Option<common::price::domain::Price>,
    pub price_estimate_min: Option<common::price::domain::Price>,
    pub price_estimate_max: Option<common::price::domain::Price>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductPricingValuation {
    Current {
        fx_rate_id: FxRateId,
        captured_at: OffsetDateTime,
    },
    Sale {
        fx_rate_id: FxRateId,
        captured_at: OffsetDateTime,
        sold_at: OffsetDateTime,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProductPricingPresentationError {
    #[error("sale valuation FX snapshot does not match")]
    SaleFxSnapshotMismatch {
        expected: FxRateId,
        actual: FxRateId,
    },
    #[error("product price conversion failed")]
    PriceConversionFailed {
        #[source]
        source: FxRateSnapshotError,
    },
}

/// Converts all source prices with one immutable snapshot and records its valuation.
pub fn present_product_pricing(
    source: ProductPricing,
    sale_valuation: Option<ProductSaleValuation>,
    snapshot: &FxRateSnapshot,
    display_currency: Currency,
) -> Result<ProductPricingPresentation, ProductPricingPresentationError> {
    if let Some(sale_valuation) = sale_valuation
        && sale_valuation.fx_rate_id != snapshot.id()
    {
        return Err(ProductPricingPresentationError::SaleFxSnapshotMismatch {
            expected: sale_valuation.fx_rate_id,
            actual: snapshot.id(),
        });
    }

    let convert = |price: Option<common::price::domain::Price>| {
        price
            .map(|price| snapshot.convert(price, display_currency, RoundingMode::HalfUp))
            .transpose()
            .map_err(|source| ProductPricingPresentationError::PriceConversionFailed { source })
    };
    let display = DisplayProductPricing {
        price: convert(source.price)?,
        price_estimate_min: convert(source.price_estimate_min)?,
        price_estimate_max: convert(source.price_estimate_max)?,
    };
    let valuation = match sale_valuation {
        Some(sale_valuation) => ProductPricingValuation::Sale {
            fx_rate_id: snapshot.id(),
            captured_at: snapshot.captured_at(),
            sold_at: sale_valuation.sold_at,
        },
        None => ProductPricingValuation::Current {
            fx_rate_id: snapshot.id(),
            captured_at: snapshot.captured_at(),
        },
    };

    Ok(ProductPricingPresentation {
        source,
        display,
        valuation,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductDetailsView {
    pub product_id: ProductId,
    pub product_slug_id: ProductSlugId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub seller_name: ShopName,
    pub shop_slug_id: ShopSlugId,
    pub seller_slug_id: ShopSlugId,
    pub address: ProductAddress,
    pub product_title: Option<Localized<Language, Title>>,
    pub product_description: Option<Localized<Language, Description>>,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductPricingPresentation,
    pub state: ProductState,
    pub lifecycle: ProductLifecycle,
    pub url: Url,
    pub view_url: Url,
    pub images: IndexSet<ProductImage>,
    pub auction: ProductAuction,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

pub type PersonalizedProductDetailsView = Personalized<ProductDetailsView, ProductUserState>;

#[derive(Debug, thiserror::Error)]
pub enum GetProductError {
    #[error("product not found")]
    NotFound,
    #[error("product details query failed")]
    ProductDetailsQueryFailed,
    #[error("product details read model is invalid")]
    ProductDetailsReadModelInvalid,
    #[error("no persisted FX snapshot is available for product pricing")]
    PricingFxSnapshotMissing,
    #[error("FX snapshot lookup is temporarily unavailable for product pricing")]
    PricingFxSnapshotUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("persisted FX snapshot is invalid for product pricing")]
    PricingFxSnapshotInvalid {
        #[source]
        source: BoxError,
    },
    #[error("sale valuation FX snapshot does not match")]
    SaleFxSnapshotMismatch {
        expected: FxRateId,
        actual: FxRateId,
    },
    #[error("product price conversion failed")]
    ProductPriceConversionFailed {
        #[source]
        source: FxRateSnapshotError,
    },
    #[error("product notification read failed")]
    ProductNotificationReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin get product transaction")]
    BeginTransactionFailed,
    #[error("failed to commit get product transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait GetProductUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetProductRequest,
    ) -> Result<PersonalizedProductDetailsView, GetProductError>;
}

pub struct GetProductHandler<U, D, F, N> {
    unit_of_work: U,
    details_reader: D,
    fx_rates: F,
    product_notifications: N,
}

impl<U, D, F, N> GetProductHandler<U, D, F, N> {
    pub fn new(unit_of_work: U, details_reader: D, fx_rates: F, product_notifications: N) -> Self {
        Self {
            unit_of_work,
            details_reader,
            fx_rates,
            product_notifications,
        }
    }
}

#[async_trait::async_trait]
impl<U, D, F, N> GetProductUseCase for GetProductHandler<U, D, F, N>
where
    U: UnitOfWork,
    D: ProductDetailsReaderFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
    N: ProductNotificationsReader,
{
    #[tracing::instrument(
        name = "get_product",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetProductRequest,
    ) -> Result<PersonalizedProductDetailsView, GetProductError> {
        let user_id = personalization_user_id(&context.principal);
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetProductError::BeginTransactionFailed)?;
        let factual_details = self
            .details_reader
            .in_transaction(&mut tx)
            .find_details(&ProductDetailsReadRequest {
                lookup: request.lookup,
                language: request.language,
                user_id,
            })
            .await?
            .ok_or(GetProductError::NotFound)?;
        let snapshot =
            pricing_snapshot(&self.fx_rates, &mut tx, factual_details.item.sale_valuation).await?;
        let mut details = present_product_details(factual_details, &snapshot, request.currency)?;

        tx.commit()
            .await
            .map_err(|_| GetProductError::CommitTransactionFailed)?;

        if let Some(user_id) = user_id {
            let user_state = details
                .user_state
                .as_mut()
                .ok_or(GetProductError::ProductDetailsReadModelInvalid)?;
            let notification = self
                .product_notifications
                .list_by_product(&user_id, &details.item.product_id, Some(1), true)
                .await
                .map_err(product_notification_read_error)?
                .into_iter()
                .next()
                .map(|notification| NotificationUserState {
                    seen: notification.seen,
                    origin_event_id: Some(notification.origin_event_id),
                })
                .unwrap_or_default();
            user_state.notification = notification;

            if user_state.search_filter.hidden {
                redact_hidden_product(&mut details.item)?;
            }
        }

        Ok(details)
    }
}

async fn pricing_snapshot<Tx, F>(
    fx_rates: &F,
    tx: &mut Tx,
    sale_valuation: Option<ProductSaleValuation>,
) -> Result<FxRateSnapshot, GetProductError>
where
    F: FxRateSnapshotRepositoryFactory<Tx>,
{
    let mut repository = fx_rates.in_transaction(tx);
    let snapshot = match sale_valuation {
        Some(sale_valuation) => repository.find_by_id(sale_valuation.fx_rate_id).await?,
        None => repository.find_latest().await?,
    };
    snapshot.ok_or(GetProductError::PricingFxSnapshotMissing)
}

pub fn present_product_details(
    factual_details: PersonalizedProductDetailsReadModel,
    snapshot: &FxRateSnapshot,
    currency: Currency,
) -> Result<PersonalizedProductDetailsView, ProductPricingPresentationError> {
    let Personalized { item, user_state } = factual_details;
    let pricing = present_product_pricing(item.pricing, item.sale_valuation, snapshot, currency)?;
    Ok(Personalized {
        item: ProductDetailsView {
            product_id: item.product_id,
            product_slug_id: item.product_slug_id,
            event_id: item.event_id,
            shop_id: item.shop_id,
            seller_id: item.seller_id,
            shops_product_id: item.shops_product_id,
            shop_name: item.shop_name,
            seller_name: item.seller_name,
            shop_slug_id: item.shop_slug_id,
            seller_slug_id: item.seller_slug_id,
            address: item.address,
            product_title: item.product_title,
            product_description: item.product_description,
            title: item.title,
            description: item.description,
            pricing,
            state: item.state,
            lifecycle: item.lifecycle,
            url: item.url,
            view_url: item.view_url,
            images: item.images,
            auction: item.auction,
            created: item.created,
            updated: item.updated,
        },
        user_state,
    })
}

fn personalization_user_id(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}

fn product_notification_read_error(error: ProductNotificationsReadError) -> GetProductError {
    GetProductError::ProductNotificationReadFailed {
        source: box_error(error),
    }
}

pub fn redact_hidden_product(details: &mut ProductDetailsView) -> Result<(), GetProductError> {
    let nil = uuid::Uuid::nil();
    let language = details
        .title
        .as_ref()
        .map(|title| title.localization)
        .unwrap_or(Language::En);
    let hidden_url = Url::parse("https://aura-historia.com/pricing")
        .map_err(|_| GetProductError::ProductDetailsReadModelInvalid)?;

    details.product_id = ProductId::from(nil);
    details.product_slug_id = ProductSlugId::from("Hidden");
    details.event_id = EventId::from(nil);
    details.shop_id = ShopId::from(nil);
    details.seller_id = ShopId::from(nil);
    details.shops_product_id = ShopsProductId::from(nil.to_string());
    details.shop_name = ShopName::from("Hidden");
    details.seller_name = ShopName::from("Hidden");
    details.shop_slug_id = ShopSlugId::from("Hidden");
    details.seller_slug_id = ShopSlugId::from("Hidden");
    details.address = ProductAddress::default();
    details.product_title = None;
    details.product_description = None;
    details.title = Some(Localized::new(language, hidden_title(language)));
    details.description = None;
    details.pricing = ProductPricingPresentation {
        source: ProductPricing::default(),
        display: DisplayProductPricing {
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
        },
        valuation: ProductPricingValuation::Current {
            fx_rate_id: FxRateId::from(nil),
            captured_at: OffsetDateTime::UNIX_EPOCH,
        },
    };
    details.state = ProductState::Unknown;
    details.url = hidden_url.clone();
    details.view_url = hidden_url;
    details.images = IndexSet::new();
    details.auction = ProductAuction::default();
    details.created = OffsetDateTime::UNIX_EPOCH;
    details.updated = OffsetDateTime::UNIX_EPOCH;

    Ok(())
}

fn hidden_title(language: Language) -> Title {
    match language {
        Language::De => Title::from("Versteckter Produkttitel"),
        Language::En => Title::from("Hidden Product Title"),
        Language::Fr => Title::from("Titre du produit masqué"),
        Language::Es => Title::from("Título de producto oculto"),
        Language::It => Title::from("Titolo del prodotto mascherato"),
        _ => Title::from("Hidden Product Title"),
    }
}

impl From<ProductDetailsReadError> for GetProductError {
    fn from(error: ProductDetailsReadError) -> Self {
        match error {
            ProductDetailsReadError::ProductDetailsQueryFailed => Self::ProductDetailsQueryFailed,
            ProductDetailsReadError::ProductDetailsReadModelInvalid => {
                Self::ProductDetailsReadModelInvalid
            }
        }
    }
}

impl From<FxRateSnapshotRepositoryError> for GetProductError {
    fn from(error: FxRateSnapshotRepositoryError) -> Self {
        match error {
            FxRateSnapshotRepositoryError::InsertFailed { source }
            | FxRateSnapshotRepositoryError::ReadFailed { source } => {
                Self::PricingFxSnapshotUnavailable { source }
            }
            FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
                Self::PricingFxSnapshotInvalid { source }
            }
        }
    }
}

impl From<ProductPricingPresentationError> for GetProductError {
    fn from(error: ProductPricingPresentationError) -> Self {
        match error {
            ProductPricingPresentationError::SaleFxSnapshotMismatch { expected, actual } => {
                Self::SaleFxSnapshotMismatch { expected, actual }
            }
            ProductPricingPresentationError::PriceConversionFailed { source } => {
                Self::ProductPriceConversionFailed { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::ProductDetailsReadModel;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use common::price::domain::{MonetaryAmount, Price};
    use common::transaction::TransactionError;
    use fxrate_core::{
        FX_RATE_SCALE, FxRateGeneration, FxRateQuote, FxRateSource, NewFxRateSnapshot,
    };
    use notification_core::notification::{
        NotificationPartnerApplicationPayload, NotificationPayload,
    };
    use notification_core::notification_id::NotificationId;
    use notification_service::ports::product_notifications_reader::ProductNotificationReadItem;
    use std::sync::{Arc, Mutex, MutexGuard};
    use strum::IntoEnumIterator;

    #[derive(Debug, Default)]
    struct FakeState {
        begin_error: bool,
        commit_error: bool,
        find_details_result:
            Option<Result<Option<PersonalizedProductDetailsReadModel>, ProductDetailsReadError>>,
        find_details_request: Option<ProductDetailsReadRequest>,
        latest_snapshot_result:
            Option<Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError>>,
        snapshot_by_id_result:
            Option<Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError>>,
        fx_rate_id_requests: Vec<FxRateId>,
        latest_snapshot_count: usize,
        notification_result:
            Option<Result<Vec<ProductNotificationReadItem>, ProductNotificationsReadError>>,
        notification_requests: Vec<(UserId, ProductId, Option<i32>, bool)>,
        notification_called_after_commit: Option<bool>,
        commit_count: usize,
    }

    type SharedState = Arc<Mutex<FakeState>>;

    #[derive(Clone)]
    struct FakeUnitOfWork {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeDetailsReaderFactory {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeFxRateSnapshotRepositoryFactory {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeProductNotificationsReader {
        state: SharedState,
    }

    struct FakeTx {
        state: SharedState,
    }

    struct FakeDetailsReader {
        state: SharedState,
    }

    struct FakeFxRateSnapshotRepository {
        state: SharedState,
    }

    fn state() -> SharedState {
        Arc::new(Mutex::new(FakeState::default()))
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
            if lock_state(&self.state).begin_error {
                Err(TransactionError::BeginFailed)
            } else {
                Ok(FakeTx {
                    state: Arc::clone(&self.state),
                })
            }
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock_state(&self.state);
            state.commit_count += 1;
            if state.commit_error {
                Err(TransactionError::CommitFailed)
            } else {
                Ok(())
            }
        }
    }

    impl ProductDetailsReaderFactory<FakeTx> for FakeDetailsReaderFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl ProductDetailsReader + 'tx {
            FakeDetailsReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    impl FxRateSnapshotRepositoryFactory<FakeTx> for FakeFxRateSnapshotRepositoryFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl FxRateSnapshotRepository + 'tx {
            FakeFxRateSnapshotRepository {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductDetailsReader for FakeDetailsReader {
        async fn find_details(
            &mut self,
            request: &ProductDetailsReadRequest,
        ) -> Result<Option<PersonalizedProductDetailsReadModel>, ProductDetailsReadError> {
            let mut state = lock_state(&self.state);
            state.find_details_request = Some(request.clone());
            match state.find_details_result.take() {
                Some(result) => result,
                None => Ok(None),
            }
        }
    }

    #[async_trait::async_trait]
    impl FxRateSnapshotRepository for FakeFxRateSnapshotRepository {
        async fn find_latest(
            &mut self,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state = lock_state(&self.state);
            state.latest_snapshot_count += 1;
            match state.latest_snapshot_result.take() {
                Some(result) => result,
                None => Ok(None),
            }
        }

        async fn find_latest_at_or_before(
            &mut self,
            _timestamp: OffsetDateTime,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_by_id(
            &mut self,
            id: FxRateId,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state = lock_state(&self.state);
            state.fx_rate_id_requests.push(id);
            match state.snapshot_by_id_result.take() {
                Some(result) => result,
                None => Ok(None),
            }
        }

        async fn find_by_ids(
            &mut self,
            _ids: &[FxRateId],
        ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(Vec::new())
        }

        async fn insert(
            &mut self,
            _snapshot: &fxrate_core::NewFxRateSnapshot,
            _source_event_id: &str,
        ) -> Result<fxrate_service::ports::FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError>
        {
            Err(FxRateSnapshotRepositoryError::ReadFailed {
                source: box_error(std::io::Error::other(
                    "not implemented in detail reader fake",
                )),
            })
        }
    }

    #[async_trait::async_trait]
    impl ProductNotificationsReader for FakeProductNotificationsReader {
        async fn list_by_product(
            &self,
            user_id: &UserId,
            product_id: &ProductId,
            limit: Option<i32>,
            newest_first: bool,
        ) -> Result<Vec<ProductNotificationReadItem>, ProductNotificationsReadError> {
            let mut state = lock_state(&self.state);
            state
                .notification_requests
                .push((*user_id, *product_id, limit, newest_first));
            state.notification_called_after_commit = Some(state.commit_count == 1);
            match state.notification_result.take() {
                Some(result) => result,
                None => Ok(Vec::new()),
            }
        }
    }

    fn handler(
        state: &SharedState,
    ) -> GetProductHandler<
        FakeUnitOfWork,
        FakeDetailsReaderFactory,
        FakeFxRateSnapshotRepositoryFactory,
        FakeProductNotificationsReader,
    > {
        GetProductHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(state),
            },
            FakeDetailsReaderFactory {
                state: Arc::clone(state),
            },
            FakeFxRateSnapshotRepositoryFactory {
                state: Arc::clone(state),
            },
            FakeProductNotificationsReader {
                state: Arc::clone(state),
            },
        )
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn request(language: Language, currency: Currency) -> GetProductRequest {
        GetProductRequest {
            lookup: ProductLookup::ById(ProductId::new()),
            language,
            currency,
        }
    }

    fn url(value: &str) -> Result<Url, url::ParseError> {
        Url::parse(value)
    }

    fn snapshot() -> Result<FxRateSnapshot, FxRateSnapshotError> {
        let captured = NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| {
                FxRateQuote::new(
                    currency,
                    if currency == Currency::Eur {
                        FX_RATE_SCALE
                    } else {
                        1_250_000
                    },
                )
            }),
        )?;
        Ok(captured.into_persisted(FxRateGeneration::try_from(1)?))
    }

    fn factual_details() -> Result<PersonalizedProductDetailsReadModel, url::ParseError> {
        Ok(Personalized {
            item: ProductDetailsReadModel {
                product_id: ProductId::new(),
                product_slug_id: ProductSlugId::from("cabinet-abcdef"),
                event_id: EventId::new(),
                shop_id: ShopId::new(),
                seller_id: ShopId::new(),
                shops_product_id: ShopsProductId::new(),
                shop_name: ShopName::from("Shop"),
                seller_name: ShopName::from("Seller"),
                shop_slug_id: ShopSlugId::from("shop"),
                seller_slug_id: ShopSlugId::from("seller"),
                address: ProductAddress::default(),
                product_title: Some(Localized::new(Language::En, Title::from("Cabinet"))),
                product_description: Some(Localized::new(
                    Language::En,
                    Description::from("Native"),
                )),
                title: Some(Localized::new(Language::En, Title::from("Cabinet"))),
                description: Some(Localized::new(
                    Language::En,
                    Description::from("Description"),
                )),
                pricing: ProductPricing {
                    price: Some(Price::new(MonetaryAmount::from(100_u64), Currency::Eur)),
                    price_estimate_min: Some(Price::new(
                        MonetaryAmount::from(80_u64),
                        Currency::Eur,
                    )),
                    price_estimate_max: Some(Price::new(
                        MonetaryAmount::from(120_u64),
                        Currency::Eur,
                    )),
                },
                sale_valuation: None,
                state: ProductState::Listed,
                lifecycle: ProductLifecycle::Active,
                url: url("https://shop.example/products/1")?,
                view_url: url("https://aura.example/products/cabinet-abcdef")?,
                images: IndexSet::<ProductImage>::new(),
                auction: ProductAuction::default(),
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            },
            user_state: None,
        })
    }

    fn prepare_current_snapshot(state: &SharedState) -> Result<(), FxRateSnapshotError> {
        lock_state(state).latest_snapshot_result = Some(Ok(Some(snapshot()?)));
        Ok(())
    }

    fn notification_item(
        user_id: UserId,
        origin_event_id: EventId,
        seen: bool,
    ) -> ProductNotificationReadItem {
        ProductNotificationReadItem {
            user_id,
            origin_event_id,
            notification_id: NotificationId::new(),
            notification_type: None,
            notification_payload: NotificationPayload::PartnerApplication {
                shop_name: ShopName::from("Shop"),
                image: None,
                partner_application_payload: NotificationPartnerApplicationPayload::Approved {
                    partner_application_id:
                        common::partner_shop_application_id::PartnerShopApplicationId::new(),
                },
            },
            seen,
            external: false,
        }
    }

    #[test]
    fn should_present_all_prices_with_half_up_conversion_and_current_valuation()
    -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = snapshot()?;
        let source = ProductPricing {
            price: Some(Price::new(MonetaryAmount::from(1_u64), Currency::Eur)),
            price_estimate_min: Some(Price::new(MonetaryAmount::from(2_u64), Currency::Eur)),
            price_estimate_max: Some(Price::new(MonetaryAmount::from(3_u64), Currency::Eur)),
        };

        let presentation = present_product_pricing(source, None, &snapshot, Currency::Usd)?;

        assert_eq!(source, presentation.source);
        assert_eq!(
            DisplayProductPricing {
                price: Some(Price::new(MonetaryAmount::from(1_u64), Currency::Usd)),
                price_estimate_min: Some(Price::new(MonetaryAmount::from(3_u64), Currency::Usd)),
                price_estimate_max: Some(Price::new(MonetaryAmount::from(4_u64), Currency::Usd)),
            },
            presentation.display
        );
        assert_eq!(
            ProductPricingValuation::Current {
                fx_rate_id: snapshot.id(),
                captured_at: snapshot.captured_at(),
            },
            presentation.valuation
        );
        Ok(())
    }

    #[test]
    fn should_reject_sale_valuation_with_a_different_snapshot()
    -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = snapshot()?;
        let expected = FxRateId::new();

        let result = present_product_pricing(
            ProductPricing::default(),
            Some(ProductSaleValuation {
                fx_rate_id: expected,
                sold_at: OffsetDateTime::UNIX_EPOCH,
            }),
            &snapshot,
            Currency::Eur,
        );

        assert!(matches!(
            result,
            Err(ProductPricingPresentationError::SaleFxSnapshotMismatch { actual, .. })
                if actual == snapshot.id()
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_present_current_pricing_from_latest_snapshot_and_commit_before_enrichment()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let details = factual_details()?;
        let product_id = details.item.product_id;
        lock_state(&state).find_details_result = Some(Ok(Some(details)));
        prepare_current_snapshot(&state)?;
        let request = request(Language::De, Currency::Usd);

        let result = handler(&state)
            .execute(&context(Principal::Anonymous), request.clone())
            .await?;

        assert_eq!(
            Some(Price::new(MonetaryAmount::from(125_u64), Currency::Usd)),
            result.item.pricing.display.price
        );
        assert_eq!(1, lock_state(&state).commit_count);
        let state = lock_state(&state);
        assert_eq!(1, state.latest_snapshot_count);
        assert!(state.fx_rate_id_requests.is_empty());
        assert!(state.notification_requests.is_empty());
        assert_eq!(
            Some(ProductDetailsReadRequest {
                lookup: request.lookup,
                language: Language::De,
                user_id: None,
            }),
            state.find_details_request
        );
        assert_eq!(product_id, result.item.product_id);
        Ok(())
    }

    #[tokio::test]
    async fn should_load_sale_snapshot_by_id_and_preserve_sale_timestamp()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let mut details = factual_details()?;
        let snapshot = snapshot()?;
        let sold_at = OffsetDateTime::UNIX_EPOCH + time::Duration::days(1);
        details.item.sale_valuation = Some(ProductSaleValuation {
            fx_rate_id: snapshot.id(),
            sold_at,
        });
        lock_state(&state).find_details_result = Some(Ok(Some(details)));
        lock_state(&state).snapshot_by_id_result = Some(Ok(Some(snapshot.clone())));

        let result = handler(&state)
            .execute(
                &context(Principal::Anonymous),
                request(Language::En, Currency::Eur),
            )
            .await?;

        assert_eq!(
            ProductPricingValuation::Sale {
                fx_rate_id: snapshot.id(),
                captured_at: snapshot.captured_at(),
                sold_at,
            },
            result.item.pricing.valuation
        );
        let state = lock_state(&state);
        assert_eq!(vec![snapshot.id()], state.fx_rate_id_requests);
        assert_eq!(0, state.latest_snapshot_count);
        assert_eq!(1, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_hydrate_newest_notification_after_commit_for_authenticated_user()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let user_id = UserId::new();
        let mut details = factual_details()?;
        details.user_state = Some(ProductUserState::default());
        let product_id = details.item.product_id;
        let newest_event_id = EventId::new();
        lock_state(&state).find_details_result = Some(Ok(Some(details)));
        prepare_current_snapshot(&state)?;
        lock_state(&state).notification_result =
            Some(Ok(vec![notification_item(user_id, newest_event_id, false)]));

        let result = handler(&state)
            .execute(
                &context(Principal::User(user_id)),
                request(Language::En, Currency::Eur),
            )
            .await?;

        let user_state = result.user_state.unwrap_or_default();
        assert!(!user_state.notification.seen);
        assert_eq!(
            Some(newest_event_id),
            user_state.notification.origin_event_id
        );
        let state = lock_state(&state);
        assert_eq!(Some(true), state.notification_called_after_commit);
        assert_eq!(
            vec![(user_id, product_id, Some(1), true)],
            state.notification_requests
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_redact_hidden_product_after_notification_enrichment()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let user_id = UserId::new();
        let mut details = factual_details()?;
        let lifecycle = details.item.lifecycle;
        let event_id = EventId::new();
        let mut user_state = ProductUserState::default();
        user_state.search_filter.hidden = true;
        details.user_state = Some(user_state);
        lock_state(&state).find_details_result = Some(Ok(Some(details)));
        prepare_current_snapshot(&state)?;
        lock_state(&state).notification_result =
            Some(Ok(vec![notification_item(user_id, event_id, false)]));

        let result = handler(&state)
            .execute(
                &context(Principal::User(user_id)),
                request(Language::En, Currency::Eur),
            )
            .await?;

        assert_eq!(ProductId::from(uuid::Uuid::nil()), result.item.product_id);
        assert_eq!(ProductState::Unknown, result.item.state);
        assert_eq!(lifecycle, result.item.lifecycle);
        assert_eq!(ProductPricing::default(), result.item.pricing.source);
        assert_eq!(
            DisplayProductPricing {
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
            },
            result.item.pricing.display
        );
        assert!(result.user_state.unwrap_or_default().search_filter.hidden);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_or_enrich_when_pricing_snapshot_is_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        lock_state(&state).find_details_result = Some(Ok(Some(factual_details()?)));
        lock_state(&state).latest_snapshot_result = Some(Ok(None));

        let result = handler(&state)
            .execute(
                &context(Principal::Anonymous),
                request(Language::En, Currency::Eur),
            )
            .await;

        assert!(matches!(
            result,
            Err(GetProductError::PricingFxSnapshotMissing)
        ));
        let state = lock_state(&state);
        assert_eq!(0, state.commit_count);
        assert!(state.notification_requests.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_when_sale_snapshot_does_not_match_valuation()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let mut details = factual_details()?;
        details.item.sale_valuation = Some(ProductSaleValuation {
            fx_rate_id: FxRateId::new(),
            sold_at: OffsetDateTime::UNIX_EPOCH,
        });
        lock_state(&state).find_details_result = Some(Ok(Some(details)));
        lock_state(&state).snapshot_by_id_result = Some(Ok(Some(snapshot()?)));

        let result = handler(&state)
            .execute(
                &context(Principal::Anonymous),
                request(Language::En, Currency::Eur),
            )
            .await;

        assert!(matches!(
            result,
            Err(GetProductError::SaleFxSnapshotMismatch { .. })
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_fx_snapshot_read_failure_without_commit()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        lock_state(&state).find_details_result = Some(Ok(Some(factual_details()?)));
        lock_state(&state).latest_snapshot_result =
            Some(Err(FxRateSnapshotRepositoryError::ReadFailed {
                source: box_error(std::io::Error::other("database unavailable")),
            }));

        let result = handler(&state)
            .execute(
                &context(Principal::Anonymous),
                request(Language::En, Currency::Eur),
            )
            .await;

        assert!(matches!(
            result,
            Err(GetProductError::PricingFxSnapshotUnavailable { .. })
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_not_found_without_snapshot_lookup_or_commit() {
        let state = state();

        let result = handler(&state)
            .execute(
                &context(Principal::Anonymous),
                request(Language::En, Currency::Eur),
            )
            .await;

        assert!(matches!(result, Err(GetProductError::NotFound)));
        let state = lock_state(&state);
        assert_eq!(0, state.commit_count);
        assert_eq!(0, state.latest_snapshot_count);
        assert!(state.fx_rate_id_requests.is_empty());
    }

    #[test]
    fn should_only_personalize_user_principals() {
        let user_id = UserId::new();

        assert_eq!(None, personalization_user_id(&Principal::Anonymous));
        assert_eq!(
            Some(user_id),
            personalization_user_id(&Principal::User(user_id))
        );
        assert_eq!(
            Some(user_id),
            personalization_user_id(&Principal::DelegatedUser {
                user_id,
                capabilities: Default::default(),
            })
        );
        assert_eq!(
            None,
            personalization_user_id(&Principal::Service("service".to_owned()))
        );
        assert_eq!(None, personalization_user_id(&Principal::System));
    }
}
