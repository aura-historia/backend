use crate::ports::{ShopDetailsReadError, ShopDetailsReader, ShopDetailsReaderFactory, StoredShop};
use application::transaction::{Transaction, UnitOfWork};
use common::error::boxed::BoxError;
use common::operation_context::OperationContext;
use localization::Language;
use money::Currency;
use serde_email::Email;
use shop_core::domain::Domain;
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_slug_id::ShopSlugId;
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
    pub woocommerce_currency: Option<Currency>,
    pub woocommerce_language: Option<Language>,
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

impl From<StoredShop> for ShopDetailsView {
    fn from(stored: StoredShop) -> Self {
        let shop = stored.shop;
        Self {
            shop_id: shop.id(),
            shop_slug_id: shop.slug_id().clone(),
            name: shop.name().clone(),
            shop_type: shop.shop_type(),
            domains: shop.domains().clone(),
            shopify_domain: shop.shopify().map(|value| value.domain.clone()),
            shopify_currency: shop.shopify().and_then(|value| value.currency),
            shopify_language: shop.shopify().and_then(|value| value.language),
            woocommerce_currency: shop.woocommerce().and_then(|value| value.currency),
            woocommerce_language: shop.woocommerce().and_then(|value| value.language),
            url: shop.presentation().url.clone(),
            view_url: shop.view_url(),
            image: shop.presentation().image.clone(),
            structured_address: shop.address().map(|value| value.structured.clone()),
            geo_address: shop.address().and_then(|value| value.geo),
            phone: shop.contact().phone.clone(),
            email: shop.contact().email.clone(),
            partner_status: shop.partner_status(),
            affiliate_configuration: shop.affiliate_configuration().cloned(),
            created: stored.created,
            updated: stored.updated,
        }
    }
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
    use application::transaction::{TransactionError, UnitOfWork};
    use common::error::boxed::static_error;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Copy)]
    enum ReadErrorKind {
        InvalidReadModel,
    }

    #[derive(Default)]
    struct Counts {
        begin: usize,
        commit: usize,
        details: usize,
    }

    #[derive(Default)]
    struct State {
        begin_error: bool,
        commit_error: bool,
        details: Option<ShopDetailsView>,
        details_error: Option<ReadErrorKind>,
        last_details_request: Option<GetShopRequest>,
        counts: Counts,
    }

    #[derive(Clone, Default)]
    struct FakeUnitOfWork {
        state: Arc<Mutex<State>>,
    }

    #[derive(Clone, Default)]
    struct FakeDetailsReaderFactory {
        state: Arc<Mutex<State>>,
    }

    struct FakeTx {
        state: Arc<Mutex<State>>,
    }

    struct FakeDetailsReader {
        state: Arc<Mutex<State>>,
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let fail = with_state(&self.state, |state| {
                state.counts.begin += 1;
                state.begin_error
            });
            if fail {
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
            let fail = with_state(&self.state, |state| {
                state.counts.commit += 1;
                state.commit_error
            });
            if fail {
                Err(TransactionError::CommitFailed)
            } else {
                Ok(())
            }
        }
    }

    impl ShopDetailsReaderFactory<FakeTx> for FakeDetailsReaderFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl ShopDetailsReader + 'tx {
            FakeDetailsReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ShopDetailsReader for FakeDetailsReader {
        async fn find_details(
            &mut self,
            request: &GetShopRequest,
        ) -> Result<Option<ShopDetailsView>, ShopDetailsReadError> {
            with_state(&self.state, |state| {
                state.counts.details += 1;
                state.last_details_request = Some(request.clone());
                match state.details_error {
                    Some(kind) => Err(details_read_error(kind)),
                    None => Ok(state.details.clone()),
                }
            })
        }
    }

    #[tokio::test]
    async fn should_get_shop_by_all_request_shapes() -> Result<(), Box<dyn std::error::Error>> {
        for request in [
            GetShopRequest::ById(ShopId::new()),
            GetShopRequest::BySlug(ShopSlugId::from("antik-markt")),
            GetShopRequest::ByShopifyDomain(Domain::try_from("shopify.example.org")?),
        ] {
            let state = shared_state();
            let view = shop_details_view();
            with_state(&state, |state| state.details = Some(view.clone()));
            let handler = GetShopHandler::new(uow(&state), details_reader(&state));

            let result = handler.execute(&system_context(), request.clone()).await;

            assert!(matches!(result, Ok(ref value) if value.shop_id == view.shop_id));
            assert_eq!(
                Some(request),
                with_state(&state, |state| state.last_details_request.clone())
            );
            assert_counts(&state, |counts| assert_eq!(1, counts.commit));
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_cover_get_shop_errors() {
        let state = shared_state();
        let handler = GetShopHandler::new(uow(&state), details_reader(&state));
        let not_found = handler
            .execute(&system_context(), GetShopRequest::ById(ShopId::new()))
            .await;
        assert!(matches!(not_found, Err(GetShopError::NotFound)));
        assert_counts(&state, |counts| assert_eq!(0, counts.commit));

        let state = shared_state();
        with_state(&state, |state| state.begin_error = true);
        let handler = GetShopHandler::new(uow(&state), details_reader(&state));
        let begin = handler
            .execute(&system_context(), GetShopRequest::ById(ShopId::new()))
            .await;
        assert!(matches!(begin, Err(GetShopError::BeginTransactionFailed)));

        let state = shared_state();
        with_state(&state, |state| {
            state.details_error = Some(ReadErrorKind::InvalidReadModel)
        });
        let handler = GetShopHandler::new(uow(&state), details_reader(&state));
        let read = handler
            .execute(&system_context(), GetShopRequest::ById(ShopId::new()))
            .await;
        assert!(matches!(read, Err(GetShopError::InvalidReadModel { .. })));
        assert_counts(&state, |counts| assert_eq!(0, counts.commit));

        let state = shared_state();
        with_state(&state, |state| {
            state.details = Some(shop_details_view());
            state.commit_error = true;
        });
        let handler = GetShopHandler::new(uow(&state), details_reader(&state));
        let commit = handler
            .execute(&system_context(), GetShopRequest::ById(ShopId::new()))
            .await;
        assert!(matches!(commit, Err(GetShopError::CommitTransactionFailed)));
    }

    fn details_reader(state: &Arc<Mutex<State>>) -> FakeDetailsReaderFactory {
        FakeDetailsReaderFactory {
            state: Arc::clone(state),
        }
    }

    fn uow(state: &Arc<Mutex<State>>) -> FakeUnitOfWork {
        FakeUnitOfWork {
            state: Arc::clone(state),
        }
    }

    fn shared_state() -> Arc<Mutex<State>> {
        Arc::new(Mutex::new(State::default()))
    }

    fn details_read_error(kind: ReadErrorKind) -> ShopDetailsReadError {
        match kind {
            ReadErrorKind::InvalidReadModel => ShopDetailsReadError::InvalidReadModel {
                source: static_error("invalid"),
            },
        }
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
            woocommerce_currency: None,
            woocommerce_language: None,
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

    fn system_context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::from("request"),
            correlation_id: CorrelationId::from("correlation"),
        }
    }

    fn assert_counts(state: &Arc<Mutex<State>>, assert: impl FnOnce(&Counts)) {
        with_state(state, |state| assert(&state.counts));
    }

    fn with_state<R>(state: &Arc<Mutex<State>>, f: impl FnOnce(&mut State) -> R) -> R {
        match state.lock() {
            Ok(mut guard) => f(&mut guard),
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                f(&mut guard)
            }
        }
    }
}
