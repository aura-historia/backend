use crate::ports::{
    PersonalizedProductListingDetailsReadModel, ProductListingDetailsReadError,
    ProductListingDetailsReadRequest, ProductListingDetailsReader,
    ProductListingDetailsReaderFactory,
};
use application::error::BoxError;
use application::operation_context::{OperationContext, Principal};
use application::personalized::Personalized;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use fxrate_core::{FxRateId, FxRateSnapshot, FxRateSnapshotError, RoundingMode};
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use indexmap::IndexSet;
use localization::{Language, Localized};
use money::Currency;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_lifecycle::ListingLifecycle;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;

use product_listing_core::shop_listing_id::ShopListingId;
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_slug_id::ShopSlugId;
use user_core::user_id::UserId;

use crate::user_state::ProductListingUserState;
use product_listing_core::description::Description;
use product_listing_core::product_listing::{
    ProductListingAddress, ProductListingAuction, ProductListingPricing, ProductSaleValuation,
};
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::title::Title;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingLookup {
    ById(ProductListingId),
    BySlug {
        shop_slug_id: ShopSlugId,
        product_listing_slug_id: ProductListingSlugId,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct GetProductListingRequest {
    pub lookup: ProductListingLookup,
    pub language: Language,
    pub currency: Currency,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingPricingPresentation {
    pub source: ProductListingPricing,
    pub display: DisplayProductListingPricing,
    pub valuation: ProductListingPricingValuation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayProductListingPricing {
    pub price: Option<money::Price>,
    pub price_estimate_min: Option<money::Price>,
    pub price_estimate_max: Option<money::Price>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingPricingValuation {
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
pub enum ProductListingPricingPresentationError {
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
    source: ProductListingPricing,
    sale_valuation: Option<ProductSaleValuation>,
    snapshot: &FxRateSnapshot,
    display_currency: Currency,
) -> Result<ProductListingPricingPresentation, ProductListingPricingPresentationError> {
    if let Some(sale_valuation) = sale_valuation
        && sale_valuation.fx_rate_id != snapshot.id()
    {
        return Err(
            ProductListingPricingPresentationError::SaleFxSnapshotMismatch {
                expected: sale_valuation.fx_rate_id,
                actual: snapshot.id(),
            },
        );
    }

    let convert = |price: Option<money::Price>| {
        price
            .map(|price| snapshot.convert(price, display_currency, RoundingMode::HalfUp))
            .transpose()
            .map_err(
                |source| ProductListingPricingPresentationError::PriceConversionFailed { source },
            )
    };
    let display = DisplayProductListingPricing {
        price: convert(source.price)?,
        price_estimate_min: convert(source.price_estimate_min)?,
        price_estimate_max: convert(source.price_estimate_max)?,
    };
    let valuation = match sale_valuation {
        Some(sale_valuation) => ProductListingPricingValuation::Sale {
            fx_rate_id: snapshot.id(),
            captured_at: snapshot.captured_at(),
            sold_at: sale_valuation.sold_at,
        },
        None => ProductListingPricingValuation::Current {
            fx_rate_id: snapshot.id(),
            captured_at: snapshot.captured_at(),
        },
    };

    Ok(ProductListingPricingPresentation {
        source,
        display,
        valuation,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingDetailsView {
    pub product_listing_id: ProductListingId,
    pub product_listing_slug_id: ProductListingSlugId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shop_listing_id: ShopListingId,
    pub shop_name: ShopName,
    pub seller_name: ShopName,
    pub shop_slug_id: ShopSlugId,
    pub seller_slug_id: ShopSlugId,
    pub address: ProductListingAddress,
    pub product_title: Option<Localized<Language, Title>>,
    pub product_description: Option<Localized<Language, Description>>,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductListingPricingPresentation,
    pub availability: Option<ListingAvailability>,
    pub lifecycle: ListingLifecycle,
    pub url: Url,
    pub view_url: Url,
    pub images: IndexSet<ProductListingImage>,
    pub auction: ProductListingAuction,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

pub type PersonalizedProductListingDetailsView =
    Personalized<ProductListingDetailsView, ProductListingUserState>;

#[derive(Debug, thiserror::Error)]
pub enum GetProductListingError {
    #[error("product not found")]
    NotFound,
    #[error("product details query failed")]
    ProductListingDetailsQueryFailed,
    #[error("product details read model is invalid")]
    ProductListingDetailsReadModelInvalid,
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
    ProductListingPriceConversionFailed {
        #[source]
        source: FxRateSnapshotError,
    },

    #[error("failed to begin get product transaction")]
    BeginTransactionFailed,
    #[error("failed to commit get product transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait GetProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetProductListingRequest,
    ) -> Result<PersonalizedProductListingDetailsView, GetProductListingError>;
}

pub struct GetProductListingHandler<U, D, F> {
    unit_of_work: U,
    details_reader: D,
    fx_rates: F,
}

impl<U, D, F> GetProductListingHandler<U, D, F> {
    pub fn new(unit_of_work: U, details_reader: D, fx_rates: F) -> Self {
        Self {
            unit_of_work,
            details_reader,
            fx_rates,
        }
    }
}

#[async_trait::async_trait]
impl<U, D, F> GetProductListingUseCase for GetProductListingHandler<U, D, F>
where
    U: UnitOfWork,
    D: ProductListingDetailsReaderFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
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
        request: GetProductListingRequest,
    ) -> Result<PersonalizedProductListingDetailsView, GetProductListingError> {
        let user_id = personalization_user_id(&context.principal);
        let valuation_at = OffsetDateTime::now_utc();
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetProductListingError::BeginTransactionFailed)?;
        let factual_details = self
            .details_reader
            .in_transaction(&mut tx)
            .find_details(&ProductListingDetailsReadRequest {
                lookup: request.lookup,
                language: request.language,
                user_id,
            })
            .await?
            .ok_or(GetProductListingError::NotFound)?;
        let snapshot = pricing_snapshot(
            &self.fx_rates,
            &mut tx,
            factual_details.item.sale_valuation,
            valuation_at,
        )
        .await?;
        let mut details = present_product_details(factual_details, &snapshot, request.currency)?;

        tx.commit()
            .await
            .map_err(|_| GetProductListingError::CommitTransactionFailed)?;

        if user_id.is_some()
            && details
                .user_state
                .as_ref()
                .ok_or(GetProductListingError::ProductListingDetailsReadModelInvalid)?
                .search_filter
                .hidden
        {
            redact_hidden_product(&mut details.item)?;
        }

        Ok(details)
    }
}

async fn pricing_snapshot<Tx, F>(
    fx_rates: &F,
    tx: &mut Tx,
    sale_valuation: Option<ProductSaleValuation>,
    valuation_at: OffsetDateTime,
) -> Result<FxRateSnapshot, GetProductListingError>
where
    F: FxRateSnapshotRepositoryFactory<Tx>,
{
    let mut repository = fx_rates.in_transaction(tx);
    let snapshot = match sale_valuation {
        Some(sale_valuation) => repository.find_by_id(sale_valuation.fx_rate_id).await?,
        None => repository.find_latest_at_or_before(valuation_at).await?,
    };
    snapshot.ok_or(GetProductListingError::PricingFxSnapshotMissing)
}

pub fn present_product_details(
    factual_details: PersonalizedProductListingDetailsReadModel,
    snapshot: &FxRateSnapshot,
    currency: Currency,
) -> Result<PersonalizedProductListingDetailsView, ProductListingPricingPresentationError> {
    let Personalized { item, user_state } = factual_details;
    let pricing = present_product_pricing(item.pricing, item.sale_valuation, snapshot, currency)?;
    Ok(Personalized {
        item: ProductListingDetailsView {
            product_listing_id: item.product_listing_id,
            product_listing_slug_id: item.product_listing_slug_id,
            event_id: item.event_id,
            shop_id: item.shop_id,
            seller_id: item.seller_id,
            shop_listing_id: item.shop_listing_id,
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
            availability: item.availability,
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

pub fn redact_hidden_product(
    details: &mut ProductListingDetailsView,
) -> Result<(), GetProductListingError> {
    let nil = uuid::Uuid::nil();
    let language = details
        .title
        .as_ref()
        .map(|title| title.localization)
        .unwrap_or(Language::En);
    let hidden_url = Url::parse("https://aura-historia.com/pricing")
        .map_err(|_| GetProductListingError::ProductListingDetailsReadModelInvalid)?;

    details.product_listing_id = ProductListingId::from(nil);
    details.product_listing_slug_id = ProductListingSlugId::from("Hidden");
    details.event_id = EventId::from(nil);
    details.shop_id = ShopId::from(nil);
    details.seller_id = ShopId::from(nil);
    details.shop_listing_id = ShopListingId::from(nil.to_string());
    details.shop_name = ShopName::from("Hidden");
    details.seller_name = ShopName::from("Hidden");
    details.shop_slug_id = ShopSlugId::from("Hidden");
    details.seller_slug_id = ShopSlugId::from("Hidden");
    details.address = ProductListingAddress::default();
    details.product_title = None;
    details.product_description = None;
    details.title = Some(Localized::new(language, hidden_title(language)));
    details.description = None;
    details.pricing = ProductListingPricingPresentation {
        source: ProductListingPricing::default(),
        display: DisplayProductListingPricing {
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
        },
        valuation: ProductListingPricingValuation::Current {
            fx_rate_id: FxRateId::from(nil),
            captured_at: OffsetDateTime::UNIX_EPOCH,
        },
    };
    details.availability = None;
    details.url = hidden_url.clone();
    details.view_url = hidden_url;
    details.images = IndexSet::new();
    details.auction = ProductListingAuction::default();
    details.created = OffsetDateTime::UNIX_EPOCH;
    details.updated = OffsetDateTime::UNIX_EPOCH;

    Ok(())
}

fn hidden_title(language: Language) -> Title {
    match language {
        Language::De => Title::from("Versteckter Produkttitel"),
        Language::En => Title::from("Hidden ProductListing Title"),
        Language::Fr => Title::from("Titre du produit masqué"),
        Language::Es => Title::from("Título de producto oculto"),
        Language::It => Title::from("Titolo del prodotto mascherato"),
        _ => Title::from("Hidden ProductListing Title"),
    }
}

impl From<ProductListingDetailsReadError> for GetProductListingError {
    fn from(error: ProductListingDetailsReadError) -> Self {
        match error {
            ProductListingDetailsReadError::ProductListingDetailsQueryFailed => {
                Self::ProductListingDetailsQueryFailed
            }
            ProductListingDetailsReadError::ProductListingDetailsReadModelInvalid => {
                Self::ProductListingDetailsReadModelInvalid
            }
        }
    }
}

impl From<FxRateSnapshotRepositoryError> for GetProductListingError {
    fn from(error: FxRateSnapshotRepositoryError) -> Self {
        match error {
            FxRateSnapshotRepositoryError::InsertFailed { source }
            | FxRateSnapshotRepositoryError::ReadFailed { source } => {
                Self::PricingFxSnapshotUnavailable { source }
            }
            FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
                Self::PricingFxSnapshotInvalid { source }
            }
            FxRateSnapshotRepositoryError::CapturedAtNotMonotonic => Self::PricingFxSnapshotMissing,
        }
    }
}

impl From<ProductListingPricingPresentationError> for GetProductListingError {
    fn from(error: ProductListingPricingPresentationError) -> Self {
        match error {
            ProductListingPricingPresentationError::SaleFxSnapshotMismatch { expected, actual } => {
                Self::SaleFxSnapshotMismatch { expected, actual }
            }
            ProductListingPricingPresentationError::PriceConversionFailed { source } => {
                Self::ProductListingPriceConversionFailed { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::ProductListingDetailsReadModel;

    use application::{
        error::box_error,
        operation_context::{CorrelationId, Principal, RequestId},
        transaction::TransactionError,
    };
    use fxrate_core::{
        FX_RATE_SCALE, FxRateGeneration, FxRateQuote, FxRateSource, NewFxRateSnapshot,
    };
    use money::{MonetaryAmount, Price};

    use std::sync::{Arc, Mutex, MutexGuard};
    use strum::IntoEnumIterator;

    #[derive(Debug, Default)]
    struct FakeState {
        begin_error: bool,
        commit_error: bool,
        find_details_result: Option<
            Result<
                Option<PersonalizedProductListingDetailsReadModel>,
                ProductListingDetailsReadError,
            >,
        >,
        find_details_request: Option<ProductListingDetailsReadRequest>,
        latest_snapshot_result:
            Option<Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError>>,
        snapshot_by_id_result:
            Option<Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError>>,
        fx_rate_id_requests: Vec<FxRateId>,
        latest_snapshot_count: usize,

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

    impl ProductListingDetailsReaderFactory<FakeTx> for FakeDetailsReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl ProductListingDetailsReader + 'tx {
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
    impl ProductListingDetailsReader for FakeDetailsReader {
        async fn find_details(
            &mut self,
            request: &ProductListingDetailsReadRequest,
        ) -> Result<
            Option<PersonalizedProductListingDetailsReadModel>,
            ProductListingDetailsReadError,
        > {
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
            let mut state = lock_state(&self.state);
            state.latest_snapshot_count += 1;
            match state.latest_snapshot_result.take() {
                Some(result) => result,
                None => Ok(None),
            }
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

    fn handler(
        state: &SharedState,
    ) -> GetProductListingHandler<
        FakeUnitOfWork,
        FakeDetailsReaderFactory,
        FakeFxRateSnapshotRepositoryFactory,
    > {
        GetProductListingHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(state),
            },
            FakeDetailsReaderFactory {
                state: Arc::clone(state),
            },
            FakeFxRateSnapshotRepositoryFactory {
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

    fn request(language: Language, currency: Currency) -> GetProductListingRequest {
        GetProductListingRequest {
            lookup: ProductListingLookup::ById(ProductListingId::new()),
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

    fn factual_details() -> Result<PersonalizedProductListingDetailsReadModel, url::ParseError> {
        Ok(Personalized {
            item: ProductListingDetailsReadModel {
                product_listing_id: ProductListingId::new(),
                product_listing_slug_id: ProductListingSlugId::from("cabinet-abcdef"),
                event_id: EventId::new(),
                shop_id: ShopId::new(),
                seller_id: ShopId::new(),
                shop_listing_id: ShopListingId::new(),
                shop_name: ShopName::from("Shop"),
                seller_name: ShopName::from("Seller"),
                shop_slug_id: ShopSlugId::from("shop"),
                seller_slug_id: ShopSlugId::from("seller"),
                address: ProductListingAddress::default(),
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
                pricing: ProductListingPricing {
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
                images: IndexSet::<ProductListingImage>::new(),
                auction: ProductListingAuction::default(),
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

    #[test]
    fn should_present_all_prices_with_half_up_conversion_and_current_valuation()
    -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = snapshot()?;
        let source = ProductListingPricing {
            price: Some(Price::new(MonetaryAmount::from(1_u64), Currency::Eur)),
            price_estimate_min: Some(Price::new(MonetaryAmount::from(2_u64), Currency::Eur)),
            price_estimate_max: Some(Price::new(MonetaryAmount::from(3_u64), Currency::Eur)),
        };

        let presentation = present_product_pricing(source, None, &snapshot, Currency::Usd)?;

        assert_eq!(source, presentation.source);
        assert_eq!(
            DisplayProductListingPricing {
                price: Some(Price::new(MonetaryAmount::from(1_u64), Currency::Usd)),
                price_estimate_min: Some(Price::new(MonetaryAmount::from(3_u64), Currency::Usd)),
                price_estimate_max: Some(Price::new(MonetaryAmount::from(4_u64), Currency::Usd)),
            },
            presentation.display
        );
        assert_eq!(
            ProductListingPricingValuation::Current {
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
            ProductListingPricing::default(),
            Some(ProductSaleValuation {
                fx_rate_id: expected,
                sold_at: OffsetDateTime::UNIX_EPOCH,
            }),
            &snapshot,
            Currency::Eur,
        );

        assert!(matches!(
            result,
            Err(ProductListingPricingPresentationError::SaleFxSnapshotMismatch { actual, .. })
                if actual == snapshot.id()
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_present_current_pricing_from_latest_snapshot_and_commit()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let details = factual_details()?;
        let product_listing_id = details.item.product_listing_id;
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

        assert_eq!(
            Some(ProductListingDetailsReadRequest {
                lookup: request.lookup,
                language: Language::De,
                user_id: None,
            }),
            state.find_details_request
        );
        assert_eq!(product_listing_id, result.item.product_listing_id);
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
            ProductListingPricingValuation::Sale {
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
    async fn should_preserve_notification_ids_from_transactional_details_reader()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let user_id = UserId::new();
        let mut details = factual_details()?;
        let notification_id = notification_core::notification_id::NotificationId::new();
        let mut user_state = ProductListingUserState::default();
        user_state.notification.unseen_notification_ids = vec![notification_id];
        details.user_state = Some(user_state);
        lock_state(&state).find_details_result = Some(Ok(Some(details)));
        prepare_current_snapshot(&state)?;

        let result = handler(&state)
            .execute(
                &context(Principal::User(user_id)),
                request(Language::En, Currency::Eur),
            )
            .await?;

        assert_eq!(
            vec![notification_id],
            result
                .user_state
                .unwrap_or_default()
                .notification
                .unseen_notification_ids
        );
        assert_eq!(1, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_redact_hidden_product_from_reader_state()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let user_id = UserId::new();
        let mut details = factual_details()?;
        let lifecycle = details.item.lifecycle;
        let mut user_state = ProductListingUserState::default();
        user_state.search_filter.hidden = true;
        details.user_state = Some(user_state);
        lock_state(&state).find_details_result = Some(Ok(Some(details)));
        prepare_current_snapshot(&state)?;

        let result = handler(&state)
            .execute(
                &context(Principal::User(user_id)),
                request(Language::En, Currency::Eur),
            )
            .await?;

        assert_eq!(
            ProductListingId::from(uuid::Uuid::nil()),
            result.item.product_listing_id
        );
        assert_eq!(ProductState::Unknown, result.item.state);
        assert_eq!(lifecycle, result.item.lifecycle);
        assert_eq!(ProductListingPricing::default(), result.item.pricing.source);
        assert_eq!(
            DisplayProductListingPricing {
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
            Err(GetProductListingError::PricingFxSnapshotMissing)
        ));
        let state = lock_state(&state);
        assert_eq!(0, state.commit_count);
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
            Err(GetProductListingError::SaleFxSnapshotMismatch { .. })
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
            Err(GetProductListingError::PricingFxSnapshotUnavailable { .. })
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

        assert!(matches!(result, Err(GetProductListingError::NotFound)));
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
