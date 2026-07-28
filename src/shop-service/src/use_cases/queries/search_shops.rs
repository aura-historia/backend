use crate::ports::{ShopSearchReadError, ShopSearchReader, ShopSearchReaderFactory};
use common::domain::Domain;
use common::operation_context::OperationContext;
use common::pagination::cursor::Cursor;
use common::sort::Sort;
use common::transaction::{Transaction, UnitOfWork};
use common::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
use serde_json::Value;
use shop_core::{partner_status::ShopPartnerStatus, shop_search::ShopSearch, shop_type::ShopType};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchShopsRequest {
    pub search: ShopSearch,
    pub sort: Option<Sort<shop_core::sort_shop_field::SortShopField>>,
    pub cursor: Option<Cursor<Value>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShopSummary {
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,
    pub shop_type: ShopType,
    pub partner_status: ShopPartnerStatus,
    pub domains: Vec<Domain>,
    pub image: Option<Url>,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchShopsResult {
    pub items: Vec<ShopSummary>,
    pub cursor: Cursor<Value>,
    pub total: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchShopsError {
    #[error("temporary shop search failure")]
    TemporarilyUnavailable,
    #[error("invalid shop search read model")]
    InvalidReadModel,
    #[error("internal shop search failure")]
    Internal,
    #[error("failed to begin search shops transaction")]
    BeginTransactionFailed,
    #[error("failed to commit search shops transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait SearchShopsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchShopsRequest,
    ) -> Result<SearchShopsResult, SearchShopsError>;
}

pub struct SearchShopsHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> SearchShopsHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> SearchShopsUseCase for SearchShopsHandler<U, R>
where
    U: UnitOfWork,
    R: ShopSearchReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "search_shops",
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
        request: SearchShopsRequest,
    ) -> Result<SearchShopsResult, SearchShopsError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| SearchShopsError::BeginTransactionFailed)?;

        let result = self.reader.in_transaction(&mut tx).search(&request).await?;

        tx.commit()
            .await
            .map_err(|_| SearchShopsError::CommitTransactionFailed)?;

        Ok(result)
    }
}

impl From<ShopSearchReadError> for SearchShopsError {
    fn from(error: ShopSearchReadError) -> Self {
        match error {
            ShopSearchReadError::TemporarilyUnavailable => Self::TemporarilyUnavailable,
            ShopSearchReadError::InvalidReadModel => Self::InvalidReadModel,
            ShopSearchReadError::Internal => Self::Internal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct TestShopSearchReaderFactory {
        called: Arc<Mutex<bool>>,
        result: SearchShopsResult,
    }

    struct TestShopSearchReader {
        called: Arc<Mutex<bool>>,
        result: SearchShopsResult,
    }

    struct TestUnitOfWork {
        committed: Arc<Mutex<bool>>,
    }

    struct TestTransaction {
        committed: Arc<Mutex<bool>>,
    }

    #[async_trait::async_trait]
    impl UnitOfWork for TestUnitOfWork {
        type Tx = TestTransaction;

        async fn begin(&self) -> Result<Self::Tx, common::transaction::TransactionError> {
            Ok(TestTransaction {
                committed: Arc::clone(&self.committed),
            })
        }
    }

    #[async_trait::async_trait]
    impl Transaction for TestTransaction {
        async fn commit(self) -> Result<(), common::transaction::TransactionError> {
            with_mutex(&self.committed, |committed| *committed = true);
            Ok(())
        }
    }

    impl ShopSearchReaderFactory<TestTransaction> for TestShopSearchReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl ShopSearchReader + 'tx {
            TestShopSearchReader {
                called: Arc::clone(&self.called),
                result: self.result.clone(),
            }
        }
    }

    #[async_trait::async_trait]
    impl ShopSearchReader for TestShopSearchReader {
        async fn search(
            &mut self,
            _request: &SearchShopsRequest,
        ) -> Result<SearchShopsResult, ShopSearchReadError> {
            with_mutex(&self.called, |called| *called = true);
            Ok(self.result.clone())
        }
    }

    #[tokio::test]
    async fn should_search_shops_in_owned_transaction() {
        let committed = Arc::new(Mutex::new(false));
        let called = Arc::new(Mutex::new(false));
        let expected = search_result();
        let handler = SearchShopsHandler::new(
            TestUnitOfWork {
                committed: Arc::clone(&committed),
            },
            TestShopSearchReaderFactory {
                called: Arc::clone(&called),
                result: expected.clone(),
            },
        );

        let result = handler.execute(&context(), search_request()).await;

        assert!(matches!(result, Ok(ref value) if value.items == expected.items));
        assert!(with_mutex(&called, |called| *called));
        assert!(with_mutex(&committed, |committed| *committed));
    }

    fn search_request() -> SearchShopsRequest {
        SearchShopsRequest {
            search: ShopSearch::default(),
            sort: None,
            cursor: None,
        }
    }

    fn search_result() -> SearchShopsResult {
        SearchShopsResult {
            items: vec![ShopSummary {
                shop_id: ShopId::new(),
                shop_slug_id: ShopSlugId::from("antik-markt"),
                name: ShopName::from("Antik Markt"),
                shop_type: ShopType::CommercialDealer,
                partner_status: ShopPartnerStatus::Scraped,
                domains: Vec::new(),
                image: None,
                updated: OffsetDateTime::UNIX_EPOCH,
            }],
            cursor: Cursor {
                size: 10,
                search_after: None,
            },
            total: Some(1),
        }
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        }
    }

    fn with_mutex<T, R>(mutex: &Mutex<T>, f: impl FnOnce(&mut T) -> R) -> R {
        match mutex.lock() {
            Ok(mut guard) => f(&mut guard),
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                f(&mut guard)
            }
        }
    }
}
