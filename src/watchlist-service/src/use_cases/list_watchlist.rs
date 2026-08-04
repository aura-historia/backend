use common::error::boxed::{BoxError, box_error};
use common::language::domain::Language;
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::pagination::cursor::{Cursor, CursoredResult};
use common::product_id::ProductId;
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;
use notification_core::notification::NotificationPayload;
use notification_service::ports::all_notifications_reader::{
    AllNotificationsReadError, AllNotificationsReader,
};
use product_core::user_state::NotificationUserState;
use product_service::ports::{
    ProductWatchlistDetailsCursor, ProductWatchlistDetailsReadError, ProductWatchlistDetailsReader,
    ProductWatchlistDetailsReaderFactory, ProductWatchlistDetailsRequest,
};
use product_service::use_cases::{ProductDetailsView, redact_hidden_product};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct ListWatchlistRequest {
    pub user_id: UserId,
    pub language: Language,
    pub cursor: Cursor<ProductWatchlistDetailsCursor>,
}

pub type ListWatchlistResult = CursoredResult<ProductDetailsView, ProductWatchlistDetailsCursor>;

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
    #[error("watchlist notification read failed")]
    NotificationReadFailed {
        #[source]
        source: BoxError,
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

pub struct ListWatchlistHandler<U, D, N> {
    unit_of_work: U,
    details_reader: D,
    notifications_reader: N,
}

impl<U, D, N> ListWatchlistHandler<U, D, N> {
    pub fn new(unit_of_work: U, details_reader: D, notifications_reader: N) -> Self {
        Self {
            unit_of_work,
            details_reader,
            notifications_reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, D, N> ListWatchlistUseCase for ListWatchlistHandler<U, D, N>
where
    U: UnitOfWork,
    D: ProductWatchlistDetailsReaderFactory<U::Tx>,
    N: AllNotificationsReader,
{
    #[tracing::instrument(name = "list_watchlist", skip_all, fields(user_id = %request.user_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListWatchlistRequest,
    ) -> Result<ListWatchlistResult, ListWatchlistError> {
        authorize_read(context, request.user_id)?;

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ListWatchlistError::BeginTransactionFailed)?;
        let cursor = Cursor {
            size: request.cursor.size.clamp(1, 100),
            search_after: request.cursor.search_after,
        };
        let mut page = self
            .details_reader
            .in_transaction(&mut tx)
            .find_for_user(&ProductWatchlistDetailsRequest {
                user_id: request.user_id,
                language: request.language,
                cursor,
            })
            .await?;
        tx.commit()
            .await
            .map_err(|_| ListWatchlistError::CommitTransactionFailed)?;

        if page.items.is_empty() {
            return Ok(page);
        }

        let newest_notifications = newest_notifications_by_product(
            self.notifications_reader
                .list_all_by_user(&request.user_id)
                .await
                .map_err(notification_read_error)?,
        );

        for product in &mut page.items {
            let user_state = product
                .user_state
                .as_mut()
                .ok_or(ListWatchlistError::InvalidPersistedState)?;
            user_state.notification = newest_notifications
                .get(&product.product_id)
                .copied()
                .unwrap_or_default();
            if user_state.search_filter.hidden {
                redact_hidden_product(product)
                    .map_err(|_| ListWatchlistError::InvalidPersistedState)?;
            }
        }

        Ok(page)
    }
}

fn newest_notifications_by_product(
    notifications: Vec<
        notification_service::ports::all_notifications_reader::AllNotificationsReadItem,
    >,
) -> HashMap<ProductId, NotificationUserState> {
    let mut newest = HashMap::new();

    for notification in notifications {
        let product_id = match notification.notification_payload {
            NotificationPayload::Watchlist { product_id, .. }
            | NotificationPayload::SearchFilter { product_id, .. } => product_id,
            NotificationPayload::PartnerApplication { .. } => continue,
        };
        let state = NotificationUserState {
            seen: notification.seen,
            origin_event_id: Some(notification.origin_event_id),
        };
        let replace = newest
            .get(&product_id)
            .and_then(|current: &NotificationUserState| current.origin_event_id)
            .is_none_or(|current_event_id| notification.origin_event_id > current_event_id);
        if replace {
            newest.insert(product_id, state);
        }
    }

    newest
}

fn notification_read_error(error: AllNotificationsReadError) -> ListWatchlistError {
    ListWatchlistError::NotificationReadFailed {
        source: box_error(error),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use common::event_id::EventId;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use common::product_lifecycle::domain::ProductLifecycle;
    use common::product_slug_id::ProductSlugId;
    use common::product_state::domain::ProductState;
    use common::shop_id::ShopId;
    use common::shop_name::ShopName;
    use common::shop_slug_id::ShopSlugId;
    use common::shops_product_id::ShopsProductId;
    use common::transaction::TransactionError;
    use notification_core::notification::{NotificationPayload, NotificationWatchlistPayload};
    use notification_core::notification_id::NotificationId;
    use notification_service::ports::all_notifications_reader::AllNotificationsReadItem;
    use product_core::product::{ProductAddress, ProductAuction, ProductPricing};
    use product_core::user_state::{ProductUserState, SearchFilterUserState};

    use std::sync::{Arc, Mutex, MutexGuard};
    use time::OffsetDateTime;
    use url::Url;

    #[derive(Default)]
    struct FakeState {
        begin_fails: bool,
        commit_fails: bool,
        details_result: Option<Result<Vec<ProductDetailsView>, ProductWatchlistDetailsReadError>>,
        notifications_result:
            Option<Result<Vec<AllNotificationsReadItem>, AllNotificationsReadError>>,
        commit_count: usize,
        details_requests: Vec<ProductWatchlistDetailsRequest>,
        notification_requests: usize,
        notification_after_commit: bool,
    }

    type SharedState = Arc<Mutex<FakeState>>;

    #[derive(Clone)]
    struct FakeUnitOfWork(SharedState);
    #[derive(Clone)]
    struct FakeDetailsReaderFactory(SharedState);
    #[derive(Clone)]
    struct FakeNotificationsReader(SharedState);
    struct FakeTransaction(SharedState);
    struct FakeDetailsReader(SharedState);

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
            if lock(&self.0).begin_fails {
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

    #[async_trait::async_trait]
    impl ProductWatchlistDetailsReader for FakeDetailsReader {
        async fn find_for_user(
            &mut self,
            request: &ProductWatchlistDetailsRequest,
        ) -> Result<
            CursoredResult<ProductDetailsView, ProductWatchlistDetailsCursor>,
            ProductWatchlistDetailsReadError,
        > {
            let mut state = lock(&self.0);
            state.details_requests.push(request.clone());
            match state.details_result.take().unwrap_or(Ok(Vec::new())) {
                Ok(items) => Ok(CursoredResult {
                    items,
                    cursor: request.cursor,
                    total: None,
                }),
                Err(error) => Err(error),
            }
        }
    }

    #[async_trait::async_trait]
    impl AllNotificationsReader for FakeNotificationsReader {
        async fn list_all_by_user(
            &self,
            _user_id: &UserId,
        ) -> Result<Vec<AllNotificationsReadItem>, AllNotificationsReadError> {
            let mut state = lock(&self.0);
            state.notification_requests += 1;
            state.notification_after_commit = state.commit_count == 1;
            state.notifications_result.take().unwrap_or(Ok(Vec::new()))
        }
    }

    fn handler(
        state: &SharedState,
    ) -> ListWatchlistHandler<FakeUnitOfWork, FakeDetailsReaderFactory, FakeNotificationsReader>
    {
        ListWatchlistHandler::new(
            FakeUnitOfWork(Arc::clone(state)),
            FakeDetailsReaderFactory(Arc::clone(state)),
            FakeNotificationsReader(Arc::clone(state)),
        )
    }

    fn context(user_id: UserId) -> OperationContext {
        OperationContext {
            principal: Principal::User(user_id),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn request(user_id: UserId, language: Language) -> ListWatchlistRequest {
        ListWatchlistRequest {
            user_id,
            language,
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

    fn details(product_id: ProductId) -> Result<ProductDetailsView, url::ParseError> {
        let url = Url::parse("https://example.test/product")?;
        Ok(ProductDetailsView {
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
            product_title: None,
            product_description: None,
            title: None,
            description: None,
            pricing: ProductPricing::default(),
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            currency: None,
            state: ProductState::Available,
            lifecycle: ProductLifecycle::Active,
            url: url.clone(),
            view_url: url,
            images: Default::default(),
            auction: ProductAuction::default(),
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
            user_state: Some(ProductUserState::default()),
        })
    }

    fn notification(
        user_id: UserId,
        product_id: ProductId,
        event_id: EventId,
        seen: bool,
    ) -> Result<AllNotificationsReadItem, url::ParseError> {
        let url = Url::parse("https://example.test/product")?;
        Ok(AllNotificationsReadItem {
            user_id,
            origin_event_id: event_id,
            notification_id: NotificationId::new(),
            notification_type: None,
            notification_payload: NotificationPayload::Watchlist {
                product_id,
                shop_id: ShopId::new(),
                shops_product_id: ShopsProductId::from("product"),
                shop_slug_id: ShopSlugId::from("shop"),
                product_slug_id: ProductSlugId::from("product"),
                shop_name: ShopName::from("Shop"),
                title: None,
                image: None,
                url: url.clone(),
                view_url: url,
                watchlist_payload: NotificationWatchlistPayload::StateChange {
                    old_state: ProductState::Listed,
                    new_state: ProductState::Available,
                },
            },
            seen,
            external: false,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        })
    }

    #[tokio::test]
    async fn should_list_personalized_products_and_batch_notifications()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let first_product_id = ProductId::new();
        let second_product_id = ProductId::new();
        let old_event_id = EventId::new();
        let new_event_id = EventId::new();
        let state = state();
        lock(&state).details_result = Some(Ok(vec![
            details(first_product_id)?,
            details(second_product_id)?,
        ]));
        lock(&state).notifications_result = Some(Ok(vec![
            notification(user_id, first_product_id, old_event_id, true)?,
            notification(user_id, first_product_id, new_event_id, false)?,
        ]));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id, Language::De))
            .await?;

        assert_eq!(
            vec![first_product_id, second_product_id],
            result
                .items
                .iter()
                .map(|product| product.product_id)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            Some(new_event_id),
            result.items[0]
                .user_state
                .as_ref()
                .and_then(|state| state.notification.origin_event_id)
        );
        assert!(
            !result.items[0]
                .user_state
                .as_ref()
                .map(|state| state.notification.seen)
                .unwrap_or(true)
        );
        assert_eq!(
            Some(true),
            result.items[1]
                .user_state
                .as_ref()
                .map(|state| state.notification.seen)
        );
        let state = lock(&state);
        assert_eq!(1, state.details_requests.len());
        assert_eq!(user_id, state.details_requests[0].user_id);
        assert_eq!(Language::De, state.details_requests[0].language);
        assert_eq!(21, state.details_requests[0].cursor.size);
        assert!(state.details_requests[0].cursor.search_after.is_none());
        assert_eq!(1, state.notification_requests);
        assert!(state.notification_after_commit);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_read_notifications_when_watchlist_is_empty()
    -> Result<(), ListWatchlistError> {
        let user_id = UserId::new();
        let state = state();

        let result = handler(&state)
            .execute(&context(user_id), request(user_id, Language::En))
            .await?;

        assert!(result.items.is_empty());
        assert_eq!(0, lock(&state).notification_requests);
        Ok(())
    }

    #[tokio::test]
    async fn should_redact_hidden_product_after_notification_hydration()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let product_id = ProductId::new();
        let mut product = details(product_id)?;
        product.user_state = Some(ProductUserState {
            search_filter: SearchFilterUserState {
                hidden: true,
                ..Default::default()
            },
            ..Default::default()
        });
        let state = state();
        lock(&state).details_result = Some(Ok(vec![product]));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id, Language::En))
            .await?;

        assert_ne!(product_id, result.items[0].product_id);
        Ok(())
    }

    #[tokio::test]
    async fn should_fail_when_notification_read_fails() -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let state = state();
        lock(&state).details_result = Some(Ok(vec![details(ProductId::new())?]));
        lock(&state).notifications_result = Some(Err(AllNotificationsReadError::OperationFailed {
            source: box_error(std::io::Error::other("unavailable")),
        }));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id, Language::En))
            .await;

        assert!(matches!(
            result,
            Err(ListWatchlistError::NotificationReadFailed { .. })
        ));
        assert_eq!(1, lock(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_commit_or_read_notifications_when_details_read_fails() {
        let user_id = UserId::new();
        let state = state();
        lock(&state).details_result = Some(Err(ProductWatchlistDetailsReadError::QueryFailed));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id, Language::En))
            .await;

        assert!(matches!(
            result,
            Err(ListWatchlistError::TemporarilyUnavailable)
        ));
        let state = lock(&state);
        assert_eq!(0, state.commit_count);
        assert_eq!(0, state.notification_requests);
    }

    #[tokio::test]
    async fn should_not_read_notifications_when_commit_fails()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let state = state();
        lock(&state).commit_fails = true;
        lock(&state).details_result = Some(Ok(vec![details(ProductId::new())?]));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id, Language::En))
            .await;

        assert!(matches!(
            result,
            Err(ListWatchlistError::CommitTransactionFailed)
        ));
        assert_eq!(0, lock(&state).notification_requests);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_missing_user_state_from_details_reader()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::new();
        let mut product = details(ProductId::new())?;
        product.user_state = None;
        let state = state();
        lock(&state).details_result = Some(Ok(vec![product]));

        let result = handler(&state)
            .execute(&context(user_id), request(user_id, Language::En))
            .await;

        assert!(matches!(
            result,
            Err(ListWatchlistError::InvalidPersistedState)
        ));
        assert_eq!(1, lock(&state).notification_requests);
        Ok(())
    }

    #[tokio::test]
    async fn should_not_begin_transaction_when_delegated_user_lacks_capability() {
        let user_id = UserId::new();
        let state = state();

        let result = handler(&state)
            .execute(
                &delegated_context(user_id, false),
                request(user_id, Language::En),
            )
            .await;

        assert!(matches!(result, Err(ListWatchlistError::Forbidden)));
        assert_eq!(0, lock(&state).commit_count);
    }

    #[tokio::test]
    async fn should_allow_delegated_user_with_watchlist_read_capability()
    -> Result<(), ListWatchlistError> {
        let user_id = UserId::new();
        let state = state();

        handler(&state)
            .execute(
                &delegated_context(user_id, true),
                request(user_id, Language::En),
            )
            .await?;

        assert_eq!(1, lock(&state).commit_count);
        Ok(())
    }
}
