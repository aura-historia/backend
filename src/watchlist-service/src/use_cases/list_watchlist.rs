use common::currency::domain::Currency;
use common::error::boxed::BoxError;
use common::fx_rate_id::FxRateId;
use common::language::domain::Language;
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::pagination::cursor::{Cursor, CursoredResult};

use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;
use fxrate_core::{FxRateSnapshot, FxRateSnapshotError};
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};

use product_service::ports::{
    ProductWatchlistDetailsCursor, ProductWatchlistDetailsReadError, ProductWatchlistDetailsReader,
    ProductWatchlistDetailsReaderFactory, ProductWatchlistDetailsRequest,
};
use product_service::use_cases::{
    PersonalizedProductDetailsView, ProductPricingPresentationError, present_product_details,
    redact_hidden_product,
};
use std::collections::{HashMap, HashSet};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct ListWatchlistRequest {
    pub user_id: UserId,
    pub language: Language,
    pub currency: Currency,
    pub cursor: Cursor<ProductWatchlistDetailsCursor>,
}

pub type ListWatchlistResult =
    CursoredResult<PersonalizedProductDetailsView, ProductWatchlistDetailsCursor>;

#[derive(Debug, thiserror::Error)]
pub enum ListWatchlistError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary watchlist read failure")]
    TemporarilyUnavailable,
    #[error("invalid persisted watchlist state")]
    InvalidPersistedState,
    #[error("no persisted FX snapshot is available for current product pricing")]
    CurrentPricingFxSnapshotMissing,
    #[error("sale valuation FX snapshot is missing")]
    SalePricingFxSnapshotMissing { fx_rate_id: FxRateId },
    #[error("FX snapshot lookup is temporarily unavailable for watchlist pricing")]
    PricingFxSnapshotUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("persisted FX snapshot is invalid for watchlist pricing")]
    PricingFxSnapshotInvalid {
        #[source]
        source: BoxError,
    },
    #[error("sale valuation FX snapshot does not match")]
    SaleFxSnapshotMismatch {
        expected: FxRateId,
        actual: FxRateId,
    },
    #[error("watchlist product price conversion failed")]
    ProductPriceConversionFailed {
        #[source]
        source: FxRateSnapshotError,
    },

    #[error("failed to begin watchlist transaction")]
    BeginTransactionFailed,
    #[error("failed to commit watchlist transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait ListWatchlistUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListWatchlistRequest,
    ) -> Result<ListWatchlistResult, ListWatchlistError>;
}

pub struct ListWatchlistHandler<U, D, F> {
    unit_of_work: U,
    details_reader: D,
    fx_rates: F,
}

impl<U, D, F> ListWatchlistHandler<U, D, F> {
    pub fn new(unit_of_work: U, details_reader: D, fx_rates: F) -> Self {
        Self {
            unit_of_work,
            details_reader,
            fx_rates,
        }
    }
}

#[async_trait::async_trait]
impl<U, D, F> ListWatchlistUseCase for ListWatchlistHandler<U, D, F>
where
    U: UnitOfWork,
    D: ProductWatchlistDetailsReaderFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(name = "list_watchlist", skip_all, fields(user_id = %request.user_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListWatchlistRequest,
    ) -> Result<ListWatchlistResult, ListWatchlistError> {
        authorize_read(context, request.user_id)?;

        let valuation_at = OffsetDateTime::now_utc();
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ListWatchlistError::BeginTransactionFailed)?;
        let cursor = Cursor {
            size: request.cursor.size.clamp(1, 100),
            search_after: request.cursor.search_after,
        };
        let factual_page = self
            .details_reader
            .in_transaction(&mut tx)
            .find_for_user(&ProductWatchlistDetailsRequest {
                user_id: request.user_id,
                language: request.language,
                cursor,
            })
            .await?;
        let pricing_snapshots =
            pricing_snapshots(&self.fx_rates, &mut tx, &factual_page.items, valuation_at).await?;
        let CursoredResult {
            items,
            cursor,
            total,
        } = factual_page;
        let mut page = CursoredResult {
            items: items
                .into_iter()
                .map(|factual_details| {
                    present_with_pricing_snapshot(
                        factual_details,
                        &pricing_snapshots,
                        request.currency,
                    )
                })
                .collect::<Result<_, _>>()?,
            cursor,
            total,
        };

        tx.commit()
            .await
            .map_err(|_| ListWatchlistError::CommitTransactionFailed)?;

        for product in &mut page.items {
            let user_state = product
                .user_state
                .as_ref()
                .ok_or(ListWatchlistError::InvalidPersistedState)?;
            if user_state.search_filter.hidden {
                redact_hidden_product(&mut product.item)
                    .map_err(|_| ListWatchlistError::InvalidPersistedState)?;
            }
        }

        Ok(page)
    }
}

struct PricingSnapshots {
    current: Option<FxRateSnapshot>,
    sale: HashMap<FxRateId, FxRateSnapshot>,
}

async fn pricing_snapshots<Tx, F>(
    fx_rates: &F,
    tx: &mut Tx,
    factual_details: &[product_service::ports::PersonalizedProductDetailsReadModel],
    valuation_at: OffsetDateTime,
) -> Result<PricingSnapshots, ListWatchlistError>
where
    F: FxRateSnapshotRepositoryFactory<Tx>,
{
    let sale_snapshot_ids: HashSet<_> = factual_details
        .iter()
        .filter_map(|details| {
            details
                .item
                .sale_valuation
                .map(|valuation| valuation.fx_rate_id)
        })
        .collect();
    let current = if factual_details
        .iter()
        .any(|details| details.item.sale_valuation.is_none())
    {
        Some(
            fx_rates
                .in_transaction(tx)
                .find_latest_at_or_before(valuation_at)
                .await?
                .ok_or(ListWatchlistError::CurrentPricingFxSnapshotMissing)?,
        )
    } else {
        None
    };
    let sale_snapshot_ids = sale_snapshot_ids.into_iter().collect::<Vec<_>>();
    let sale = if sale_snapshot_ids.is_empty() {
        HashMap::new()
    } else {
        fx_rates
            .in_transaction(tx)
            .find_by_ids(&sale_snapshot_ids)
            .await?
            .into_iter()
            .map(|snapshot| (snapshot.id(), snapshot))
            .collect()
    };

    Ok(PricingSnapshots { current, sale })
}

fn present_with_pricing_snapshot(
    factual_details: product_service::ports::PersonalizedProductDetailsReadModel,
    pricing_snapshots: &PricingSnapshots,
    currency: Currency,
) -> Result<PersonalizedProductDetailsView, ListWatchlistError> {
    let snapshot = match factual_details.item.sale_valuation {
        Some(valuation) => pricing_snapshots.sale.get(&valuation.fx_rate_id).ok_or(
            ListWatchlistError::SalePricingFxSnapshotMissing {
                fx_rate_id: valuation.fx_rate_id,
            },
        )?,
        None => pricing_snapshots
            .current
            .as_ref()
            .ok_or(ListWatchlistError::CurrentPricingFxSnapshotMissing)?,
    };
    Ok(present_product_details(
        factual_details,
        snapshot,
        currency,
    )?)
}

fn authorize_read(context: &OperationContext, user_id: UserId) -> Result<(), ListWatchlistError> {
    context
        .require()
        .credential_capability(CredentialCapability::WatchlistRead)
        .user(&user_id)
        .service_or_system()
        .authorize::<ListWatchlistError>()
}

impl From<OperationAuthorizationError> for ListWatchlistError {
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

impl From<ProductWatchlistDetailsReadError> for ListWatchlistError {
    fn from(error: ProductWatchlistDetailsReadError) -> Self {
        match error {
            ProductWatchlistDetailsReadError::QueryFailed => Self::TemporarilyUnavailable,
            ProductWatchlistDetailsReadError::InvalidReadModel => Self::InvalidPersistedState,
        }
    }
}

impl From<FxRateSnapshotRepositoryError> for ListWatchlistError {
    fn from(error: FxRateSnapshotRepositoryError) -> Self {
        match error {
            FxRateSnapshotRepositoryError::InsertFailed { source }
            | FxRateSnapshotRepositoryError::ReadFailed { source } => {
                Self::PricingFxSnapshotUnavailable { source }
            }
            FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
                Self::PricingFxSnapshotInvalid { source }
            }
            FxRateSnapshotRepositoryError::CapturedAtNotMonotonic => {
                Self::CurrentPricingFxSnapshotMissing
            }
        }
    }
}

impl From<ProductPricingPresentationError> for ListWatchlistError {
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
    use common::error::boxed::box_error;
    use common::event_id::EventId;
    use common::localized::Localized;
    use common::notification_id::NotificationId;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use common::personalized::Personalized;
    use common::price::domain::{MonetaryAmount, Price};
    use common::product_id::ProductId;
    use common::product_lifecycle::domain::ProductLifecycle;
    use common::product_slug_id::ProductSlugId;
    use common::product_state::domain::ProductState;
    use common::shop_id::ShopId;
    use common::shop_name::ShopName;
    use common::shop_slug_id::ShopSlugId;
    use common::shops_product_id::ShopsProductId;
    use common::transaction::TransactionError;
    use fxrate_core::{
        FX_RATE_SCALE, FxRateGeneration, FxRateQuote, FxRateSource, NewFxRateSnapshot,
    };

    use product_core::description::Description;
    use product_core::product::{
        ProductAddress, ProductAuction, ProductPricing, ProductSaleValuation,
    };
    use product_core::title::Title;
    use product_core::user_state::{NotificationUserState, ProductUserState};
    use product_service::ports::{PersonalizedProductDetailsReadModel, ProductDetailsReadModel};
    use product_service::use_cases::ProductPricingValuation;

    use std::sync::{Arc, Mutex, MutexGuard};
    use strum::IntoEnumIterator;
    use time::OffsetDateTime;
    use url::Url;

    #[derive(Default)]
    struct FakeState {
        begin_fails: bool,
        commit_fails: bool,
        details_result: Option<
            Result<
                CursoredResult<PersonalizedProductDetailsReadModel, ProductWatchlistDetailsCursor>,
                ProductWatchlistDetailsReadError,
            >,
        >,
        latest_snapshot_result:
            Option<Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError>>,
        sale_snapshots_result: Option<Result<Vec<FxRateSnapshot>, FxRateSnapshotRepositoryError>>,

        begin_count: usize,
        commit_count: usize,
        details_requests: Vec<ProductWatchlistDetailsRequest>,
        latest_snapshot_requests: usize,
        sale_snapshot_requests: Vec<Vec<FxRateId>>,
    }

    type SharedState = Arc<Mutex<FakeState>>;

    #[derive(Clone)]
    struct FakeUnitOfWork(SharedState);
    #[derive(Clone)]
    struct FakeDetailsReaderFactory(SharedState);
    #[derive(Clone)]
    struct FakeFxRateSnapshotRepositoryFactory(SharedState);

    struct FakeTransaction(SharedState);
    struct FakeDetailsReader(SharedState);
    struct FakeFxRateSnapshotRepository(SharedState);

    fn state() -> SharedState {
        Arc::new(Mutex::new(FakeState::default()))
    }

    fn lock(state: &SharedState) -> MutexGuard<'_, FakeState> {
        match state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let mut state = lock(&self.0);
            state.begin_count += 1;
            if state.begin_fails {
                Err(TransactionError::BeginFailed)
            } else {
                Ok(FakeTransaction(Arc::clone(&self.0)))
            }
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock(&self.0);
            state.commit_count += 1;
            if state.commit_fails {
                Err(TransactionError::CommitFailed)
            } else {
                Ok(())
            }
        }
    }

    impl ProductWatchlistDetailsReaderFactory<FakeTransaction> for FakeDetailsReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ProductWatchlistDetailsReader + 'tx {
            FakeDetailsReader(Arc::clone(&self.0))
        }
    }

    impl FxRateSnapshotRepositoryFactory<FakeTransaction> for FakeFxRateSnapshotRepositoryFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl FxRateSnapshotRepository + 'tx {
            FakeFxRateSnapshotRepository(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl ProductWatchlistDetailsReader for FakeDetailsReader {
        async fn find_for_user(
            &mut self,
            request: &ProductWatchlistDetailsRequest,
        ) -> Result<
            CursoredResult<PersonalizedProductDetailsReadModel, ProductWatchlistDetailsCursor>,
            ProductWatchlistDetailsReadError,
        > {
            let mut state = lock(&self.0);
            state.details_requests.push(request.clone());
            state.details_result.take().unwrap_or_else(|| {
                Ok(CursoredResult {
                    items: Vec::new(),
                    cursor: request.cursor,
                    total: None,
                })
            })
        }
    }

    #[async_trait::async_trait]
    impl FxRateSnapshotRepository for FakeFxRateSnapshotRepository {
        async fn find_latest(
            &mut self,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state = lock(&self.0);
            state.latest_snapshot_requests += 1;
            state.latest_snapshot_result.take().unwrap_or(Ok(None))
        }

        async fn find_latest_at_or_before(
            &mut self,
            _timestamp: OffsetDateTime,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state = lock(&self.0);
            state.latest_snapshot_requests += 1;
            state.latest_snapshot_result.take().unwrap_or(Ok(None))
        }

        async fn find_by_id(
            &mut self,
            _id: FxRateId,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_by_ids(
            &mut self,
            ids: &[FxRateId],
        ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state = lock(&self.0);
            state.sale_snapshot_requests.push(ids.to_vec());
            state.sale_snapshots_result.take().unwrap_or(Ok(Vec::new()))
        }

        async fn insert(
            &mut self,
            _snapshot: &fxrate_core::NewFxRateSnapshot,
            _source_event_id: &str,
        ) -> Result<fxrate_service::ports::FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError>
        {
            Err(FxRateSnapshotRepositoryError::ReadFailed {
                source: box_error(std::io::Error::other("not implemented in fake")),
            })
        }
    }

    fn handler(
        state: &SharedState,
    ) -> ListWatchlistHandler<
        FakeUnitOfWork,
        FakeDetailsReaderFactory,
        FakeFxRateSnapshotRepositoryFactory,
    > {
        ListWatchlistHandler::new(
            FakeUnitOfWork(Arc::clone(state)),
            FakeDetailsReaderFactory(Arc::clone(state)),
            FakeFxRateSnapshotRepositoryFactory(Arc::clone(state)),
        )
    }

    fn context(user_id: UserId) -> OperationContext {
        OperationContext {
            principal: Principal::User(user_id),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn request(user_id: UserId) -> ListWatchlistRequest {
        ListWatchlistRequest {
            user_id,
            language: Language::En,
            currency: Currency::Usd,
            cursor: Cursor::default(),
        }
    }

    fn delegated_context(user_id: UserId, capability: bool) -> OperationContext {
        let capabilities = if capability {
            [CredentialCapability::WatchlistRead].into_iter().collect()
        } else {
            Default::default()
        };
        OperationContext {
            principal: Principal::DelegatedUser {
                user_id,
                capabilities,
            },
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn snapshot(id: FxRateId) -> Result<FxRateSnapshot, FxRateSnapshotError> {
        let snapshot = NewFxRateSnapshot::capture_eur(
            id,
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
        Ok(snapshot.into_persisted(FxRateGeneration::try_from(1)?))
    }

    fn details(
        product_id: ProductId,
    ) -> Result<PersonalizedProductDetailsReadModel, url::ParseError> {
        let url = Url::parse("https://example.test/product")?;
        Ok(Personalized {
            item: ProductDetailsReadModel {
                product_id,
                product_slug_id: ProductSlugId::from("product"),
                event_id: EventId::new(),
                shop_id: ShopId::new(),
                seller_id: ShopId::new(),
                shops_product_id: ShopsProductId::from("product"),
                shop_name: ShopName::from("Shop"),
                seller_name: ShopName::from("Seller"),
                shop_slug_id: ShopSlugId::from("shop"),
                seller_slug_id: ShopSlugId::from("seller"),
                address: ProductAddress::default(),
                product_title: Some(Localized::new(Language::En, Title::from("Product"))),
                product_description: Some(Localized::new(
                    Language::En,
                    Description::from("Description"),
                )),
                title: Some(Localized::new(Language::En, Title::from("Product"))),
                description: Some(Localized::new(
                    Language::En,
                    Description::from("Description"),
                )),
                pricing: ProductPricing {
                    price: Some(Price::new(MonetaryAmount::from(100_u64), Currency::Eur)),
                    price_estimate_min: None,
                    price_estimate_max: None,
                },
                sale_valuation: None,
                state: ProductState::Available,
                lifecycle: ProductLifecycle::Active,
                url: url.clone(),
                view_url: url,
                images: Default::default(),
                auction: ProductAuction::default(),
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            },
            user_state: Some(ProductUserState::default()),
        })
    }

    fn page(
        items: Vec<PersonalizedProductDetailsReadModel>,
    ) -> CursoredResult<PersonalizedProductDetailsReadModel, ProductWatchlistDetailsCursor> {
        CursoredResult {
            items,
            cursor: Cursor::default(),
            total: None,
        }
    }

    #[tokio::test]
    async fn should_use_one_current_snapshot_for_all_current_products()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let first_product_id = ProductId::new();
        let second_product_id = ProductId::new();
        let current_snapshot = snapshot(FxRateId::new())?;
        let state = state();
        lock(&state).details_result = Some(Ok(page(vec![
            details(first_product_id)?,
            details(second_product_id)?,
        ])));
        lock(&state).latest_snapshot_result = Some(Ok(Some(current_snapshot.clone())));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id))
            .await?;

        assert_eq!(
            vec![first_product_id, second_product_id],
            result
                .items
                .iter()
                .map(|item| item.item.product_id)
                .collect::<Vec<_>>()
        );
        assert!(result.items.iter().all(|item| matches!(
            item.item.pricing.valuation,
            ProductPricingValuation::Current { fx_rate_id, .. } if fx_rate_id == current_snapshot.id()
        )));
        let state = lock(&state);
        assert_eq!(1, state.latest_snapshot_requests);
        assert!(state.sale_snapshot_requests.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_retain_canonical_notification_user_state()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let expected_user_state = ProductUserState {
            notification: NotificationUserState {
                unseen_notification_ids: vec![NotificationId::new()],
            },
            ..Default::default()
        };
        let mut product = details(ProductId::new())?;
        product.user_state = Some(expected_user_state.clone());
        let state = state();
        lock(&state).details_result = Some(Ok(page(vec![product])));
        lock(&state).latest_snapshot_result = Some(Ok(Some(snapshot(FxRateId::new())?)));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id))
            .await?;

        assert_eq!(
            Some(&expected_user_state),
            result.items[0].user_state.as_ref()
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_batch_sale_valuation_snapshots_without_current_snapshot()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let first_snapshot = snapshot(FxRateId::new())?;
        let second_snapshot = snapshot(FxRateId::new())?;
        let mut first = details(ProductId::new())?;
        first.item.sale_valuation = Some(ProductSaleValuation {
            fx_rate_id: first_snapshot.id(),
            sold_at: OffsetDateTime::UNIX_EPOCH,
        });
        let mut second = details(ProductId::new())?;
        second.item.sale_valuation = Some(ProductSaleValuation {
            fx_rate_id: second_snapshot.id(),
            sold_at: OffsetDateTime::UNIX_EPOCH,
        });
        let state = state();
        lock(&state).details_result = Some(Ok(page(vec![first, second])));
        lock(&state).sale_snapshots_result =
            Some(Ok(vec![first_snapshot.clone(), second_snapshot.clone()]));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id))
            .await?;

        assert!(matches!(
            result.items[0].item.pricing.valuation,
            ProductPricingValuation::Sale { fx_rate_id, .. } if fx_rate_id == first_snapshot.id()
        ));
        assert!(matches!(
            result.items[1].item.pricing.valuation,
            ProductPricingValuation::Sale { fx_rate_id, .. } if fx_rate_id == second_snapshot.id()
        ));
        let state = lock(&state);
        assert_eq!(0, state.latest_snapshot_requests);
        assert_eq!(1, state.sale_snapshot_requests.len());
        assert_eq!(
            HashSet::from([first_snapshot.id(), second_snapshot.id()]),
            state.sale_snapshot_requests[0].iter().copied().collect(),
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_use_current_and_sale_snapshots_for_mixed_page()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let current_snapshot = snapshot(FxRateId::new())?;
        let sale_snapshot = snapshot(FxRateId::new())?;
        let current = details(ProductId::new())?;
        let mut sale = details(ProductId::new())?;
        sale.item.sale_valuation = Some(ProductSaleValuation {
            fx_rate_id: sale_snapshot.id(),
            sold_at: OffsetDateTime::UNIX_EPOCH,
        });
        let state = state();
        lock(&state).details_result = Some(Ok(page(vec![current, sale])));
        lock(&state).latest_snapshot_result = Some(Ok(Some(current_snapshot)));
        lock(&state).sale_snapshots_result = Some(Ok(vec![sale_snapshot.clone()]));

        handler(&state)
            .execute(&context(user_id), request(user_id))
            .await?;

        let state = lock(&state);
        assert_eq!(1, state.latest_snapshot_requests);
        assert_eq!(vec![vec![sale_snapshot.id()]], state.sale_snapshot_requests);
        Ok(())
    }

    #[tokio::test]
    async fn should_fail_without_fallback_when_sale_snapshot_is_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let missing_snapshot_id = FxRateId::new();
        let mut sale = details(ProductId::new())?;
        sale.item.sale_valuation = Some(ProductSaleValuation {
            fx_rate_id: missing_snapshot_id,
            sold_at: OffsetDateTime::UNIX_EPOCH,
        });
        let state = state();
        lock(&state).details_result = Some(Ok(page(vec![sale])));
        lock(&state).sale_snapshots_result = Some(Ok(Vec::new()));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id))
            .await;

        assert!(matches!(
            result,
            Err(ListWatchlistError::SalePricingFxSnapshotMissing { fx_rate_id }) if fx_rate_id == missing_snapshot_id
        ));
        let state = lock(&state);
        assert_eq!(0, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_fail_without_fallback_when_current_snapshot_is_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let state = state();
        lock(&state).details_result = Some(Ok(page(vec![details(ProductId::new())?])));
        lock(&state).latest_snapshot_result = Some(Ok(None));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id))
            .await;

        assert!(matches!(
            result,
            Err(ListWatchlistError::CurrentPricingFxSnapshotMissing)
        ));
        let state = lock(&state);
        assert_eq!(0, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_typed_error_when_current_snapshot_is_invalid()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let state = state();
        lock(&state).details_result = Some(Ok(page(vec![details(ProductId::new())?])));
        lock(&state).latest_snapshot_result = Some(Err(
            FxRateSnapshotRepositoryError::InvalidPersistedSnapshot {
                source: box_error(std::io::Error::other("invalid FX snapshot")),
            },
        ));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id))
            .await;

        assert!(matches!(
            result,
            Err(ListWatchlistError::PricingFxSnapshotInvalid { .. })
        ));
        let state = lock(&state);
        assert_eq!(0, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_commit_an_empty_watchlist() -> Result<(), ListWatchlistError> {
        let user_id = UserId::new();
        let state = state();

        handler(&state)
            .execute(&context(user_id), request(user_id))
            .await?;

        let state = lock(&state);
        assert_eq!(0, state.latest_snapshot_requests);
        assert!(state.sale_snapshot_requests.is_empty());
        assert_eq!(1, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_when_details_read_fails() {
        let user_id = UserId::new();
        let state = state();
        lock(&state).details_result = Some(Err(ProductWatchlistDetailsReadError::QueryFailed));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id))
            .await;

        assert!(matches!(
            result,
            Err(ListWatchlistError::TemporarilyUnavailable)
        ));
        let state = lock(&state);
        assert_eq!(0, state.commit_count);
        assert_eq!(0, state.latest_snapshot_requests);
    }

    #[tokio::test]
    async fn should_return_commit_failure() -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let state = state();
        lock(&state).commit_fails = true;
        lock(&state).details_result = Some(Ok(page(vec![details(ProductId::new())?])));
        lock(&state).latest_snapshot_result = Some(Ok(Some(snapshot(FxRateId::new())?)));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id))
            .await;

        assert!(matches!(
            result,
            Err(ListWatchlistError::CommitTransactionFailed)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_missing_canonical_user_state() -> Result<(), Box<dyn std::error::Error>>
    {
        let user_id = UserId::new();
        let mut product = details(ProductId::new())?;
        product.user_state = None;
        let state = state();
        lock(&state).details_result = Some(Ok(page(vec![product])));
        lock(&state).latest_snapshot_result = Some(Ok(Some(snapshot(FxRateId::new())?)));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id))
            .await;

        assert!(matches!(
            result,
            Err(ListWatchlistError::InvalidPersistedState)
        ));
        let state = lock(&state);
        assert_eq!(1, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_begin_transaction_when_delegated_user_lacks_capability() {
        let user_id = UserId::new();
        let state = state();

        let result = handler(&state)
            .execute(&delegated_context(user_id, false), request(user_id))
            .await;

        assert!(matches!(result, Err(ListWatchlistError::Forbidden)));
        assert_eq!(0, lock(&state).begin_count);
    }

    #[tokio::test]
    async fn should_allow_delegated_user_with_watchlist_read_capability()
    -> Result<(), ListWatchlistError> {
        let user_id = UserId::new();
        let state = state();

        handler(&state)
            .execute(&delegated_context(user_id, true), request(user_id))
            .await?;

        assert_eq!(1, lock(&state).commit_count);
        Ok(())
    }
}
