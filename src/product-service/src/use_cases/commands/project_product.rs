use crate::ports::{
    ProductSearchFilterMatchSource, ProductSearchFilterMatchSourceReadError,
    ProductSearchFilterMatchSourceReader, ProductSearchFilterMatchSourceReaderFactory,
    ProductSearchProjection, ProductSearchProjectionWriteOutcome,
};
use application::error::{BoxError, box_error};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use fxrate_core::FxRateSnapshot;
use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use product_core::{product_id::ProductId, product_lifecycle::ProductLifecycle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectProductCommand {
    pub event_id: EventId,
    pub product_id: ProductId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectProductOutcome {
    Applied,
    Stale,
    MissingSource,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectProductResult {
    pub outcome: ProjectProductOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectProductError {
    #[error("failed to begin Product projection source transaction")]
    BeginFailed {
        #[source]
        source: BoxError,
    },
    #[error("Product projection source read failed")]
    SourceReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("Product projection sale FX snapshot is missing")]
    SaleFxSnapshotMissing,
    #[error("Product projection sale FX snapshot is invalid")]
    SaleFxSnapshotInvalid {
        #[source]
        source: BoxError,
    },
    #[error("Product projection sale FX snapshot read failed")]
    SaleFxSnapshotReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to commit Product projection source transaction")]
    CommitFailed {
        #[source]
        source: BoxError,
    },
    #[error("Product projection target write failed")]
    WriteFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProjectProductUseCase: Send + Sync {
    async fn execute(
        &self,
        command: ProjectProductCommand,
    ) -> Result<ProjectProductResult, ProjectProductError>;
}

pub struct ProjectProductHandler<U, S, F, P> {
    unit_of_work: U,
    sources: S,
    fx_rates: F,
    projection: P,
}

impl<U, S, F, P> ProjectProductHandler<U, S, F, P> {
    pub fn new(unit_of_work: U, sources: S, fx_rates: F, projection: P) -> Self {
        Self {
            unit_of_work,
            sources,
            fx_rates,
            projection,
        }
    }
}

#[async_trait::async_trait]
impl<U, S, F, P> ProjectProductUseCase for ProjectProductHandler<U, S, F, P>
where
    U: UnitOfWork,
    S: ProductSearchFilterMatchSourceReaderFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
    P: ProductSearchProjection,
{
    #[tracing::instrument(name = "project_product", skip_all, fields(product_id = %command.product_id, event_id = %command.event_id))]
    async fn execute(
        &self,
        command: ProjectProductCommand,
    ) -> Result<ProjectProductResult, ProjectProductError> {
        let mut tx =
            self.unit_of_work
                .begin()
                .await
                .map_err(|source| ProjectProductError::BeginFailed {
                    source: box_error(source),
                })?;
        let source = self
            .sources
            .in_transaction(&mut tx)
            .find_source(command.event_id, command.product_id)
            .await
            .map_err(source_read_error)?;
        let Some(source) = source else {
            tx.commit()
                .await
                .map_err(|source| ProjectProductError::CommitFailed {
                    source: box_error(source),
                })?;
            return Ok(ProjectProductResult {
                outcome: ProjectProductOutcome::MissingSource,
            });
        };
        if source.current_event_id != command.event_id {
            tx.commit()
                .await
                .map_err(|source| ProjectProductError::CommitFailed {
                    source: box_error(source),
                })?;
            return Ok(ProjectProductResult {
                outcome: ProjectProductOutcome::Stale,
            });
        }
        let sale_snapshot = load_sale_snapshot(&mut tx, &self.fx_rates, &source).await?;
        tx.commit()
            .await
            .map_err(|source| ProjectProductError::CommitFailed {
                source: box_error(source),
            })?;

        let outcome = if source.lifecycle == ProductLifecycle::Deleted {
            self.projection
                .delete(source.product_id, source.projection_version)
                .await
        } else {
            self.projection
                .upsert(&source, sale_snapshot.as_ref())
                .await
        }
        .map_err(|source| ProjectProductError::WriteFailed {
            source: box_error(source),
        })?;
        Ok(ProjectProductResult {
            outcome: match (source.lifecycle, outcome) {
                (ProductLifecycle::Deleted, ProductSearchProjectionWriteOutcome::Applied) => {
                    ProjectProductOutcome::Deleted
                }
                (_, ProductSearchProjectionWriteOutcome::Applied) => ProjectProductOutcome::Applied,
                (_, ProductSearchProjectionWriteOutcome::Stale) => ProjectProductOutcome::Stale,
            },
        })
    }
}

async fn load_sale_snapshot<Tx, F>(
    tx: &mut Tx,
    fx_rates: &F,
    source: &ProductSearchFilterMatchSource,
) -> Result<Option<FxRateSnapshot>, ProjectProductError>
where
    F: FxRateSnapshotRepositoryFactory<Tx>,
{
    let Some(valuation) = source.sale_valuation else {
        return Ok(None);
    };
    if source.pricing.price.is_none() {
        return Ok(None);
    }
    fx_rates
        .in_transaction(tx)
        .find_by_id(valuation.fx_rate_id)
        .await
        .map_err(fx_rate_error)?
        .ok_or(ProjectProductError::SaleFxSnapshotMissing)
        .map(Some)
}

fn source_read_error(error: ProductSearchFilterMatchSourceReadError) -> ProjectProductError {
    ProjectProductError::SourceReadFailed {
        source: match error {
            ProductSearchFilterMatchSourceReadError::QueryFailed { source }
            | ProductSearchFilterMatchSourceReadError::InvalidPersistedState { source } => source,
        },
    }
}

fn fx_rate_error(error: FxRateSnapshotRepositoryError) -> ProjectProductError {
    match error {
        FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
            ProjectProductError::SaleFxSnapshotInvalid { source }
        }
        FxRateSnapshotRepositoryError::InsertFailed { source }
        | FxRateSnapshotRepositoryError::ReadFailed { source } => {
            ProjectProductError::SaleFxSnapshotReadFailed { source }
        }
        FxRateSnapshotRepositoryError::CapturedAtNotMonotonic => {
            ProjectProductError::SaleFxSnapshotMissing
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        ProductSearchFilterMatchShopType, ProductSearchFilterMatchSourceEventKind,
        ProductSearchFilterMatchSourceRef, ProductSearchProjectionWriteError,
    };
    use application::error::box_error;
    use application::transaction::TransactionError;
    use fxrate_core::{
        FX_RATE_SCALE, FxRateGeneration, FxRateId, FxRateQuote, FxRateSource, NewFxRateSnapshot,
    };
    use indexmap::IndexSet;
    use localization::{Language, Localized};
    use money::Currency;
    use product_core::{
        description::Description,
        product::{ProductAddress, ProductAuction, ProductPricing, ProductSaleValuation},
        product_slug_id::ProductSlugId,
        product_state::ProductState,
        shops_product_id::ShopsProductId,
        title::Title,
    };
    use shop_core::seller_slug_id::SellerSlugId;
    use shop_core::shop_id::ShopId;
    use shop_core::shop_name::ShopName;
    use shop_core::shop_slug_id::ShopSlugId;
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex, MutexGuard},
    };
    use strum::IntoEnumIterator;
    use time::OffsetDateTime;
    use url::Url;

    #[derive(Default)]
    struct FakeState {
        source_result: Option<
            Result<Option<ProductSearchFilterMatchSource>, ProductSearchFilterMatchSourceReadError>,
        >,
        source_requests: Vec<(EventId, ProductId)>,
        fx_result: Option<Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError>>,
        fx_lookup_ids: Vec<FxRateId>,
        commit_count: usize,
        upserts: Vec<(ProductId, Option<FxRateSnapshot>)>,
        deletes: Vec<(ProductId, i64)>,
        write_commit_counts: Vec<usize>,
        projection_result:
            Option<Result<ProductSearchProjectionWriteOutcome, ProductSearchProjectionWriteError>>,
    }

    type SharedState = Arc<Mutex<FakeState>>;

    #[derive(Clone)]
    struct FakeUnitOfWork {
        state: SharedState,
    }

    struct FakeTx {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeSourceFactory {
        state: SharedState,
    }

    struct FakeSourceReader {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeFxRateSnapshotFactory {
        state: SharedState,
    }

    struct FakeFxRateSnapshotRepository {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeProjection {
        state: SharedState,
    }

    fn state() -> SharedState {
        Arc::new(Mutex::new(FakeState::default()))
    }

    fn lock_state(state: &SharedState) -> MutexGuard<'_, FakeState> {
        match state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
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

    impl ProductSearchFilterMatchSourceReaderFactory<FakeTx> for FakeSourceFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl ProductSearchFilterMatchSourceReader + 'tx {
            FakeSourceReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductSearchFilterMatchSourceReader for FakeSourceReader {
        async fn find_source(
            &mut self,
            event_id: EventId,
            product_id: ProductId,
        ) -> Result<Option<ProductSearchFilterMatchSource>, ProductSearchFilterMatchSourceReadError>
        {
            let mut state = lock_state(&self.state);
            state.source_requests.push((event_id, product_id));
            match state.source_result.take() {
                Some(result) => result,
                None => Ok(None),
            }
        }

        async fn find_sources(
            &mut self,
            refs: &[ProductSearchFilterMatchSourceRef],
        ) -> Result<
            HashMap<ProductSearchFilterMatchSourceRef, ProductSearchFilterMatchSource>,
            ProductSearchFilterMatchSourceReadError,
        > {
            let mut sources = HashMap::new();
            for reference in refs {
                if let Some(source) = self
                    .find_source(reference.event_id, reference.product_id)
                    .await?
                {
                    sources.insert(*reference, source);
                }
            }
            Ok(sources)
        }
    }

    impl FxRateSnapshotRepositoryFactory<FakeTx> for FakeFxRateSnapshotFactory {
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
    impl FxRateSnapshotRepository for FakeFxRateSnapshotRepository {
        async fn find_latest(
            &mut self,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_latest_at_or_before(
            &mut self,
            _timestamp: time::OffsetDateTime,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_by_id(
            &mut self,
            id: FxRateId,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            let mut state = lock_state(&self.state);
            state.fx_lookup_ids.push(id);
            match state.fx_result.take() {
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
            _snapshot: &NewFxRateSnapshot,
            _source_event_id: &str,
        ) -> Result<fxrate_service::ports::FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError>
        {
            Ok(fxrate_service::ports::FxRateSnapshotInsertOutcome::Duplicate)
        }
    }

    #[async_trait::async_trait]
    impl ProductSearchProjection for FakeProjection {
        async fn upsert(
            &self,
            source: &ProductSearchFilterMatchSource,
            sale_snapshot: Option<&FxRateSnapshot>,
        ) -> Result<ProductSearchProjectionWriteOutcome, ProductSearchProjectionWriteError>
        {
            let mut state = lock_state(&self.state);
            let commit_count = state.commit_count;
            state
                .upserts
                .push((source.product_id, sale_snapshot.cloned()));
            state.write_commit_counts.push(commit_count);
            match state.projection_result.take() {
                Some(result) => result,
                None => Ok(ProductSearchProjectionWriteOutcome::Applied),
            }
        }

        async fn delete(
            &self,
            product_id: ProductId,
            source_version: i64,
        ) -> Result<ProductSearchProjectionWriteOutcome, ProductSearchProjectionWriteError>
        {
            let mut state = lock_state(&self.state);
            let commit_count = state.commit_count;
            state.deletes.push((product_id, source_version));
            state.write_commit_counts.push(commit_count);
            match state.projection_result.take() {
                Some(result) => result,
                None => Ok(ProductSearchProjectionWriteOutcome::Applied),
            }
        }
    }

    fn handler(
        state: &SharedState,
    ) -> ProjectProductHandler<
        FakeUnitOfWork,
        FakeSourceFactory,
        FakeFxRateSnapshotFactory,
        FakeProjection,
    > {
        ProjectProductHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(state),
            },
            FakeSourceFactory {
                state: Arc::clone(state),
            },
            FakeFxRateSnapshotFactory {
                state: Arc::clone(state),
            },
            FakeProjection {
                state: Arc::clone(state),
            },
        )
    }

    fn source() -> Result<ProductSearchFilterMatchSource, url::ParseError> {
        let event_id = EventId::new();
        let url = Url::parse("https://shop.example.test/products/cabinet")?;
        let title = Title::from("Cabinet");
        Ok(ProductSearchFilterMatchSource {
            event_id,
            event_kind: ProductSearchFilterMatchSourceEventKind::Domain,
            origin_event_time: OffsetDateTime::UNIX_EPOCH,
            current_event_id: event_id,
            projection_version: 41,
            product_id: ProductId::new(),
            product_slug_id: ProductSlugId::from("cabinet"),
            shop_id: ShopId::new(),
            shop_slug_id: ShopSlugId::from("shop"),
            shop_name: ShopName::from("Shop"),
            shop_type: ProductSearchFilterMatchShopType::Marketplace,
            seller_id: ShopId::new(),
            seller_slug_id: SellerSlugId::from(ShopSlugId::from("seller")),
            seller_name: ShopName::from("Seller"),
            shops_product_id: ShopsProductId::from("cabinet-1"),
            address: ProductAddress::default(),
            product_title: Some(Localized::new(Language::En, title.clone())),
            product_description: Some(Localized::new(
                Language::En,
                Description::from("Old cabinet"),
            )),
            titles: HashMap::from([(Language::En, title)]),
            descriptions: HashMap::new(),
            pricing: ProductPricing::default(),
            sale_valuation: None,
            state: ProductState::Listed,
            lifecycle: ProductLifecycle::Active,
            url: url.clone(),
            view_url: url,
            image: None,
            images: IndexSet::new(),
            embedding: None,
            auction: ProductAuction::default(),
            created: time::OffsetDateTime::UNIX_EPOCH,
            updated: time::OffsetDateTime::UNIX_EPOCH,
        })
    }

    fn snapshot() -> Result<FxRateSnapshot, fxrate_core::FxRateSnapshotError> {
        NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            time::OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| FxRateQuote::new(currency, FX_RATE_SCALE)),
        )
        .and_then(|snapshot| Ok(snapshot.into_persisted(FxRateGeneration::try_from(1)?)))
    }

    fn command(source: &ProductSearchFilterMatchSource) -> ProjectProductCommand {
        ProjectProductCommand {
            event_id: source.event_id,
            product_id: source.product_id,
        }
    }

    #[tokio::test]
    async fn should_upsert_active_source_without_fx_snapshot_after_commit()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let source = source()?;
        let command = command(&source);
        lock_state(&state).source_result = Some(Ok(Some(source.clone())));

        let result = handler(&state).execute(command).await?;

        assert_eq!(ProjectProductOutcome::Applied, result.outcome);
        let state = lock_state(&state);
        assert_eq!(
            vec![(command.event_id, command.product_id)],
            state.source_requests
        );
        assert!(state.fx_lookup_ids.is_empty());
        assert_eq!(vec![(source.product_id, None)], state.upserts);
        assert_eq!(vec![1], state.write_commit_counts);
        Ok(())
    }

    #[tokio::test]
    async fn should_use_exact_sale_fx_snapshot_when_source_is_sold()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let snapshot = snapshot()?;
        let mut source = source()?;
        source.pricing.price = Some(money::Price::new(100_u64.into(), Currency::Eur));
        source.sale_valuation = Some(ProductSaleValuation {
            sold_at: time::OffsetDateTime::UNIX_EPOCH,
            fx_rate_id: snapshot.id(),
        });
        let command = command(&source);
        {
            let mut state = lock_state(&state);
            state.source_result = Some(Ok(Some(source.clone())));
            state.fx_result = Some(Ok(Some(snapshot.clone())));
        }

        let result = handler(&state).execute(command).await?;

        assert_eq!(ProjectProductOutcome::Applied, result.outcome);
        let state = lock_state(&state);
        assert_eq!(vec![snapshot.id()], state.fx_lookup_ids);
        assert_eq!(vec![(source.product_id, Some(snapshot))], state.upserts);
        assert_eq!(vec![1], state.write_commit_counts);
        Ok(())
    }

    #[tokio::test]
    async fn should_project_sold_source_without_price_without_fx_lookup()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let mut source = source()?;
        source.sale_valuation = Some(ProductSaleValuation {
            sold_at: time::OffsetDateTime::UNIX_EPOCH,
            fx_rate_id: FxRateId::new(),
        });
        let command = command(&source);
        lock_state(&state).source_result = Some(Ok(Some(source.clone())));

        let result = handler(&state).execute(command).await?;

        assert_eq!(ProjectProductOutcome::Applied, result.outcome);
        let state = lock_state(&state);
        assert!(state.fx_lookup_ids.is_empty());
        assert_eq!(vec![(source.product_id, None)], state.upserts);
        assert_eq!(vec![1], state.write_commit_counts);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_or_write_when_sale_fx_snapshot_is_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let snapshot = snapshot()?;
        let mut source = source()?;
        source.pricing.price = Some(money::Price::new(100_u64.into(), Currency::Eur));
        source.sale_valuation = Some(ProductSaleValuation {
            sold_at: time::OffsetDateTime::UNIX_EPOCH,
            fx_rate_id: snapshot.id(),
        });
        let command = command(&source);
        {
            let mut state = lock_state(&state);
            state.source_result = Some(Ok(Some(source)));
            state.fx_result = Some(Ok(None));
        }

        let result = handler(&state).execute(command).await;

        assert!(matches!(
            result,
            Err(ProjectProductError::SaleFxSnapshotMissing)
        ));
        let state = lock_state(&state);
        assert_eq!(0, state.commit_count);
        assert!(state.upserts.is_empty());
        assert!(state.deletes.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_map_invalid_persisted_sale_snapshot() -> Result<(), Box<dyn std::error::Error>>
    {
        let state = state();
        let snapshot = snapshot()?;
        let mut source = source()?;
        source.pricing.price = Some(money::Price::new(100_u64.into(), Currency::Eur));
        source.sale_valuation = Some(ProductSaleValuation {
            sold_at: time::OffsetDateTime::UNIX_EPOCH,
            fx_rate_id: snapshot.id(),
        });
        let command = command(&source);
        {
            let mut state = lock_state(&state);
            state.source_result = Some(Ok(Some(source)));
            state.fx_result = Some(Err(
                FxRateSnapshotRepositoryError::InvalidPersistedSnapshot {
                    source: box_error(std::io::Error::other("invalid snapshot")),
                },
            ));
        }

        let result = handler(&state).execute(command).await;

        assert!(matches!(
            result,
            Err(ProjectProductError::SaleFxSnapshotInvalid { .. })
        ));
        let state = lock_state(&state);
        assert_eq!(0, state.commit_count);
        assert!(state.upserts.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_commit_without_fx_lookup_or_write_when_source_is_stale()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let mut source = source()?;
        let command = command(&source);
        source.current_event_id = EventId::new();
        lock_state(&state).source_result = Some(Ok(Some(source)));

        let result = handler(&state).execute(command).await?;

        assert_eq!(ProjectProductOutcome::Stale, result.outcome);
        let state = lock_state(&state);
        assert_eq!(1, state.commit_count);
        assert!(state.fx_lookup_ids.is_empty());
        assert!(state.upserts.is_empty());
        assert!(state.deletes.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_commit_without_write_when_source_is_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let command = ProjectProductCommand {
            event_id: EventId::new(),
            product_id: ProductId::new(),
        };
        lock_state(&state).source_result = Some(Ok(None));

        let result = handler(&state).execute(command).await?;

        assert_eq!(ProjectProductOutcome::MissingSource, result.outcome);
        let state = lock_state(&state);
        assert_eq!(1, state.commit_count);
        assert!(state.fx_lookup_ids.is_empty());
        assert!(state.upserts.is_empty());
        assert!(state.deletes.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_delete_exact_source_version_when_deleted_projection_is_applied()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let mut source = source()?;
        source.lifecycle = ProductLifecycle::Deleted;
        let command = command(&source);
        lock_state(&state).source_result = Some(Ok(Some(source.clone())));

        let result = handler(&state).execute(command).await?;

        assert_eq!(ProjectProductOutcome::Deleted, result.outcome);
        let state = lock_state(&state);
        assert_eq!(
            vec![(source.product_id, source.projection_version)],
            state.deletes
        );
        assert!(state.upserts.is_empty());
        assert_eq!(vec![1], state.write_commit_counts);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_stale_when_deleted_projection_write_is_stale()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let mut source = source()?;
        source.lifecycle = ProductLifecycle::Deleted;
        let command = command(&source);
        {
            let mut state = lock_state(&state);
            state.source_result = Some(Ok(Some(source)));
            state.projection_result = Some(Ok(ProductSearchProjectionWriteOutcome::Stale));
        }

        let result = handler(&state).execute(command).await?;

        assert_eq!(ProjectProductOutcome::Stale, result.outcome);
        let state = lock_state(&state);
        assert_eq!(1, state.commit_count);
        assert_eq!(1, state.deletes.len());
        Ok(())
    }

    #[tokio::test]
    async fn should_map_target_write_error_after_source_transaction_commits()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let source = source()?;
        let command = command(&source);
        {
            let mut state = lock_state(&state);
            state.source_result = Some(Ok(Some(source)));
            state.projection_result = Some(Err(ProductSearchProjectionWriteError::WriteFailed {
                source: box_error(std::io::Error::other("target unavailable")),
            }));
        }

        let result = handler(&state).execute(command).await;

        assert!(matches!(
            result,
            Err(ProjectProductError::WriteFailed { .. })
        ));
        let state = lock_state(&state);
        assert_eq!(1, state.commit_count);
        assert_eq!(vec![1], state.write_commit_counts);
        Ok(())
    }
}
