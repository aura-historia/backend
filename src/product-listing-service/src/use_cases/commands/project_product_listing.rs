use crate::ports::{
    ProductListingSearchFilterMatchSourceReadError, ProductListingSearchFilterMatchSourceReader,
    ProductListingSearchFilterMatchSourceReaderFactory, ProductListingSearchProjection,
    ProductListingSearchProjectionWriteOutcome,
};
use application::error::{BoxError, box_error};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use product_listing_core::{
    listing_discoverability::is_discoverable, product_listing_id::ProductListingId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectProductListingCommand {
    pub event_id: EventId,
    pub product_listing_id: ProductListingId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectProductListingOutcome {
    Applied,
    Stale,
    MissingSource,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectProductListingResult {
    pub outcome: ProjectProductListingOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectProductListingError {
    #[error("failed to begin ProductListing projection source transaction")]
    BeginFailed {
        #[source]
        source: BoxError,
    },
    #[error("ProductListing projection source read failed")]
    SourceReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("ProductListing projection source persisted state is invalid")]
    SourceStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("failed to commit ProductListing projection source transaction")]
    CommitFailed {
        #[source]
        source: BoxError,
    },
    #[error("ProductListing projection target write failed")]
    WriteFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProjectProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        command: ProjectProductListingCommand,
    ) -> Result<ProjectProductListingResult, ProjectProductListingError>;
}

pub struct ProjectProductListingHandler<U, S, P> {
    unit_of_work: U,
    sources: S,
    projection: P,
}

impl<U, S, P> ProjectProductListingHandler<U, S, P> {
    pub fn new(unit_of_work: U, sources: S, projection: P) -> Self {
        Self {
            unit_of_work,
            sources,
            projection,
        }
    }
}

#[async_trait::async_trait]
impl<U, S, P> ProjectProductListingUseCase for ProjectProductListingHandler<U, S, P>
where
    U: UnitOfWork,
    S: ProductListingSearchFilterMatchSourceReaderFactory<U::Tx>,
    P: ProductListingSearchProjection,
{
    #[tracing::instrument(name = "project_product", skip_all, fields(product_listing_id = %command.product_listing_id, event_id = %command.event_id))]
    async fn execute(
        &self,
        command: ProjectProductListingCommand,
    ) -> Result<ProjectProductListingResult, ProjectProductListingError> {
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            ProjectProductListingError::BeginFailed {
                source: box_error(source),
            }
        })?;
        let source = self
            .sources
            .in_transaction(&mut tx)
            .find_source(command.event_id, command.product_listing_id)
            .await
            .map_err(source_read_error)?;
        tx.commit()
            .await
            .map_err(|source| ProjectProductListingError::CommitFailed {
                source: box_error(source),
            })?;
        let Some(source) = source else {
            return Ok(ProjectProductListingResult {
                outcome: ProjectProductListingOutcome::MissingSource,
            });
        };
        if source.current_event_id != command.event_id {
            return Ok(ProjectProductListingResult {
                outcome: ProjectProductListingOutcome::Stale,
            });
        }

        // Public discovery offers only listings a buyer can still pursue; every other state is
        // the same versioned tombstone as a withdrawal (docs/product-listing.md).
        let discoverable = is_discoverable(source.lifecycle, source.availability);
        let outcome = if discoverable {
            self.projection.upsert(&source).await
        } else {
            self.projection
                .delete(source.product_listing_id, source.projection_version)
                .await
        }
        .map_err(|source| ProjectProductListingError::WriteFailed {
            source: box_error(source),
        })?;
        Ok(ProjectProductListingResult {
            outcome: match (discoverable, outcome) {
                (false, ProductListingSearchProjectionWriteOutcome::Applied) => {
                    ProjectProductListingOutcome::Deleted
                }
                (true, ProductListingSearchProjectionWriteOutcome::Applied) => {
                    ProjectProductListingOutcome::Applied
                }
                (_, ProductListingSearchProjectionWriteOutcome::Stale) => {
                    ProjectProductListingOutcome::Stale
                }
            },
        })
    }
}

fn source_read_error(
    error: ProductListingSearchFilterMatchSourceReadError,
) -> ProjectProductListingError {
    match error {
        ProductListingSearchFilterMatchSourceReadError::QueryFailed { source } => {
            ProjectProductListingError::SourceReadFailed { source }
        }
        ProductListingSearchFilterMatchSourceReadError::InvalidPersistedState { source } => {
            ProjectProductListingError::SourceStateInvalid { source }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        ListingSourceSummary, ProductListingSearchFilterMatchSource,
        ProductListingSearchFilterMatchSourceEventKind, ProductListingSearchFilterMatchSourceRef,
        ProductListingSearchProjectionWriteError,
    };
    use application::error::box_error;
    use application::transaction::TransactionError;
    use indexmap::IndexSet;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use localization::{Language, Localized};
    use product_listing_core::{
        description::Description, listing_availability::ListingAvailability,
        listing_lifecycle::ListingLifecycle, product_listing::ProductListingPricing,
        product_listing_slug_id::ProductListingSlugId, source_listing_id::SourceListingId,
        title::Title,
    };
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
            Result<
                Option<ProductListingSearchFilterMatchSource>,
                ProductListingSearchFilterMatchSourceReadError,
            >,
        >,
        source_requests: Vec<(EventId, ProductListingId)>,
        commit_count: usize,
        upserts: Vec<(ProductListingId, i64)>,
        deletes: Vec<(ProductListingId, i64)>,
        write_commit_counts: Vec<usize>,
        projection_result: Option<
            Result<
                ProductListingSearchProjectionWriteOutcome,
                ProductListingSearchProjectionWriteError,
            >,
        >,
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

    impl ProductListingSearchFilterMatchSourceReaderFactory<FakeTx> for FakeSourceFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl ProductListingSearchFilterMatchSourceReader + 'tx {
            FakeSourceReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductListingSearchFilterMatchSourceReader for FakeSourceReader {
        async fn find_source(
            &mut self,
            event_id: EventId,
            product_listing_id: ProductListingId,
        ) -> Result<
            Option<ProductListingSearchFilterMatchSource>,
            ProductListingSearchFilterMatchSourceReadError,
        > {
            let mut state = lock_state(&self.state);
            state.source_requests.push((event_id, product_listing_id));
            match state.source_result.take() {
                Some(result) => result,
                None => Ok(None),
            }
        }

        async fn find_sources(
            &mut self,
            refs: &[ProductListingSearchFilterMatchSourceRef],
        ) -> Result<
            HashMap<
                ProductListingSearchFilterMatchSourceRef,
                ProductListingSearchFilterMatchSource,
            >,
            ProductListingSearchFilterMatchSourceReadError,
        > {
            let mut sources = HashMap::new();
            for reference in refs {
                if let Some(source) = self
                    .find_source(reference.event_id, reference.product_listing_id)
                    .await?
                {
                    sources.insert(*reference, source);
                }
            }
            Ok(sources)
        }
    }

    #[async_trait::async_trait]
    impl ProductListingSearchProjection for FakeProjection {
        async fn upsert(
            &self,
            source: &ProductListingSearchFilterMatchSource,
        ) -> Result<
            ProductListingSearchProjectionWriteOutcome,
            ProductListingSearchProjectionWriteError,
        > {
            let mut state = lock_state(&self.state);
            let commit_count = state.commit_count;
            state
                .upserts
                .push((source.product_listing_id, source.projection_version));
            state.write_commit_counts.push(commit_count);
            match state.projection_result.take() {
                Some(result) => result,
                None => Ok(ProductListingSearchProjectionWriteOutcome::Applied),
            }
        }

        async fn delete(
            &self,
            product_listing_id: ProductListingId,
            source_version: i64,
        ) -> Result<
            ProductListingSearchProjectionWriteOutcome,
            ProductListingSearchProjectionWriteError,
        > {
            let mut state = lock_state(&self.state);
            let commit_count = state.commit_count;
            state.deletes.push((product_listing_id, source_version));
            state.write_commit_counts.push(commit_count);
            match state.projection_result.take() {
                Some(result) => result,
                None => Ok(ProductListingSearchProjectionWriteOutcome::Applied),
            }
        }
    }

    fn handler(
        state: &SharedState,
    ) -> ProjectProductListingHandler<FakeUnitOfWork, FakeSourceFactory, FakeProjection> {
        ProjectProductListingHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(state),
            },
            FakeSourceFactory {
                state: Arc::clone(state),
            },
            FakeProjection {
                state: Arc::clone(state),
            },
        )
    }

    fn source() -> Result<ProductListingSearchFilterMatchSource, url::ParseError> {
        let event_id = EventId::new();
        let url = Url::parse("https://shop.example.test/products/cabinet")?;
        let title = Title::from("Cabinet");
        Ok(ProductListingSearchFilterMatchSource {
            event_id,
            event_kind: ProductListingSearchFilterMatchSourceEventKind::Domain,
            origin_event_time: OffsetDateTime::UNIX_EPOCH,
            current_event_id: event_id,
            projection_version: 41,
            shipping: Default::default(),
            geography: Default::default(),
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::raw("cabinet-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            source: ListingSourceSummary {
                listing_source_id: ListingSourceId::new(),
                name: ListingSourceName::try_from("Source")
                    .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
                slug_id: ListingSourceSlugId::raw("source")
                    .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
                operator: crate::ports::ListingSourceOperatorSummary {
                    name: party_core::party_name::PartyName::try_from("Operator")
                        .unwrap_or_else(|error| panic!("valid test operator name: {error}")),
                    partner: false,
                },
            },
            source_listing_id: SourceListingId::try_from("cabinet-1")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            product_title: Some(Localized::new(Language::En, title.clone())),
            product_description: Some(Localized::new(
                Language::En,
                Description::from("Old cabinet"),
            )),
            titles: HashMap::from([(Language::En, title)]),
            descriptions: HashMap::new(),
            pricing: ProductListingPricing::default(),
            sale_observation: None,
            availability: None,
            lifecycle: ListingLifecycle::Active,
            url: url.clone(),
            view_link: listing_source_core::ViewLink::non_affiliate(url),
            image: None,
            images: IndexSet::new(),
            embedding: None,
            auction: None,
            created: time::OffsetDateTime::UNIX_EPOCH,
            updated: time::OffsetDateTime::UNIX_EPOCH,
        })
    }

    fn command(source: &ProductListingSearchFilterMatchSource) -> ProjectProductListingCommand {
        ProjectProductListingCommand {
            event_id: source.event_id,
            product_listing_id: source.product_listing_id,
        }
    }

    #[tokio::test]
    async fn should_upsert_active_source_without_availability_after_commit()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let source = source()?;
        let command = command(&source);
        lock_state(&state).source_result = Some(Ok(Some(source.clone())));

        let result = handler(&state).execute(command).await?;

        assert_eq!(ProjectProductListingOutcome::Applied, result.outcome);
        let state = lock_state(&state);
        assert_eq!(
            vec![(command.event_id, command.product_listing_id)],
            state.source_requests
        );
        assert_eq!(
            vec![(source.product_listing_id, source.projection_version)],
            state.upserts
        );
        assert!(state.deletes.is_empty());
        assert_eq!(vec![1], state.write_commit_counts);
        assert_eq!(1, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_index_active_source_only_while_its_availability_is_discoverable()
    -> Result<(), Box<dyn std::error::Error>> {
        for availability in ListingAvailability::iter() {
            let state = state();
            let mut source = source()?;
            source.availability = Some(availability);
            lock_state(&state).source_result = Some(Ok(Some(source.clone())));

            let result = handler(&state).execute(command(&source)).await?;

            let state = lock_state(&state);
            let written = vec![(source.product_listing_id, source.projection_version)];
            if availability.is_discoverable() {
                assert_eq!(ProjectProductListingOutcome::Applied, result.outcome);
                assert_eq!(written, state.upserts, "{availability:?}");
                assert!(state.deletes.is_empty(), "{availability:?}");
            } else {
                assert_eq!(ProjectProductListingOutcome::Deleted, result.outcome);
                assert_eq!(written, state.deletes, "{availability:?}");
                assert!(state.upserts.is_empty(), "{availability:?}");
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_delete_then_reindex_when_availability_leaves_and_returns()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let handler = handler(&state);
        let mut source = source()?;
        let mut outcomes = Vec::new();
        for availability in [
            ListingAvailability::InStock,
            ListingAvailability::OutOfStock,
            ListingAvailability::InStock,
            ListingAvailability::SoldOut,
            ListingAvailability::Reserved,
        ] {
            source.availability = Some(availability);
            source.projection_version += 1;
            source.event_id = EventId::new();
            source.current_event_id = source.event_id;
            lock_state(&state).source_result = Some(Ok(Some(source.clone())));
            outcomes.push(handler.execute(command(&source)).await?.outcome);
        }

        assert_eq!(
            vec![
                ProjectProductListingOutcome::Applied,
                ProjectProductListingOutcome::Deleted,
                ProjectProductListingOutcome::Applied,
                ProjectProductListingOutcome::Deleted,
                ProjectProductListingOutcome::Applied,
            ],
            outcomes
        );
        let state = lock_state(&state);
        let id = source.product_listing_id;
        assert_eq!(vec![(id, 42), (id, 44), (id, 46)], state.upserts);
        assert_eq!(vec![(id, 43), (id, 45)], state.deletes);
        Ok(())
    }

    #[tokio::test]
    async fn should_keep_invalid_persisted_source_state_apart_from_a_failed_source_read()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = source()?;
        for (read_error, invalid) in [
            (
                ProductListingSearchFilterMatchSourceReadError::InvalidPersistedState {
                    source: box_error(std::io::Error::other("invalid source")),
                },
                true,
            ),
            (
                ProductListingSearchFilterMatchSourceReadError::QueryFailed {
                    source: box_error(std::io::Error::other("query failed")),
                },
                false,
            ),
        ] {
            let state = state();
            lock_state(&state).source_result = Some(Err(read_error));

            let result = handler(&state).execute(command(&source)).await;

            match result {
                Err(ProjectProductListingError::SourceStateInvalid { .. }) => assert!(invalid),
                Err(ProjectProductListingError::SourceReadFailed { .. }) => assert!(!invalid),
                other => panic!("unexpected projection result: {other:?}"),
            }
            assert!(lock_state(&state).upserts.is_empty());
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_commit_without_write_when_source_is_stale()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let mut source = source()?;
        let command = command(&source);
        source.current_event_id = EventId::new();
        lock_state(&state).source_result = Some(Ok(Some(source)));

        let result = handler(&state).execute(command).await?;

        assert_eq!(ProjectProductListingOutcome::Stale, result.outcome);
        let state = lock_state(&state);
        assert_eq!(1, state.commit_count);
        assert!(state.upserts.is_empty());
        assert!(state.deletes.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_commit_without_write_when_source_is_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let command = ProjectProductListingCommand {
            event_id: EventId::new(),
            product_listing_id: ProductListingId::new(),
        };
        lock_state(&state).source_result = Some(Ok(None));

        let result = handler(&state).execute(command).await?;

        assert_eq!(ProjectProductListingOutcome::MissingSource, result.outcome);
        let state = lock_state(&state);
        assert_eq!(1, state.commit_count);
        assert!(state.upserts.is_empty());
        assert!(state.deletes.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_delete_exact_source_version_when_withdrawn_projection_is_applied()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = state();
        let mut source = source()?;
        source.lifecycle = ListingLifecycle::Withdrawn;
        let command = command(&source);
        lock_state(&state).source_result = Some(Ok(Some(source.clone())));

        let result = handler(&state).execute(command).await?;

        assert_eq!(ProjectProductListingOutcome::Deleted, result.outcome);
        let state = lock_state(&state);
        assert_eq!(
            vec![(source.product_listing_id, source.projection_version)],
            state.deletes
        );
        assert!(state.upserts.is_empty());
        assert_eq!(vec![1], state.write_commit_counts);
        assert_eq!(1, state.commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_complete_stale_target_writes_with_only_the_source_transaction()
    -> Result<(), Box<dyn std::error::Error>> {
        for lifecycle in [ListingLifecycle::Active, ListingLifecycle::Withdrawn] {
            let state = state();
            let mut source = source()?;
            source.lifecycle = lifecycle;
            let command = command(&source);
            {
                let mut state = lock_state(&state);
                state.source_result = Some(Ok(Some(source)));
                state.projection_result =
                    Some(Ok(ProductListingSearchProjectionWriteOutcome::Stale));
            }

            let result = handler(&state).execute(command).await?;

            assert_eq!(ProjectProductListingOutcome::Stale, result.outcome);
            let state = lock_state(&state);
            assert_eq!(1, state.commit_count);
            assert_eq!(vec![1], state.write_commit_counts);
            assert_eq!(1, state.upserts.len() + state.deletes.len());
        }
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
            state.projection_result =
                Some(Err(ProductListingSearchProjectionWriteError::WriteFailed {
                    source: box_error(std::io::Error::other("target unavailable")),
                }));
        }

        let result = handler(&state).execute(command).await;

        assert!(matches!(
            result,
            Err(ProjectProductListingError::WriteFailed { .. })
        ));
        let state = lock_state(&state);
        assert_eq!(1, state.commit_count);
        assert_eq!(vec![1], state.write_commit_counts);
        Ok(())
    }
}
