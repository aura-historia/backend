use crate::ports::{
    EnhancedSearchFilterEvaluation, EnhancedSearchFilterEvaluator,
    EnhancedSearchFilterEvaluatorError, SearchFilterIndex, SearchFilterIndexError,
    SearchFilterMatchCandidate, SearchFilterMatchCandidateValidationError,
    SearchFilterMatchCandidateValidator, SearchFilterMatchCandidateValidatorFactory,
    SearchFilterMatchPersistOutcome, SearchFilterMatchWriteError, SearchFilterMatchWriter,
    SearchFilterMatchWriterFactory, SearchFilterMonthlyMatchQuotaReadError,
    SearchFilterMonthlyMatchQuotaReader, SearchFilterMonthlyMatchQuotaReaderFactory,
    ValidatedSearchFilterMatchCandidate,
};
use crate::tier_policy::monthly_match_quota;
use common::error::boxed::{BoxError, box_error};
use common::event_id::EventId;
use common::resource_state::domain::ResourceState;
use common::transaction::{Transaction, UnitOfWork};
use product_service::ports::ProductSearchFilterMatchSource;
use search_filter_core::SearchFilterProductMatch;
use time::OffsetDateTime;
use user_service::ports::{
    UserTierEntitlements, UserTierEntitlementsError, UserTierEntitlementsFactory,
};

#[derive(Debug, Clone, PartialEq)]
pub struct MatchProductEventCommand {
    pub origin_event_id: EventId,
    pub occurred_at: OffsetDateTime,
    pub product: ProductSearchFilterMatchSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchProductEventResult {
    pub percolated_count: usize,
    pub persisted_match_count: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum MatchProductEventError {
    #[error("search filter percolation failed")]
    PercolationFailed {
        #[source]
        source: BoxError,
    },
    #[error("enhanced search filter evaluation failed")]
    EnhancedEvaluationFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin search filter match transaction")]
    BeginTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("authoritative search filter match candidate validation failed")]
    CandidateValidationFailed {
        #[source]
        source: BoxError,
    },
    #[error("authoritative search filter match candidate state is invalid")]
    CandidateStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("user tier entitlement lock failed")]
    UserTierEntitlementsLockFailed {
        #[source]
        source: BoxError,
    },
    #[error("monthly search filter match quota read failed")]
    MonthlyMatchQuotaReadFailed {
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

pub struct MatchProductEventHandler<U, I, E, V, Q, W, A> {
    unit_of_work: U,
    index: I,
    evaluator: E,
    candidates: V,
    quotas: Q,
    matches: W,
    tier_entitlements: A,
}

impl<U, I, E, V, Q, W, A> MatchProductEventHandler<U, I, E, V, Q, W, A> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        unit_of_work: U,
        index: I,
        evaluator: E,
        candidates: V,
        quotas: Q,
        matches: W,
        tier_entitlements: A,
    ) -> Self {
        Self {
            unit_of_work,
            index,
            evaluator,
            candidates,
            quotas,
            matches,
            tier_entitlements,
        }
    }
}

#[async_trait::async_trait]
impl<U, I, E, V, Q, W, A> MatchProductEventUseCase for MatchProductEventHandler<U, I, E, V, Q, W, A>
where
    U: UnitOfWork,
    I: SearchFilterIndex,
    E: EnhancedSearchFilterEvaluator,
    V: SearchFilterMatchCandidateValidatorFactory<U::Tx>,
    Q: SearchFilterMonthlyMatchQuotaReaderFactory<U::Tx>,
    W: SearchFilterMatchWriterFactory<U::Tx>,
    A: UserTierEntitlementsFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "match_product_event",
        skip_all,
        fields(
            origin_event_id = %command.origin_event_id,
            product_id = %command.product.product_id,
        )
    )]
    async fn execute(
        &self,
        command: MatchProductEventCommand,
    ) -> Result<MatchProductEventResult, MatchProductEventError> {
        let percolated = self
            .index
            .percolate(&command.product)
            .await
            .map_err(percolation_error)?;
        let percolated_count = percolated.len();
        let candidates = evaluate_candidates(&self.evaluator, &command.product, percolated).await?;
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
                .validate_for_product(command.product.product_id, &candidates)
                .await
                .map_err(candidate_validation_error)?
        };
        sort_candidates(&mut candidates);

        let mut persisted_match_count = 0;
        let mut offset = 0;
        while offset < candidates.len() {
            let user_id = candidates[offset].user_id;
            let end = candidates[offset..]
                .iter()
                .position(|candidate| candidate.user_id != user_id)
                .map_or(candidates.len(), |position| offset + position);
            let Some(tier) = self
                .tier_entitlements
                .in_transaction(&mut tx)
                .lock_user_tier(user_id)
                .await
                .map_err(tier_entitlements_error)?
            else {
                offset = end;
                continue;
            };
            let quota = monthly_match_quota(tier);
            let mut used = self
                .quotas
                .in_transaction(&mut tx)
                .count_for_user_in_month(user_id, command.occurred_at)
                .await
                .map_err(monthly_match_quota_error)?;

            for candidate in &candidates[offset..end] {
                if used >= quota {
                    break;
                }
                let product_match = SearchFilterProductMatch {
                    user_id,
                    user_search_filter_id: candidate.search_filter_id,
                    user_search_filter_name: Some(candidate.search_filter_name.clone()),
                    product_id: command.product.product_id,
                    origin_event_id: command.origin_event_id,
                    enhanced_match_reason: candidate.enhanced_match_reason.clone(),
                    feedback: None,
                };
                let outcome = self
                    .matches
                    .in_transaction(&mut tx)
                    .insert_if_absent(&product_match)
                    .await
                    .map_err(match_write_error)?;
                if outcome == SearchFilterMatchPersistOutcome::AlreadyExists {
                    continue;
                }

                persisted_match_count += 1;
                used = used.saturating_add(1);
            }
            offset = end;
        }

        tx.commit()
            .await
            .map_err(|source| MatchProductEventError::CommitTransactionFailed {
                source: box_error(source),
            })?;

        Ok(MatchProductEventResult {
            percolated_count,
            persisted_match_count,
        })
    }
}

async fn evaluate_candidates<E>(
    evaluator: &E,
    product: &ProductSearchFilterMatchSource,
    mut filters: Vec<crate::ports::SearchFilterView>,
) -> Result<Vec<SearchFilterMatchCandidate>, MatchProductEventError>
where
    E: EnhancedSearchFilterEvaluator,
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
                .map_err(enhanced_evaluation_error)?
            {
                EnhancedSearchFilterEvaluation::Matched { reason } => reason,
                EnhancedSearchFilterEvaluation::NotMatched => continue,
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

fn sort_candidates(candidates: &mut Vec<ValidatedSearchFilterMatchCandidate>) {
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

fn percolation_error(error: SearchFilterIndexError) -> MatchProductEventError {
    MatchProductEventError::PercolationFailed {
        source: box_error(error),
    }
}

fn enhanced_evaluation_error(error: EnhancedSearchFilterEvaluatorError) -> MatchProductEventError {
    MatchProductEventError::EnhancedEvaluationFailed {
        source: box_error(error),
    }
}

fn candidate_validation_error(
    error: SearchFilterMatchCandidateValidationError,
) -> MatchProductEventError {
    match error {
        SearchFilterMatchCandidateValidationError::InvalidPersistedState { source } => {
            MatchProductEventError::CandidateStateInvalid { source }
        }
        error => MatchProductEventError::CandidateValidationFailed {
            source: box_error(error),
        },
    }
}

fn tier_entitlements_error(error: UserTierEntitlementsError) -> MatchProductEventError {
    match error {
        UserTierEntitlementsError::LockFailed { source }
        | UserTierEntitlementsError::ReconciliationFailed { source } => {
            MatchProductEventError::UserTierEntitlementsLockFailed { source }
        }
    }
}

fn monthly_match_quota_error(
    error: SearchFilterMonthlyMatchQuotaReadError,
) -> MatchProductEventError {
    MatchProductEventError::MonthlyMatchQuotaReadFailed {
        source: box_error(error),
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
    use user_core::tier::UserTier;

    #[derive(Default)]
    struct State {
        committed: usize,
        persisted: Vec<SearchFilterProductMatch>,
        validated: usize,
        quota_reads: usize,
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
    impl EnhancedSearchFilterEvaluator for Evaluator {
        async fn evaluate(
            &self,
            _product: &ProductSearchFilterMatchSource,
            _filter: &SearchFilterView,
        ) -> Result<EnhancedSearchFilterEvaluation, EnhancedSearchFilterEvaluatorError> {
            Ok(EnhancedSearchFilterEvaluation::NotMatched)
        }
    }

    #[derive(Clone)]
    struct Validator(Arc<Mutex<State>>);

    struct Validating<'a>(&'a Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl SearchFilterMatchCandidateValidator for Validating<'_> {
        async fn validate_for_product(
            &mut self,
            _product_id: common::product_id::ProductId,
            candidates: &[SearchFilterMatchCandidate],
        ) -> Result<
            Vec<ValidatedSearchFilterMatchCandidate>,
            SearchFilterMatchCandidateValidationError,
        > {
            self.0
                .lock()
                .map_err(
                    |_| SearchFilterMatchCandidateValidationError::ValidationFailed {
                        source: box_error(std::io::Error::other("test mutex poisoned")),
                    },
                )?
                .validated += 1;
            Ok(candidates
                .iter()
                .map(|candidate| ValidatedSearchFilterMatchCandidate {
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

    impl SearchFilterMatchCandidateValidatorFactory<FakeTransaction> for Validator {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl SearchFilterMatchCandidateValidator + 'tx {
            Validating(&self.0)
        }
    }

    #[derive(Clone)]
    struct Quotas(Arc<Mutex<State>>);

    struct ReadingQuota<'a>(&'a Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl SearchFilterMonthlyMatchQuotaReader for ReadingQuota<'_> {
        async fn count_for_user_in_month(
            &mut self,
            _user_id: UserId,
            _occurred_at: OffsetDateTime,
        ) -> Result<usize, SearchFilterMonthlyMatchQuotaReadError> {
            self.0
                .lock()
                .map_err(|_| SearchFilterMonthlyMatchQuotaReadError::ReadFailed {
                    source: box_error(std::io::Error::other("test mutex poisoned")),
                })?
                .quota_reads += 1;
            Ok(0)
        }
    }

    impl SearchFilterMonthlyMatchQuotaReaderFactory<FakeTransaction> for Quotas {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl SearchFilterMonthlyMatchQuotaReader + 'tx {
            ReadingQuota(&self.0)
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

    struct Tiers;

    struct LockingTier;

    #[async_trait::async_trait]
    impl UserTierEntitlements for LockingTier {
        async fn lock_user_tier(
            &mut self,
            _user_id: UserId,
        ) -> Result<Option<UserTier>, UserTierEntitlementsError> {
            Ok(Some(UserTier::Free))
        }

        async fn reconcile_for_tier(
            &mut self,
            _user_id: UserId,
            _tier: UserTier,
        ) -> Result<(), UserTierEntitlementsError> {
            Ok(())
        }
    }

    impl UserTierEntitlementsFactory<FakeTransaction> for Tiers {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl UserTierEntitlements + 'tx {
            LockingTier
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
    async fn should_validate_and_persist_all_candidates() -> Result<(), Box<dyn std::error::Error>>
    {
        let state = Arc::new(Mutex::new(State::default()));
        let user_id = UserId::new();
        let handler = MatchProductEventHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            Index {
                filters: vec![
                    filter(user_id, UserSearchFilterId::new()),
                    filter(user_id, UserSearchFilterId::new()),
                ],
            },
            Evaluator,
            Validator(Arc::clone(&state)),
            Quotas(Arc::clone(&state)),
            Matches(Arc::clone(&state)),
            Tiers,
        );
        let product = product()?;

        let result = handler
            .execute(MatchProductEventCommand {
                origin_event_id: product.event_id,
                occurred_at: OffsetDateTime::UNIX_EPOCH,
                product,
            })
            .await?;

        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(2, result.percolated_count);
        assert_eq!(2, result.persisted_match_count);
        assert_eq!(1, state.committed);
        assert_eq!(1, state.validated);
        assert_eq!(1, state.quota_reads);
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
        let handler = MatchProductEventHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            Index {
                filters: vec![enhanced],
            },
            Evaluator,
            Validator(Arc::clone(&state)),
            Quotas(Arc::clone(&state)),
            Matches(Arc::clone(&state)),
            Tiers,
        );
        let product = product()?;

        let result = handler
            .execute(MatchProductEventCommand {
                origin_event_id: product.event_id,
                occurred_at: OffsetDateTime::UNIX_EPOCH,
                product,
            })
            .await?;

        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(1, result.percolated_count);
        assert_eq!(0, result.persisted_match_count);
        assert_eq!(1, state.committed);
        assert_eq!(0, state.validated);
        Ok(())
    }
}
