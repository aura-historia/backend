use crate::ports::{
    ProductListingHistoryReadError, ProductListingHistoryReader, ProductListingHistoryReaderFactory,
};
use application::{
    error::BoxError,
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use domain_primitives::event_id::EventId;
use fxrate_core::{FxRateId, FxRateSnapshot, FxRateSnapshotError, RoundingMode};
use fxrate_service::ports::{FxRateSnapshotReadError, FxRateSnapshotReader};
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use money::{Currency, Price};
use product_listing_core::{
    description::Description,
    listing_availability::ListingAvailability,
    product_listing::{ListingSaleObservation, ProductListingAuction, ProductListingPricing},
    product_listing_event::ProductListingEventType,
    product_listing_id::ProductListingId,
    product_listing_price::ProductListingPrice,
    product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId,
    title::Title,
};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingHistoryLookup {
    ById(ProductListingId),
    ByTitleSlug(ProductListingSlugId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct GetProductListingHistoryRequest {
    pub lookup: ProductListingHistoryLookup,
    pub currency: Option<Currency>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingHistoryEntry {
    pub product_listing_id: ProductListingId,
    pub event_id: EventId,
    pub occurred_at: OffsetDateTime,
    pub kind: ProductListingHistoryEntryKind,
}

impl ProductListingHistoryEntry {
    pub const fn event_type(&self) -> ProductListingEventType {
        self.kind.event_type()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingHistoryEntryKind {
    Discovered(Box<ProductListingDiscoveryHistory>),
    Changed(ProductListingHistoryChanges),
}

impl ProductListingHistoryEntryKind {
    pub const fn event_type(&self) -> ProductListingEventType {
        match self {
            Self::Discovered(_) => ProductListingEventType::Discovered,
            Self::Changed(_) => ProductListingEventType::Changed,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingDiscoveryHistory {
    pub listing_source_id: ListingSourceId,
    pub source_listing_id: SourceListingId,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductListingHistoryPricing,
    pub availability: Option<ListingAvailability>,
    pub url: Url,
    pub image_count: u64,
    pub auction: Option<ProductListingAuction>,
}

/// Source facts and an optional historical display projection, never persisted together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductListingHistoryPrice<T> {
    pub source: T,
    pub display: Option<ProductListingHistoryPriceDisplay>,
}

impl<T> From<T> for ProductListingHistoryPrice<T> {
    fn from(source: T) -> Self {
        Self {
            source,
            display: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductListingHistoryPriceDisplay {
    pub price: Price,
    pub fx_rate_id: FxRateId,
    pub captured_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProductListingHistoryPricing {
    pub price: Option<ProductListingHistoryPrice<ProductListingPrice>>,
    pub price_estimate_min: Option<ProductListingHistoryPrice<Price>>,
    pub price_estimate_max: Option<ProductListingHistoryPrice<Price>>,
}

impl From<ProductListingPricing> for ProductListingHistoryPricing {
    fn from(pricing: ProductListingPricing) -> Self {
        Self {
            price: pricing.price.map(Into::into),
            price_estimate_min: pricing.price_estimate_min.map(Into::into),
            price_estimate_max: pricing.price_estimate_max.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingHistoryChanges(Vec<ProductListingHistoryChange>);

impl ProductListingHistoryChanges {
    pub fn as_slice(&self) -> &[ProductListingHistoryChange] {
        &self.0
    }

    pub fn into_inner(self) -> Vec<ProductListingHistoryChange> {
        self.0
    }
}

#[derive(Debug, thiserror::Error)]
#[error("ProductListing history changes must not be empty")]
pub struct EmptyProductListingHistoryChangesError;

impl TryFrom<Vec<ProductListingHistoryChange>> for ProductListingHistoryChanges {
    type Error = EmptyProductListingHistoryChangesError;

    fn try_from(changes: Vec<ProductListingHistoryChange>) -> Result<Self, Self::Error> {
        if changes.is_empty() {
            return Err(EmptyProductListingHistoryChangesError);
        }

        Ok(Self(changes))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingHistoryChange {
    MainPriceChanged {
        previous: Option<ProductListingHistoryPrice<ProductListingPrice>>,
        current: Option<ProductListingHistoryPrice<ProductListingPrice>>,
    },
    MinimumEstimateChanged {
        previous: Option<ProductListingHistoryPrice<Price>>,
        current: Option<ProductListingHistoryPrice<Price>>,
    },
    MaximumEstimateChanged {
        previous: Option<ProductListingHistoryPrice<Price>>,
        current: Option<ProductListingHistoryPrice<Price>>,
    },
    AvailabilityChanged {
        previous: Option<ListingAvailability>,
        current: Option<ListingAvailability>,
    },
    UrlChanged {
        previous: Url,
        current: Url,
    },
    ImagesChanged {
        previous_count: u64,
        current_count: u64,
    },
    AuctionChanged {
        previous: Option<ProductListingAuction>,
        current: Option<ProductListingAuction>,
    },
    Withdrawn {
        previous_availability: Option<ListingAvailability>,
    },
    Restored,
    SaleObserved {
        observation: ListingSaleObservation,
    },
    SaleObservationRetracted {
        observation: ListingSaleObservation,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum GetProductListingHistoryError {
    #[error("product listing not found")]
    NotFound,
    #[error("product listing history query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product listing history read model is invalid")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("product listing history FX snapshot read failed")]
    FxSnapshotUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("product listing history price conversion failed")]
    PriceConversionFailed(#[source] FxRateSnapshotError),
    #[error("failed to begin product listing history transaction")]
    BeginTransactionFailed(#[source] application::error::BoxError),
    #[error("failed to commit product listing history transaction")]
    CommitTransactionFailed(#[source] application::error::BoxError),
}

#[async_trait::async_trait]
pub trait GetProductListingHistoryUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetProductListingHistoryRequest,
    ) -> Result<Vec<ProductListingHistoryEntry>, GetProductListingHistoryError>;
}

pub struct GetProductListingHistoryHandler<U, R, F> {
    unit_of_work: U,
    reader: R,
    fx_rates: F,
}

impl<U, R, F> GetProductListingHistoryHandler<U, R, F> {
    pub fn new(unit_of_work: U, reader: R, fx_rates: F) -> Self {
        Self {
            unit_of_work,
            reader,
            fx_rates,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, F> GetProductListingHistoryUseCase for GetProductListingHistoryHandler<U, R, F>
where
    U: UnitOfWork,
    R: ProductListingHistoryReaderFactory<U::Tx>,
    F: FxRateSnapshotReader,
{
    #[tracing::instrument(name = "get_product_listing_history", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetProductListingHistoryRequest,
    ) -> Result<Vec<ProductListingHistoryEntry>, GetProductListingHistoryError> {
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            GetProductListingHistoryError::BeginTransactionFailed(Box::new(source))
        })?;
        let mut history = self
            .reader
            .in_transaction(&mut tx)
            .find_history(&request.lookup)
            .await?
            .ok_or(GetProductListingHistoryError::NotFound)?;
        tx.commit().await.map_err(|source| {
            GetProductListingHistoryError::CommitTransactionFailed(Box::new(source))
        })?;
        if let Some(currency) = request.currency {
            let timestamps = history
                .iter()
                .filter(|entry| has_monetary_value(entry))
                .map(|entry| entry.occurred_at)
                .collect::<Vec<_>>();
            if !timestamps.is_empty() {
                let snapshots = self
                    .fx_rates
                    .find_latest_at_or_before_many(&timestamps)
                    .await
                    .map_err(|source| match source {
                        FxRateSnapshotReadError::ReadFailed { .. } => {
                            GetProductListingHistoryError::FxSnapshotUnavailable {
                                source: Box::new(source),
                            }
                        }
                        FxRateSnapshotReadError::InvalidPersistedSnapshot { .. } => {
                            GetProductListingHistoryError::InvalidReadModel {
                                source: Box::new(source),
                            }
                        }
                    })?;
                for entry in &mut history {
                    let index = snapshots
                        .partition_point(|snapshot| snapshot.captured_at() <= entry.occurred_at);
                    let snapshot = index.checked_sub(1).map(|index| &snapshots[index]);
                    project_entry(entry, snapshot, currency)?;
                }
            }
        }
        Ok(history)
    }
}

fn has_monetary_value(entry: &ProductListingHistoryEntry) -> bool {
    let is_monetary = |value: Option<ProductListingHistoryPrice<ProductListingPrice>>| {
        value.is_some_and(|value| matches!(value.source, ProductListingPrice::Monetary(_)))
    };
    match &entry.kind {
        ProductListingHistoryEntryKind::Discovered(discovery) => {
            is_monetary(discovery.pricing.price)
                || discovery.pricing.price_estimate_min.is_some()
                || discovery.pricing.price_estimate_max.is_some()
        }
        ProductListingHistoryEntryKind::Changed(changes) => {
            changes.as_slice().iter().any(|change| match change {
                ProductListingHistoryChange::MainPriceChanged { previous, current } => {
                    is_monetary(*previous) || is_monetary(*current)
                }
                ProductListingHistoryChange::MinimumEstimateChanged { previous, current }
                | ProductListingHistoryChange::MaximumEstimateChanged { previous, current } => {
                    previous.is_some() || current.is_some()
                }
                _ => false,
            })
        }
    }
}

fn project_entry(
    entry: &mut ProductListingHistoryEntry,
    snapshot: Option<&FxRateSnapshot>,
    currency: Currency,
) -> Result<(), GetProductListingHistoryError> {
    match &mut entry.kind {
        ProductListingHistoryEntryKind::Discovered(discovery) => {
            project_main_price(discovery.pricing.price.as_mut(), snapshot, currency)?;
            project_estimate(
                discovery.pricing.price_estimate_min.as_mut(),
                snapshot,
                currency,
            )?;
            project_estimate(
                discovery.pricing.price_estimate_max.as_mut(),
                snapshot,
                currency,
            )?;
        }
        ProductListingHistoryEntryKind::Changed(changes) => {
            for change in &mut changes.0 {
                match change {
                    ProductListingHistoryChange::MainPriceChanged { previous, current } => {
                        project_main_price(previous.as_mut(), snapshot, currency)?;
                        project_main_price(current.as_mut(), snapshot, currency)?;
                    }
                    ProductListingHistoryChange::MinimumEstimateChanged { previous, current }
                    | ProductListingHistoryChange::MaximumEstimateChanged { previous, current } => {
                        project_estimate(previous.as_mut(), snapshot, currency)?;
                        project_estimate(current.as_mut(), snapshot, currency)?;
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

fn project_main_price(
    value: Option<&mut ProductListingHistoryPrice<ProductListingPrice>>,
    snapshot: Option<&FxRateSnapshot>,
    currency: Currency,
) -> Result<(), GetProductListingHistoryError> {
    if let Some(value) = value {
        value.display = match value.source {
            ProductListingPrice::Monetary(price) => display_price(price, snapshot, currency)?,
            ProductListingPrice::OnRequest => None,
        };
    }
    Ok(())
}

fn project_estimate(
    value: Option<&mut ProductListingHistoryPrice<Price>>,
    snapshot: Option<&FxRateSnapshot>,
    currency: Currency,
) -> Result<(), GetProductListingHistoryError> {
    if let Some(value) = value {
        value.display = display_price(value.source, snapshot, currency)?;
    }
    Ok(())
}

fn display_price(
    price: Price,
    snapshot: Option<&FxRateSnapshot>,
    currency: Currency,
) -> Result<Option<ProductListingHistoryPriceDisplay>, GetProductListingHistoryError> {
    let Some(snapshot) = snapshot else {
        return Ok(None);
    };
    // Even a same-currency projection requires that currency's historical quote.
    if !snapshot
        .quotes()
        .iter()
        .any(|quote| quote.currency() == currency)
    {
        return Ok(None);
    }
    match snapshot.convert(price, currency, RoundingMode::HalfUp) {
        Ok(price) => Ok(Some(ProductListingHistoryPriceDisplay {
            price,
            fx_rate_id: snapshot.id(),
            captured_at: snapshot.captured_at(),
        })),
        Err(FxRateSnapshotError::MissingQuote(_)) => Ok(None),
        Err(source) => Err(GetProductListingHistoryError::PriceConversionFailed(source)),
    }
}

impl From<ProductListingHistoryReadError> for GetProductListingHistoryError {
    fn from(error: ProductListingHistoryReadError) -> Self {
        match error {
            ProductListingHistoryReadError::ProductListingHistoryQueryFailed { source } => {
                Self::QueryFailed { source }
            }
            ProductListingHistoryReadError::ProductListingHistoryReadModelInvalid { source } => {
                Self::InvalidReadModel { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use application::{
        error::static_error, operation_context::Principal, transaction::TransactionError,
    };
    use fxrate_core::{FX_RATE_SCALE, FxRateGeneration, FxRateQuote, FxRateSource};
    use fxrate_service::ports::FxRateSnapshotReadError;
    use money::MonetaryAmount;
    use std::sync::{Arc, Mutex};
    use strum::IntoEnumIterator;
    use time::Duration;

    #[derive(Clone, Default)]
    struct FakePorts {
        history: Option<Vec<ProductListingHistoryEntry>>,
        snapshots: Vec<FxRateSnapshot>,
        calls: Arc<Mutex<Vec<Vec<OffsetDateTime>>>>,
        commits: Arc<Mutex<usize>>,
        fail_fx_read: bool,
    }

    #[async_trait::async_trait]
    impl Transaction for FakePorts {
        async fn commit(self) -> Result<(), TransactionError> {
            *self.commits.lock().unwrap() += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakePorts {
        type Tx = Self;
        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            Ok(self.clone())
        }
    }

    impl ProductListingHistoryReaderFactory<FakePorts> for FakePorts {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakePorts,
        ) -> impl ProductListingHistoryReader + 'tx {
            self.clone()
        }
    }

    #[async_trait::async_trait]
    impl ProductListingHistoryReader for FakePorts {
        async fn find_history(
            &mut self,
            _lookup: &ProductListingHistoryLookup,
        ) -> Result<Option<Vec<ProductListingHistoryEntry>>, ProductListingHistoryReadError>
        {
            Ok(self.history.clone())
        }
    }

    #[async_trait::async_trait]
    impl FxRateSnapshotReader for FakePorts {
        async fn find_by_id(
            &self,
            _id: FxRateId,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
            panic!("history must use one batch read")
        }
        async fn find_latest_at_or_before(
            &self,
            _at: OffsetDateTime,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
            panic!("history must use one batch read")
        }
        async fn find_latest_at_or_before_many(
            &self,
            timestamps: &[OffsetDateTime],
        ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotReadError> {
            self.calls.lock().unwrap().push(timestamps.to_vec());
            if self.fail_fx_read {
                return Err(FxRateSnapshotReadError::ReadFailed {
                    source: static_error("database unavailable"),
                });
            }
            Ok(self.snapshots.clone())
        }
    }

    fn snapshot(at: OffsetDateTime, multiplier: u64, legacy: bool) -> FxRateSnapshot {
        let added = [
            Currency::Sek,
            Currency::Dkk,
            Currency::Nok,
            Currency::Krw,
            Currency::Inr,
            Currency::Twd,
            Currency::Huf,
            Currency::Ron,
            Currency::Mxn,
            Currency::Thb,
        ];
        FxRateSnapshot::rehydrate(
            FxRateId::new(),
            FxRateGeneration::try_from(1).unwrap(),
            at,
            FxRateSource::FxRatesApi,
            Currency::iter()
                .filter(|currency| !legacy || !added.contains(currency))
                .map(|currency| {
                    FxRateQuote::new(
                        currency,
                        match currency {
                            Currency::Eur => FX_RATE_SCALE,
                            Currency::Sek => 10 * FX_RATE_SCALE,
                            Currency::Jpy => 150 * FX_RATE_SCALE,
                            Currency::Krw => 1500 * FX_RATE_SCALE,
                            _ => multiplier * FX_RATE_SCALE / 2,
                        },
                    )
                }),
        )
        .unwrap()
    }

    fn price(amount: u64, currency: Currency) -> Price {
        Price::new(MonetaryAmount::from(amount), currency)
    }

    fn changes(
        at: OffsetDateTime,
        changes: Vec<ProductListingHistoryChange>,
    ) -> ProductListingHistoryEntry {
        ProductListingHistoryEntry {
            product_listing_id: ProductListingId::new(),
            event_id: EventId::new(),
            occurred_at: at,
            kind: ProductListingHistoryEntryKind::Changed(changes.try_into().unwrap()),
        }
    }

    fn discovery(at: OffsetDateTime) -> ProductListingHistoryEntry {
        ProductListingHistoryEntry {
            product_listing_id: ProductListingId::new(),
            event_id: EventId::new(),
            occurred_at: at,
            kind: ProductListingHistoryEntryKind::Discovered(Box::new(
                ProductListingDiscoveryHistory {
                    listing_source_id: ListingSourceId::new(),
                    source_listing_id: SourceListingId::try_from("source-1").unwrap(),
                    title: None,
                    description: None,
                    pricing: ProductListingPricing {
                        price: Some(ProductListingPrice::Monetary(price(1, Currency::Eur))),
                        price_estimate_min: Some(price(3, Currency::Eur)),
                        price_estimate_max: Some(price(5, Currency::Eur)),
                    }
                    .into(),
                    availability: None,
                    url: Url::parse("https://example.test/listing").unwrap(),
                    image_count: 0,
                    auction: None,
                },
            )),
        }
    }

    async fn execute(
        ports: &FakePorts,
        currency: Option<Currency>,
    ) -> Result<Vec<ProductListingHistoryEntry>, GetProductListingHistoryError> {
        GetProductListingHistoryHandler::new(ports.clone(), ports.clone(), ports.clone())
            .execute(
                &OperationContext {
                    principal: Principal::Anonymous,
                    request_id: application::operation_context::RequestId::new("request"),
                    correlation_id: application::operation_context::CorrelationId::new(
                        "correlation",
                    ),
                },
                GetProductListingHistoryRequest {
                    lookup: ProductListingHistoryLookup::ById(ProductListingId::new()),
                    currency,
                },
            )
            .await
    }

    fn assert_display(
        display: Option<ProductListingHistoryPriceDisplay>,
        amount: u64,
        currency: Currency,
        snapshot: &FxRateSnapshot,
    ) {
        assert_eq!(
            Some(ProductListingHistoryPriceDisplay {
                price: price(amount, currency),
                fx_rate_id: snapshot.id(),
                captured_at: snapshot.captured_at(),
            }),
            display
        );
    }

    #[tokio::test]
    async fn should_preserve_history_without_currency_and_skip_fx_reads() {
        let history = vec![discovery(OffsetDateTime::UNIX_EPOCH)];
        let ports = FakePorts {
            history: Some(history.clone()),
            fail_fx_read: true,
            ..Default::default()
        };
        assert_eq!(history, execute(&ports, None).await.unwrap());
        assert!(ports.calls.lock().unwrap().is_empty());
        assert_eq!(1, *ports.commits.lock().unwrap());
    }

    #[tokio::test]
    async fn should_batch_history_and_project_all_values_at_each_entry_timestamp() {
        let at = OffsetDateTime::UNIX_EPOCH;
        let first = snapshot(at, 3, false);
        let later = snapshot(at + Duration::hours(1), 5, false);
        let history = vec![
            discovery(at - Duration::seconds(1)),
            discovery(at),
            changes(
                at + Duration::minutes(1),
                vec![
                    ProductListingHistoryChange::MainPriceChanged {
                        previous: Some(
                            ProductListingPrice::Monetary(price(1, Currency::Eur)).into(),
                        ),
                        current: Some(
                            ProductListingPrice::Monetary(price(10, Currency::Sek)).into(),
                        ),
                    },
                    ProductListingHistoryChange::MinimumEstimateChanged {
                        previous: Some(price(3, Currency::Eur).into()),
                        current: Some(price(30, Currency::Sek).into()),
                    },
                    ProductListingHistoryChange::MaximumEstimateChanged {
                        previous: Some(price(5, Currency::Eur).into()),
                        current: Some(price(50, Currency::Sek).into()),
                    },
                    ProductListingHistoryChange::Restored,
                ],
            ),
            discovery(later.captured_at()),
            changes(
                later.captured_at() + Duration::hours(1),
                vec![ProductListingHistoryChange::Restored],
            ),
        ];
        let ports = FakePorts {
            history: Some(history.clone()),
            snapshots: vec![first.clone(), later.clone()],
            ..Default::default()
        };
        let projected = execute(&ports, Some(Currency::Usd)).await.unwrap();
        assert_eq!(
            vec![
                history
                    .iter()
                    .take(4)
                    .map(|entry| entry.occurred_at)
                    .collect::<Vec<_>>()
            ],
            *ports.calls.lock().unwrap()
        );
        assert_eq!(history[0], projected[0]);
        for (index, basis, expected) in [(1, &first, [2, 5, 8]), (3, &later, [3, 8, 13])] {
            let ProductListingHistoryEntryKind::Discovered(discovery) = &projected[index].kind
            else {
                panic!("discovery")
            };
            assert_display(
                discovery.pricing.price.unwrap().display,
                expected[0],
                Currency::Usd,
                basis,
            );
            assert_display(
                discovery.pricing.price_estimate_min.unwrap().display,
                expected[1],
                Currency::Usd,
                basis,
            );
            assert_display(
                discovery.pricing.price_estimate_max.unwrap().display,
                expected[2],
                Currency::Usd,
                basis,
            );
        }
        let ProductListingHistoryEntryKind::Changed(changes) = &projected[2].kind else {
            panic!("changes")
        };
        let ProductListingHistoryChange::MainPriceChanged { previous, current } =
            changes.as_slice()[0]
        else {
            panic!("main price")
        };
        assert_display(previous.unwrap().display, 2, Currency::Usd, &first);
        assert_display(current.unwrap().display, 2, Currency::Usd, &first);
        assert_eq!(
            ProductListingPrice::Monetary(price(10, Currency::Sek)),
            current.unwrap().source
        );
        for (change, amount) in [(&changes.as_slice()[1], 5), (&changes.as_slice()[2], 8)] {
            let (ProductListingHistoryChange::MinimumEstimateChanged { previous, current }
            | ProductListingHistoryChange::MaximumEstimateChanged { previous, current }) = change
            else {
                panic!("estimate")
            };
            assert_display(previous.unwrap().display, amount, Currency::Usd, &first);
            assert_display(current.unwrap().display, amount, Currency::Usd, &first);
        }
        assert_eq!(ProductListingHistoryChange::Restored, changes.as_slice()[3]);
        assert_eq!(history[4], projected[4]);
        assert_eq!(1, *ports.commits.lock().unwrap());
    }

    #[tokio::test]
    async fn should_apply_zero_exponents_and_preserve_same_currency_provenance() {
        let at = OffsetDateTime::UNIX_EPOCH;
        let basis = snapshot(at, 3, false);
        let ports = FakePorts {
            history: Some(vec![discovery(at)]),
            snapshots: vec![basis.clone()],
            ..Default::default()
        };
        for (currency, amount) in [(Currency::Jpy, 2), (Currency::Krw, 15), (Currency::Eur, 1)] {
            let result = execute(&ports, Some(currency)).await.unwrap();
            let ProductListingHistoryEntryKind::Discovered(discovery) = &result[0].kind else {
                panic!("discovery")
            };
            assert_display(
                discovery.pricing.price.unwrap().display,
                amount,
                currency,
                &basis,
            );
        }
    }

    #[tokio::test]
    async fn should_omit_unavailable_legacy_quotes_without_substituting_later_rates() {
        let at = OffsetDateTime::UNIX_EPOCH;
        let legacy = snapshot(at, 3, true);
        assert_eq!(19, legacy.quotes().len());
        let expanded = snapshot(at + Duration::hours(1), 5, false);
        let history = vec![discovery(at), discovery(expanded.captured_at())];
        let ports = FakePorts {
            history: Some(history.clone()),
            snapshots: vec![legacy.clone(), expanded.clone()],
            ..Default::default()
        };
        let result = execute(&ports, Some(Currency::Ron)).await.unwrap();
        assert_eq!(history[0], result[0]);
        let ProductListingHistoryEntryKind::Discovered(discovery) = &result[1].kind else {
            panic!("discovery")
        };
        assert_display(
            discovery.pricing.price.unwrap().display,
            3,
            Currency::Ron,
            &expanded,
        );
        assert_eq!(
            None,
            display_price(price(1, Currency::Sek), Some(&legacy), Currency::Usd).unwrap()
        );
        assert_eq!(
            None,
            display_price(price(1, Currency::Ron), Some(&legacy), Currency::Ron).unwrap()
        );
    }

    #[tokio::test]
    async fn should_preserve_on_request_nulls_and_entries_without_snapshots() {
        let at = OffsetDateTime::UNIX_EPOCH;
        let mut history = vec![changes(
            at,
            vec![
                ProductListingHistoryChange::MainPriceChanged {
                    previous: None,
                    current: Some(ProductListingPrice::OnRequest.into()),
                },
                ProductListingHistoryChange::MinimumEstimateChanged {
                    previous: None,
                    current: None,
                },
                ProductListingHistoryChange::Restored,
            ],
        )];
        let mut unpriced_discovery = discovery(at);
        let ProductListingHistoryEntryKind::Discovered(discovery) = &mut unpriced_discovery.kind
        else {
            panic!("discovery")
        };
        discovery.pricing = ProductListingHistoryPricing {
            price: Some(ProductListingPrice::OnRequest.into()),
            ..Default::default()
        };
        history.push(unpriced_discovery);
        for snapshots in [vec![], vec![snapshot(at, 3, false)]] {
            let ports = FakePorts {
                history: Some(history.clone()),
                snapshots,
                fail_fx_read: true,
                ..Default::default()
            };
            assert_eq!(history, execute(&ports, Some(Currency::Ron)).await.unwrap());
            assert!(ports.calls.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn should_propagate_fx_read_failure_and_conversion_overflow() {
        use std::error::Error;
        let at = OffsetDateTime::UNIX_EPOCH;
        let ports = FakePorts {
            history: Some(vec![discovery(at)]),
            fail_fx_read: true,
            ..Default::default()
        };
        let error = execute(&ports, Some(Currency::Usd)).await.unwrap_err();
        assert!(matches!(
            error,
            GetProductListingHistoryError::FxSnapshotUnavailable { .. }
        ));
        assert!(error.source().unwrap().source().is_some());
        assert!(matches!(
            display_price(
                price(u64::MAX, Currency::Eur),
                Some(&snapshot(at, 5, false)),
                Currency::Usd
            ),
            Err(GetProductListingHistoryError::PriceConversionFailed(
                FxRateSnapshotError::ConversionOverflow
            ))
        ));
    }

    #[tokio::test]
    async fn should_skip_fx_reads_for_empty_history_and_not_found() {
        let empty = FakePorts {
            history: Some(vec![]),
            ..Default::default()
        };
        assert!(
            execute(&empty, Some(Currency::Usd))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(empty.calls.lock().unwrap().is_empty());
        let missing = FakePorts::default();
        assert!(matches!(
            execute(&missing, Some(Currency::Usd)).await,
            Err(GetProductListingHistoryError::NotFound)
        ));
        assert!(missing.calls.lock().unwrap().is_empty());
        assert_eq!(0, *missing.commits.lock().unwrap());
    }

    #[test]
    fn should_reject_an_empty_history_change_set() {
        let result = ProductListingHistoryChanges::try_from(Vec::new());

        assert!(matches!(
            result,
            Err(EmptyProductListingHistoryChangesError)
        ));
    }
}
