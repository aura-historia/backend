use crate::ports::{
    ActiveSearchFilterMatchCandidate, ActiveSearchFilterMatchCandidateReadError,
    ActiveSearchFilterMatchCandidateReader, ActiveSearchFilterMatchCandidateReaderFactory,
    ProductMatchEvaluation, ProductMatchEvaluator, ProductMatchEvaluatorError, SearchFilterIndex,
    SearchFilterIndexError, SearchFilterMatchCandidate, SearchFilterMatchPersistOutcome,
    SearchFilterMatchWriteError, SearchFilterMatchWriter, SearchFilterMatchWriterFactory,
};
use common::error::boxed::{BoxError, box_error};
use common::event_id::EventId;
use common::product_id::ProductId;
use common::resource_state::domain::ResourceState;
use common::transaction::{Transaction, UnitOfWork};
use product_service::ports::{
    ProductSearchFilterMatchSource, ProductSearchFilterMatchSourceReadError,
    ProductSearchFilterMatchSourceReader, ProductSearchFilterMatchSourceReaderFactory,
};
use search_filter_core::SearchFilterProductMatch;

#[derive(Debug, Clone, PartialEq)]
pub struct MatchProductEventCommand {
    pub origin_event_id: EventId,
    pub product_id: ProductId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchProductEventOutcome {
    Matched,
    IgnoredMissingSource,
    IgnoredStaleEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchProductEventResult {
    pub outcome: MatchProductEventOutcome,
    pub percolated_count: usize,
    pub persisted_match_count: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum MatchProductEventError {
    #[error("failed to begin product source read transaction")]
    BeginSourceReadTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("product source read failed")]
    ProductSourceReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("product source persisted state is invalid")]
    ProductSourceStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("product source does not match requested event or product")]
    ProductSourceMismatch,
    #[error("failed to commit product source read transaction")]
    CommitSourceReadTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter percolation failed")]
    PercolationFailed {
        #[source]
        source: BoxError,
    },
    #[error("product match evaluation failed")]
    ProductMatchEvaluationFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin search filter match transaction")]
    BeginTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("active search filter match candidate read failed")]
    CandidateReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("active search filter match candidate state is invalid")]
    CandidateStateInvalid {
        #[source]
        source: BoxError,
    },

    #[error("search filter match persistence failed")]
    MatchPersistenceFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted search filter match state is invalid")]
    PersistedMatchStateInvalid {
        #[source]
        source: BoxError,
    },

    #[error("failed to commit search filter match transaction")]
    CommitTransactionFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait MatchProductEventUseCase: Send + Sync {
    async fn execute(
        &self,
        command: MatchProductEventCommand,
    ) -> Result<MatchProductEventResult, MatchProductEventError>;
}

pub struct MatchProductEventHandler<U, S, I, E, R, W> {
    unit_of_work: U,
    sources: S,
    index: I,
    evaluator: E,
    candidates: R,
    matches: W,
}

impl<U, S, I, E, R, W> MatchProductEventHandler<U, S, I, E, R, W> {
    pub fn new(
        unit_of_work: U,
        sources: S,
        index: I,
        evaluator: E,
        candidates: R,
        matches: W,
    ) -> Self {
        Self {
            unit_of_work,
            sources,
            index,
            evaluator,
            candidates,
            matches,
        }
    }
}

#[async_trait::async_trait]
impl<U, S, I, E, R, W> MatchProductEventUseCase for MatchProductEventHandler<U, S, I, E, R, W>
where
    U: UnitOfWork,
    S: ProductSearchFilterMatchSourceReaderFactory<U::Tx>,
    I: SearchFilterIndex,
    E: ProductMatchEvaluator,
    R: ActiveSearchFilterMatchCandidateReaderFactory<U::Tx>,
    W: SearchFilterMatchWriterFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "match_product_event",
        skip_all,
        fields(
            origin_event_id = %command.origin_event_id,
            product_id = %command.product_id,
        )
    )]
    async fn execute(
        &self,
        command: MatchProductEventCommand,
    ) -> Result<MatchProductEventResult, MatchProductEventError> {
        let product = load_product_source(&self.unit_of_work, &self.sources, &command).await?;
        let product = match product {
            ProductSourceReadOutcome::Missing => {
                return Ok(MatchProductEventResult {
                    outcome: MatchProductEventOutcome::IgnoredMissingSource,
                    percolated_count: 0,
                    persisted_match_count: 0,
                });
            }
            ProductSourceReadOutcome::Stale => {
                return Ok(MatchProductEventResult {
                    outcome: MatchProductEventOutcome::IgnoredStaleEvent,
                    percolated_count: 0,
                    persisted_match_count: 0,
                });
            }
            ProductSourceReadOutcome::Current(product) => *product,
        };

        let percolated = self
            .index
            .percolate(&product)
            .await
            .map_err(percolation_error)?;
        let percolated_count = percolated.len();
        let candidates = evaluate_candidates(&self.evaluator, &product, percolated).await?;
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            MatchProductEventError::BeginTransactionFailed {
                source: box_error(source),
            }
        })?;
        let mut candidates = if candidates.is_empty() {
            Vec::new()
        } else {
            self.candidates
                .in_transaction(&mut tx)
                .find_active(&candidates)
                .await
                .map_err(candidate_read_error)?
        };
        sort_candidates(&mut candidates);

        let mut persisted_match_count = 0;
        for candidate in candidates {
            let product_match = SearchFilterProductMatch {
                user_id: candidate.user_id,
                user_search_filter_id: candidate.search_filter_id,
                user_search_filter_name: Some(candidate.search_filter_name),
                product_id: command.product_id,
                origin_event_id: command.origin_event_id,
                enhanced_match_reason: candidate.enhanced_match_reason,
                feedback: None,
            };
            let outcome = self
                .matches
                .in_transaction(&mut tx)
                .insert_if_absent(&product_match)
                .await
                .map_err(match_write_error)?;
            if outcome == SearchFilterMatchPersistOutcome::Inserted {
                persisted_match_count += 1;
            }
        }

        tx.commit()
            .await
            .map_err(|source| MatchProductEventError::CommitTransactionFailed {
                source: box_error(source),
            })?;

        Ok(MatchProductEventResult {
            outcome: MatchProductEventOutcome::Matched,
            percolated_count,
            persisted_match_count,
        })
    }
}

enum ProductSourceReadOutcome {
    Missing,
    Stale,
    Current(Box<ProductSearchFilterMatchSource>),
}

async fn load_product_source<U, S>(
    unit_of_work: &U,
    sources: &S,
    command: &MatchProductEventCommand,
) -> Result<ProductSourceReadOutcome, MatchProductEventError>
where
    U: UnitOfWork,
    S: ProductSearchFilterMatchSourceReaderFactory<U::Tx>,
{
    let mut tx = unit_of_work.begin().await.map_err(|source| {
        MatchProductEventError::BeginSourceReadTransactionFailed {
            source: box_error(source),
        }
    })?;
    let source = sources
        .in_transaction(&mut tx)
        .find_source(command.origin_event_id, command.product_id)
        .await
        .map_err(product_source_read_error)?;
    let outcome = match source {
        None => ProductSourceReadOutcome::Missing,
        Some(product)
            if product.event_id != command.origin_event_id
                || product.product_id != command.product_id =>
        {
            return Err(MatchProductEventError::ProductSourceMismatch);
        }
        Some(product) if product.current_event_id != command.origin_event_id => {
            ProductSourceReadOutcome::Stale
        }
        Some(product) => ProductSourceReadOutcome::Current(Box::new(product)),
    };
    tx.commit().await.map_err(|source| {
        MatchProductEventError::CommitSourceReadTransactionFailed {
            source: box_error(source),
        }
    })?;
    Ok(outcome)
}

async fn evaluate_candidates<E>(
    evaluator: &E,
    product: &ProductSearchFilterMatchSource,
    mut filters: Vec<crate::ports::SearchFilterView>,
) -> Result<Vec<SearchFilterMatchCandidate>, MatchProductEventError>
where
    E: ProductMatchEvaluator,
{
    filters.retain(|filter| filter.state == ResourceState::Active);
    filters.sort_by_key(|filter| filter.search_filter_id.to_string());
    filters.dedup_by(|left, right| left.search_filter_id == right.search_filter_id);

    let mut candidates = Vec::with_capacity(filters.len());
    for filter in filters {
        let enhanced_match_reason = match filter.search.enhanced_search_description {
            Some(_) => match evaluator
                .evaluate(product, &filter)
                .await
                .map_err(product_match_evaluation_error)?
            {
                ProductMatchEvaluation::Matched { reason } => reason,
                ProductMatchEvaluation::NotMatched => continue,
            },
            None => None,
        };
        candidates.push(SearchFilterMatchCandidate {
            user_id: filter.user_id,
            search_filter_id: filter.search_filter_id,
            enhanced_match_reason,
        });
    }
    Ok(candidates)
}

fn sort_candidates(candidates: &mut Vec<ActiveSearchFilterMatchCandidate>) {
    candidates.sort_by_key(|candidate| {
        (
            candidate.user_id.to_string(),
            candidate.search_filter_id.to_string(),
        )
    });
    candidates.dedup_by(|left, right| {
        left.user_id == right.user_id && left.search_filter_id == right.search_filter_id
    });
}

fn product_source_read_error(
    error: ProductSearchFilterMatchSourceReadError,
) -> MatchProductEventError {
    match error {
        ProductSearchFilterMatchSourceReadError::InvalidPersistedState { source } => {
            MatchProductEventError::ProductSourceStateInvalid { source }
        }
        error => MatchProductEventError::ProductSourceReadFailed {
            source: box_error(error),
        },
    }
}

fn percolation_error(error: SearchFilterIndexError) -> MatchProductEventError {
    MatchProductEventError::PercolationFailed {
        source: box_error(error),
    }
}

fn product_match_evaluation_error(error: ProductMatchEvaluatorError) -> MatchProductEventError {
    MatchProductEventError::ProductMatchEvaluationFailed {
        source: box_error(error),
    }
}

fn candidate_read_error(
    error: ActiveSearchFilterMatchCandidateReadError,
) -> MatchProductEventError {
    match error {
        ActiveSearchFilterMatchCandidateReadError::InvalidPersistedState { source } => {
            MatchProductEventError::CandidateStateInvalid { source }
        }
        error => MatchProductEventError::CandidateReadFailed {
            source: box_error(error),
        },
    }
}

fn match_write_error(error: SearchFilterMatchWriteError) -> MatchProductEventError {
    match error {
        SearchFilterMatchWriteError::InvalidPersistedState { source } => {
            MatchProductEventError::PersistedMatchStateInvalid { source }
        }
        error => MatchProductEventError::MatchPersistenceFailed {
            source: box_error(error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        SearchFilterIndexQuery, SearchFilterProjection, SearchFilterProjectionWriteOutcome,
        SearchFilterView,
    };
    use common::{
        currency::domain::Currency, language::domain::Language,
        product_lifecycle::domain::ProductLifecycle, product_slug_id::ProductSlugId,
        product_state::domain::ProductState, shop_id::ShopId, shop_name::ShopName,
        shop_slug_id::ShopSlugId, shops_product_id::ShopsProductId, transaction::TransactionError,
        user_id::UserId, user_search_filter_id::UserSearchFilterId,
        user_search_filter_name::UserSearchFilterName,
    };
    use indexmap::IndexSet;
    use product_core::{
        product::{ProductAddress, ProductAuction, ProductPricing},
        product_image::ProductImage,
    };
    use product_service::ports::{
        ProductSearchFilterMatchShopType, ProductSearchFilterMatchSource,
    };
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;
    use url::Url;

    #[derive(Default)]
    struct State {
        committed: usize,
        persisted: Vec<SearchFilterProductMatch>,
        active_reads: usize,
    }

    #[derive(Clone, Default)]
    struct FakeUnitOfWork(Arc<Mutex<State>>);

    struct FakeTransaction(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = self.0.lock().map_err(|_| TransactionError::CommitFailed)?;
            state.committed += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            Ok(FakeTransaction(Arc::clone(&self.0)))
        }
    }

    struct Sources(Option<ProductSearchFilterMatchSource>);

    struct ReadingSource(Option<ProductSearchFilterMatchSource>);

    #[async_trait::async_trait]
    impl ProductSearchFilterMatchSourceReader for ReadingSource {
        async fn find_source(
            &mut self,
            _event_id: EventId,
            _product_id: ProductId,
        ) -> Result<Option<ProductSearchFilterMatchSource>, ProductSearchFilterMatchSourceReadError>
        {
            Ok(self.0.clone())
        }
    }

    impl ProductSearchFilterMatchSourceReaderFactory<FakeTransaction> for Sources {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ProductSearchFilterMatchSourceReader + 'tx {
            ReadingSource(self.0.clone())
        }
    }

    struct Index {
        filters: Vec<SearchFilterView>,
    }

    #[async_trait::async_trait]
    impl SearchFilterIndex for Index {
        async fn upsert(
            &self,
            _projection: &SearchFilterProjection,
        ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
            Ok(SearchFilterProjectionWriteOutcome::Applied)
        }

        async fn delete(
            &self,
            _id: UserSearchFilterId,
            _source_version: i64,
        ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
            Ok(SearchFilterProjectionWriteOutcome::Applied)
        }

        async fn percolate(
            &self,
            _product: &ProductSearchFilterMatchSource,
        ) -> Result<Vec<SearchFilterView>, SearchFilterIndexError> {
            Ok(self.filters.clone())
        }

        async fn query(
            &self,
            _query: &SearchFilterIndexQuery,
        ) -> Result<
            common::pagination::cursor::CursoredResult<SearchFilterView, serde_json::Value>,
            SearchFilterIndexError,
        > {
            Ok(Default::default())
        }
    }

    struct Evaluator;

    #[async_trait::async_trait]
    impl ProductMatchEvaluator for Evaluator {
        async fn evaluate(
            &self,
            _product: &ProductSearchFilterMatchSource,
            _filter: &SearchFilterView,
        ) -> Result<ProductMatchEvaluation, ProductMatchEvaluatorError> {
            Ok(ProductMatchEvaluation::NotMatched)
        }
    }

    #[derive(Clone)]
    struct Candidates(Arc<Mutex<State>>);

    struct ReadingActiveCandidates<'a>(&'a Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl ActiveSearchFilterMatchCandidateReader for ReadingActiveCandidates<'_> {
        async fn find_active(
            &mut self,
            candidates: &[SearchFilterMatchCandidate],
        ) -> Result<Vec<ActiveSearchFilterMatchCandidate>, ActiveSearchFilterMatchCandidateReadError>
        {
            self.0
                .lock()
                .map_err(|_| ActiveSearchFilterMatchCandidateReadError::ReadFailed {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                })?
                .active_reads += 1;
            Ok(candidates
                .iter()
                .map(|candidate| ActiveSearchFilterMatchCandidate {
                    user_id: candidate.user_id,
                    search_filter_id: candidate.search_filter_id,
                    search_filter_name: UserSearchFilterName::from(
                        candidate.search_filter_id.to_string(),
                    ),
                    enhanced_match_reason: candidate.enhanced_match_reason.clone(),
                })
                .collect())
        }
    }

    impl ActiveSearchFilterMatchCandidateReaderFactory<FakeTransaction> for Candidates {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ActiveSearchFilterMatchCandidateReader + 'tx {
            ReadingActiveCandidates(&self.0)
        }
    }

    #[derive(Clone)]
    struct Matches(Arc<Mutex<State>>);

    struct WritingMatches<'a>(&'a Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl SearchFilterMatchWriter for WritingMatches<'_> {
        async fn insert_if_absent(
            &mut self,
            product_match: &SearchFilterProductMatch,
        ) -> Result<SearchFilterMatchPersistOutcome, SearchFilterMatchWriteError> {
            self.0
                .lock()
                .map_err(|_| SearchFilterMatchWriteError::WriteFailed {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                })?
                .persisted
                .push(product_match.clone());
            Ok(SearchFilterMatchPersistOutcome::Inserted)
        }
    }

    impl SearchFilterMatchWriterFactory<FakeTransaction> for Matches {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl SearchFilterMatchWriter + 'tx {
            WritingMatches(&self.0)
        }
    }

    fn product() -> Result<ProductSearchFilterMatchSource, url::ParseError> {
        let url = Url::parse("https://example.test/product")?;
        let event_id = EventId::new();
        Ok(ProductSearchFilterMatchSource {
            event_id,
            current_event_id: event_id,
            product_id: common::product_id::ProductId::new(),
            product_slug_id: ProductSlugId::from("product"),
            shop_id: ShopId::new(),
            shop_slug_id: ShopSlugId::from("shop"),
            shop_name: ShopName::from("Shop"),
            shop_type: ProductSearchFilterMatchShopType::Marketplace,
            seller_id: ShopId::new(),
            seller_slug_id: common::seller_slug_id::SellerSlugId::from("seller"),
            seller_name: ShopName::from("Seller"),
            shops_product_id: ShopsProductId::from("product"),
            address: ProductAddress::default(),
            product_title: None,
            product_description: None,
            titles: std::collections::HashMap::new(),
            descriptions: std::collections::HashMap::new(),
            pricing: ProductPricing::default(),
            state: ProductState::Available,
            lifecycle: ProductLifecycle::Active,
            url: url.clone(),
            view_url: url,
            image: None,
            images: IndexSet::<ProductImage>::new(),
            auction: ProductAuction::default(),
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        })
    }

    fn filter(user_id: UserId, search_filter_id: UserSearchFilterId) -> SearchFilterView {
        SearchFilterView {
            search_filter_id,
            user_id,
            name: UserSearchFilterName::from("daily"),
            notifications: true,
            state: ResourceState::Active,
            search: search_filter_core::ProductSearch::new(Language::En, Currency::Eur),
            embedding: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
            last_hybrid_search_matched: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[tokio::test]
    async fn should_persist_all_active_candidates_without_a_notification_quota()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let user_id = UserId::new();
        let product = product()?;
        let handler = MatchProductEventHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            Sources(Some(product.clone())),
            Index {
                filters: vec![
                    filter(user_id, UserSearchFilterId::new()),
                    filter(user_id, UserSearchFilterId::new()),
                ],
            },
            Evaluator,
            Candidates(Arc::clone(&state)),
            Matches(Arc::clone(&state)),
        );

        let result = handler
            .execute(MatchProductEventCommand {
                origin_event_id: product.event_id,
                product_id: product.product_id,
            })
            .await?;

        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(2, result.percolated_count);
        assert_eq!(2, result.persisted_match_count);
        assert_eq!(2, state.committed);
        assert_eq!(1, state.active_reads);
        assert_eq!(2, state.persisted.len());
        Ok(())
    }

    #[tokio::test]
    async fn should_skip_enhanced_candidate_when_evaluator_does_not_match()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let user_id = UserId::new();
        let mut enhanced = filter(user_id, UserSearchFilterId::new());
        enhanced.search.enhanced_search_description = Some("only paintings".into());
        let product = product()?;
        let handler = MatchProductEventHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            Sources(Some(product.clone())),
            Index {
                filters: vec![enhanced],
            },
            Evaluator,
            Candidates(Arc::clone(&state)),
            Matches(Arc::clone(&state)),
        );

        let result = handler
            .execute(MatchProductEventCommand {
                origin_event_id: product.event_id,
                product_id: product.product_id,
            })
            .await?;

        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(1, result.percolated_count);
        assert_eq!(0, result.persisted_match_count);
        assert_eq!(2, state.committed);
        assert_eq!(0, state.active_reads);
        Ok(())
    }

    #[tokio::test]
    async fn should_ignore_stale_product_events_after_committing_source_read()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let mut product = product()?;
        product.current_event_id = EventId::new();
        let handler = MatchProductEventHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            Sources(Some(product.clone())),
            Index {
                filters: Vec::new(),
            },
            Evaluator,
            Candidates(Arc::clone(&state)),
            Matches(Arc::clone(&state)),
        );

        let result = handler
            .execute(MatchProductEventCommand {
                origin_event_id: product.event_id,
                product_id: product.product_id,
            })
            .await?;

        assert_eq!(MatchProductEventOutcome::IgnoredStaleEvent, result.outcome);
        assert_eq!(
            1,
            state
                .lock()
                .map_err(|_| std::io::Error::other("test mutex poisoned"))?
                .committed
        );
        Ok(())
    }
}
