use crate::ports::{ShopDetailsReadError, ShopDetailsReader, ShopDetailsReaderFactory};
use common::currency::domain::Currency;
use common::domain::Domain;
use common::error::boxed::BoxError;
use common::language::domain::Language;
use common::operation_context::OperationContext;
use common::transaction::{Transaction, UnitOfWork};
use common::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
use serde_email::Email;
use shop_core::{
    address::{GeoAddress, StructuredAddress},
    affiliate_configuration::AffiliateConfiguration,
    partner_status::ShopPartnerStatus,
    shop_type::ShopType,
};
use std::collections::HashSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub enum GetShopRequest {
    ById(ShopId),
    BySlug(ShopSlugId),
    ByShopifyDomain(Domain),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShopDetailsView {
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,
    pub shop_type: ShopType,
    pub domains: HashSet<Domain>,
    pub shopify_domain: Option<Domain>,
    pub shopify_currency: Option<Currency>,
    pub shopify_language: Option<Language>,
    pub url: Option<Url>,
    pub view_url: Option<Url>,
    pub image: Option<Url>,
    pub structured_address: Option<StructuredAddress>,
    pub geo_address: Option<GeoAddress>,
    pub phone: Option<String>,
    pub email: Option<Email>,
    pub partner_status: ShopPartnerStatus,
    pub affiliate_configuration: Option<AffiliateConfiguration>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum GetShopError {
    #[error("shop not found")]
    NotFound,
    #[error("temporary shop details read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid shop details read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal shop details read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin get shop transaction")]
    BeginTransactionFailed,
    #[error("failed to commit get shop transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait GetShopUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetShopRequest,
    ) -> Result<ShopDetailsView, GetShopError>;
}

pub struct GetShopHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> GetShopHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> GetShopUseCase for GetShopHandler<U, R>
where
    U: UnitOfWork,
    R: ShopDetailsReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "get_shop",
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
        request: GetShopRequest,
    ) -> Result<ShopDetailsView, GetShopError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetShopError::BeginTransactionFailed)?;

        let result = self
            .reader
            .in_transaction(&mut tx)
            .find_details(&request)
            .await?
            .ok_or(GetShopError::NotFound)?;

        tx.commit()
            .await
            .map_err(|_| GetShopError::CommitTransactionFailed)?;

        Ok(result)
    }
}

impl From<ShopDetailsReadError> for GetShopError {
    fn from(error: ShopDetailsReadError) -> Self {
        match error {
            ShopDetailsReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ShopDetailsReadError::InvalidReadModel { source } => Self::InvalidReadModel { source },
            ShopDetailsReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct TestShopDetailsReaderFactory {
        called: Arc<Mutex<bool>>,
        view: Option<ShopDetailsView>,
    }

    struct TestShopDetailsReader {
        called: Arc<Mutex<bool>>,
        view: Option<ShopDetailsView>,
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

    impl ShopDetailsReaderFactory<TestTransaction> for TestShopDetailsReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl ShopDetailsReader + 'tx {
            TestShopDetailsReader {
                called: Arc::clone(&self.called),
                view: self.view.clone(),
            }
        }
    }

    #[async_trait::async_trait]
    impl ShopDetailsReader for TestShopDetailsReader {
        async fn find_details(
            &mut self,
            _request: &GetShopRequest,
        ) -> Result<Option<ShopDetailsView>, ShopDetailsReadError> {
            with_mutex(&self.called, |called| *called = true);
            Ok(self.view.clone())
        }
    }

    #[tokio::test]
    async fn should_read_shop_details_in_owned_transaction() {
        let committed = Arc::new(Mutex::new(false));
        let called = Arc::new(Mutex::new(false));
        let view = shop_details_view();
        let handler = GetShopHandler::new(
            TestUnitOfWork {
                committed: Arc::clone(&committed),
            },
            TestShopDetailsReaderFactory {
                called: Arc::clone(&called),
                view: Some(view.clone()),
            },
        );

        let result = handler
            .execute(&context(), GetShopRequest::ById(view.shop_id))
            .await;

        assert!(matches!(result, Ok(ref value) if value.shop_id == view.shop_id));
        assert!(with_mutex(&called, |called| *called));
        assert!(with_mutex(&committed, |committed| *committed));
    }

    #[tokio::test]
    async fn should_not_commit_when_shop_details_missing() {
        let committed = Arc::new(Mutex::new(false));
        let handler = GetShopHandler::new(
            TestUnitOfWork {
                committed: Arc::clone(&committed),
            },
            TestShopDetailsReaderFactory {
                called: Arc::new(Mutex::new(false)),
                view: None,
            },
        );

        let result = handler
            .execute(&context(), GetShopRequest::ById(ShopId::new()))
            .await;

        assert!(matches!(result, Err(GetShopError::NotFound)));
        assert!(!with_mutex(&committed, |committed| *committed));
    }

    fn shop_details_view() -> ShopDetailsView {
        ShopDetailsView {
            shop_id: ShopId::new(),
            shop_slug_id: ShopSlugId::from("antik-markt"),
            name: ShopName::from("Antik Markt"),
            shop_type: ShopType::CommercialDealer,
            domains: HashSet::new(),
            shopify_domain: None,
            shopify_currency: None,
            shopify_language: None,
            url: None,
            view_url: None,
            image: None,
            structured_address: None,
            geo_address: None,
            phone: None,
            email: None,
            partner_status: ShopPartnerStatus::Scraped,
            affiliate_configuration: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
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
