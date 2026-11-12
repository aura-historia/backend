use application::{
    error::BoxError,
    operation_context::OperationContext,
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use listing_source_core::ListingSourceId;
use std::collections::BTreeSet;

use crate::{
    ports::{
        PublicListingSourceDetailsReadError, PublicListingSourceDetailsReader,
        PublicListingSourceDetailsReaderFactory,
    },
    use_cases::queries::public_listing_source::PublicListingSourceSummary,
};

/// Public cards for ListingSources another context references by ID.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GetPublicListingSourcesByIdsRequest {
    pub ids: BTreeSet<ListingSourceId>,
}

/// The requested sources that exist, ordered by ID. Unknown IDs are left out, not reported.
pub type GetPublicListingSourcesByIdsResult = Vec<PublicListingSourceSummary>;

#[derive(Debug, thiserror::Error)]
pub enum GetPublicListingSourcesByIdsError {
    #[error("temporary public listing source detail read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid public listing source detail read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal public listing source detail read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin public listing source detail transaction")]
    BeginTransaction {
        #[source]
        source: TransactionError,
    },
    #[error("failed to commit public listing source detail transaction")]
    CommitTransaction {
        #[source]
        source: TransactionError,
    },
}

#[async_trait::async_trait]
pub trait GetPublicListingSourcesByIdsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetPublicListingSourcesByIdsRequest,
    ) -> Result<GetPublicListingSourcesByIdsResult, GetPublicListingSourcesByIdsError>;
}

pub struct GetPublicListingSourcesByIdsHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> GetPublicListingSourcesByIdsHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> GetPublicListingSourcesByIdsUseCase for GetPublicListingSourcesByIdsHandler<U, R>
where
    U: UnitOfWork,
    R: PublicListingSourceDetailsReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "get_public_listing_sources_by_ids",
        skip_all,
        fields(
            requested = request.ids.len(),
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetPublicListingSourcesByIdsRequest,
    ) -> Result<GetPublicListingSourcesByIdsResult, GetPublicListingSourcesByIdsError> {
        if request.ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids = request.ids.into_iter().collect::<Vec<_>>();
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|source| GetPublicListingSourcesByIdsError::BeginTransaction { source })?;
        let summaries = self
            .reader
            .in_transaction(&mut tx)
            .find_by_ids(&ids)
            .await?;
        tx.commit()
            .await
            .map_err(|source| GetPublicListingSourcesByIdsError::CommitTransaction { source })?;
        Ok(summaries)
    }
}

impl From<PublicListingSourceDetailsReadError> for GetPublicListingSourcesByIdsError {
    fn from(value: PublicListingSourceDetailsReadError) -> Self {
        match value {
            PublicListingSourceDetailsReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PublicListingSourceDetailsReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            PublicListingSourceDetailsReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::use_cases::queries::public_listing_source::PublicListingSourceOperatorSummary;
    use application::{
        error::static_error,
        operation_context::{CorrelationId, Principal, RequestId},
    };
    use listing_source_core::{ListingSourceName, ListingSourceSlugId};
    use party_core::party_name::PartyName;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Default)]
    struct State {
        begins: usize,
        requested: Vec<Vec<ListingSourceId>>,
        commits: usize,
    }

    #[derive(Clone)]
    struct FakeUnitOfWork(Arc<Mutex<State>>);

    struct FakeTransaction(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            lock(&self.0).commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            lock(&self.0).begins += 1;
            Ok(FakeTransaction(Arc::clone(&self.0)))
        }
    }

    /// Knows `stored` sources and fails every read when `fails` is set.
    struct FakeReaderFactory {
        state: Arc<Mutex<State>>,
        stored: Vec<PublicListingSourceSummary>,
        fails: bool,
    }

    struct FakeReader<'a>(&'a FakeReaderFactory);

    impl PublicListingSourceDetailsReaderFactory<FakeTransaction> for FakeReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl PublicListingSourceDetailsReader + 'tx {
            FakeReader(self)
        }
    }

    #[async_trait::async_trait]
    impl PublicListingSourceDetailsReader for FakeReader<'_> {
        async fn find_by_slug(
            &mut self,
            _slug_id: &ListingSourceSlugId,
        ) -> Result<Option<PublicListingSourceSummary>, PublicListingSourceDetailsReadError>
        {
            unreachable!("ID lookups never read by slug")
        }

        async fn find_by_ids(
            &mut self,
            ids: &[ListingSourceId],
        ) -> Result<Vec<PublicListingSourceSummary>, PublicListingSourceDetailsReadError> {
            lock(&self.0.state).requested.push(ids.to_vec());
            if self.0.fails {
                return Err(
                    PublicListingSourceDetailsReadError::TemporarilyUnavailable {
                        source: static_error("reader unavailable"),
                    },
                );
            }
            Ok(self
                .0
                .stored
                .iter()
                .filter(|summary| ids.contains(&summary.listing_source_id))
                .cloned()
                .collect())
        }
    }

    fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
        match value.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::Anonymous,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn summary(slug: &str) -> Result<PublicListingSourceSummary, Box<dyn std::error::Error>> {
        Ok(PublicListingSourceSummary {
            listing_source_id: ListingSourceId::new(),
            listing_source_slug_id: ListingSourceSlugId::raw(slug)?,
            name: ListingSourceName::try_from("Source")?,
            operator: PublicListingSourceOperatorSummary {
                name: PartyName::try_from("Operator")?,
            },
            website: None,
            image: None,
        })
    }

    fn handler(
        stored: Vec<PublicListingSourceSummary>,
        fails: bool,
    ) -> (
        Arc<Mutex<State>>,
        GetPublicListingSourcesByIdsHandler<FakeUnitOfWork, FakeReaderFactory>,
    ) {
        let state = Arc::new(Mutex::new(State::default()));
        let handler = GetPublicListingSourcesByIdsHandler::new(
            FakeUnitOfWork(Arc::clone(&state)),
            FakeReaderFactory {
                state: Arc::clone(&state),
                stored,
                fails,
            },
        );
        (state, handler)
    }

    #[tokio::test]
    async fn should_return_known_sources_in_one_read_and_omit_unknown_ids()
    -> Result<(), Box<dyn std::error::Error>> {
        let known = summary("known")?;
        let unknown = ListingSourceId::new();
        let (state, handler) = handler(vec![known.clone(), summary("other")?], false);

        let result = handler
            .execute(
                &context(),
                GetPublicListingSourcesByIdsRequest {
                    ids: BTreeSet::from([known.listing_source_id, unknown]),
                },
            )
            .await?;

        assert_eq!(vec![known], result);
        let state = lock(&state);
        assert_eq!(1, state.requested.len());
        assert_eq!(1, state.commits);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_open_a_transaction_when_no_ids_are_requested()
    -> Result<(), Box<dyn std::error::Error>> {
        let (state, handler) = handler(vec![summary("known")?], false);

        let result = handler
            .execute(&context(), GetPublicListingSourcesByIdsRequest::default())
            .await?;

        assert!(result.is_empty());
        assert_eq!(0, lock(&state).begins);
        Ok(())
    }

    #[tokio::test]
    async fn should_report_reader_failure_instead_of_an_empty_result()
    -> Result<(), Box<dyn std::error::Error>> {
        let (state, handler) = handler(vec![], true);

        let result = handler
            .execute(
                &context(),
                GetPublicListingSourcesByIdsRequest {
                    ids: BTreeSet::from([ListingSourceId::new()]),
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(GetPublicListingSourcesByIdsError::TemporarilyUnavailable { .. })
        ));
        assert_eq!(0, lock(&state).commits);
        Ok(())
    }
}
