use crate::ports::{
    PartnerShopRepository, PartnerShopRepositoryError, PartnerShopRepositoryFactory,
    ShopRepository, ShopRepositoryError, ShopRepositoryFactory,
};
use application::transaction::{Transaction, UnitOfWork};
use common::error::boxed::{BoxError, static_error};
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use common::user_id::UserId;
use shop_core::shop_id::ShopId;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, PartialEq)]
pub struct GrantPartnerShopCommand {
    pub user_id: UserId,
    pub shop_id: ShopId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GrantPartnerShopResult {
    pub user_id: UserId,
    pub shop_id: ShopId,
}

#[derive(Debug, thiserror::Error)]
pub enum GrantPartnerShopError {
    #[error("authenticated actor required to grant partner shop")]
    AuthenticatedActorRequired,
    #[error("user not found")]
    UserNotFound {
        #[source]
        source: Option<BoxError>,
    },
    #[error("shop not found")]
    ShopNotFound {
        #[source]
        source: Option<BoxError>,
    },
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary partner shop persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted shop state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal partner shop persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin grant partner shop transaction")]
    BeginTransactionFailed,
    #[error("failed to commit grant partner shop transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait GrantPartnerShopUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: GrantPartnerShopCommand,
    ) -> Result<GrantPartnerShopResult, GrantPartnerShopError>;
}

pub struct GrantPartnerShopHandler<U, S, P, A> {
    unit_of_work: U,
    shops: S,
    partner_shops: P,
    check_user_admin: A,
}

impl<U, S, P, A> GrantPartnerShopHandler<U, S, P, A> {
    pub fn new(unit_of_work: U, shops: S, partner_shops: P, check_user_admin: A) -> Self {
        Self {
            unit_of_work,
            shops,
            partner_shops,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, S, P, A> GrantPartnerShopUseCase for GrantPartnerShopHandler<U, S, P, A>
where
    U: UnitOfWork,
    S: ShopRepositoryFactory<U::Tx>,
    P: PartnerShopRepositoryFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "grant_partner_shop",
        skip_all,
        fields(
            user_id = %command.user_id,
            shop_id = %command.shop_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: GrantPartnerShopCommand,
    ) -> Result<GrantPartnerShopResult, GrantPartnerShopError> {
        context
            .require()
            .credential_capability(CredentialCapability::PartnerShopsWrite)
            .authorize::<GrantPartnerShopError>()?;
        ensure_admin_or_internal(context, &self.check_user_admin).await?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GrantPartnerShopError::BeginTransactionFailed)?;

        self.shops
            .in_transaction(&mut tx)
            .find_by_id(command.shop_id)
            .await?
            .ok_or(GrantPartnerShopError::ShopNotFound { source: None })?;

        self.partner_shops
            .in_transaction(&mut tx)
            .grant(command.user_id, command.shop_id)
            .await?;

        tx.commit()
            .await
            .map_err(|_| GrantPartnerShopError::CommitTransactionFailed)?;

        tracing::info!(
            event = "shop.partner_granted",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            user_id = %command.user_id,
            shop_id = %command.shop_id,
            outcome = "success",
        );

        Ok(GrantPartnerShopResult {
            user_id: command.user_id,
            shop_id: command.shop_id,
        })
    }
}

async fn ensure_admin_or_internal<A>(
    context: &OperationContext,
    check_user_admin: &A,
) -> Result<(), GrantPartnerShopError>
where
    A: CheckUserAdminUseCase,
{
    match context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::User(_) | Principal::DelegatedUser { .. } => check_user_admin
            .execute(context, CheckUserAdminRequest)
            .await
            .map(|_| ())
            .map_err(map_admin_error),
        Principal::Anonymous => Err(GrantPartnerShopError::AuthenticatedActorRequired),
    }
}

fn map_admin_error(error: CheckUserAdminError) -> GrantPartnerShopError {
    match error {
        CheckUserAdminError::AuthenticatedActorRequired => {
            GrantPartnerShopError::AuthenticatedActorRequired
        }
        CheckUserAdminError::Forbidden => GrantPartnerShopError::Forbidden,
        CheckUserAdminError::TemporarilyUnavailable { source } => {
            GrantPartnerShopError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source }
        | CheckUserAdminError::Internal { source } => GrantPartnerShopError::Internal { source },
        CheckUserAdminError::BeginTransactionFailed
        | CheckUserAdminError::CommitTransactionFailed => {
            GrantPartnerShopError::TemporarilyUnavailable {
                source: static_error("check user admin transaction failed"),
            }
        }
    }
}

impl From<OperationAuthorizationError> for GrantPartnerShopError {
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

impl From<ShopRepositoryError> for GrantPartnerShopError {
    fn from(error: ShopRepositoryError) -> Self {
        match error {
            ShopRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ShopRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            ShopRepositoryError::ConcurrencyConflict => Self::Internal {
                source: static_error("unexpected grant partner shop concurrency conflict"),
            },
            ShopRepositoryError::SlugConflict { source }
            | ShopRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<PartnerShopRepositoryError> for GrantPartnerShopError {
    fn from(error: PartnerShopRepositoryError) -> Self {
        match error {
            PartnerShopRepositoryError::UserNotFound { source } => Self::UserNotFound {
                source: Some(source),
            },
            PartnerShopRepositoryError::ShopNotFound { source } => Self::ShopNotFound {
                source: Some(source),
            },
            PartnerShopRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnerShopRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{ShopStorageVersion, StoredShop};
    use application::transaction::{TransactionError, UnitOfWork};
    use common::error::boxed::static_error;
    use common::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use shop_core::partner_status::ShopPartnerStatus;
    use shop_core::shop::{NewShop, Shop, ShopContact, ShopPresentation};
    use shop_core::shop_name::ShopName;
    use shop_core::shop_slug_id::ShopSlugId;
    use shop_core::shop_type::ShopType;
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Copy)]
    struct AllowAdmin;

    #[derive(Clone, Copy)]
    enum PartnerRepoErrorKind {
        UserNotFound,
        ShopNotFound,
        TemporarilyUnavailable,
        Internal,
    }

    #[derive(Default)]
    struct Counts {
        begin: usize,
        commit: usize,
        find_by_id: usize,
        grant: usize,
    }

    #[derive(Default)]
    struct State {
        begin_error: bool,
        commit_error: bool,
        shop_by_id: Option<StoredShop>,
        grant_error: Option<PartnerRepoErrorKind>,
        counts: Counts,
    }

    #[derive(Clone, Default)]
    struct FakeUnitOfWork {
        state: Arc<Mutex<State>>,
    }

    #[derive(Clone, Default)]
    struct FakeShopRepositoryFactory {
        state: Arc<Mutex<State>>,
    }

    #[derive(Clone, Default)]
    struct FakePartnerShopRepositoryFactory {
        state: Arc<Mutex<State>>,
    }

    struct FakeTx {
        state: Arc<Mutex<State>>,
    }

    struct FakeShopRepository {
        state: Arc<Mutex<State>>,
    }

    struct FakePartnerShopRepository {
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

    #[async_trait::async_trait]
    impl CheckUserAdminUseCase for AllowAdmin {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: CheckUserAdminRequest,
        ) -> Result<
            user_service::use_cases::queries::check_user_admin::CheckUserAdminResult,
            CheckUserAdminError,
        > {
            Ok(user_service::use_cases::queries::check_user_admin::CheckUserAdminResult)
        }
    }

    impl ShopRepositoryFactory<FakeTx> for FakeShopRepositoryFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl ShopRepository + 'tx {
            FakeShopRepository {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ShopRepository for FakeShopRepository {
        async fn find_by_id(
            &mut self,
            _id: ShopId,
        ) -> Result<Option<StoredShop>, ShopRepositoryError> {
            with_state(&self.state, |state| {
                state.counts.find_by_id += 1;
                Ok(state.shop_by_id.clone())
            })
        }

        async fn find_by_slug(
            &mut self,
            _slug_id: &ShopSlugId,
        ) -> Result<Option<StoredShop>, ShopRepositoryError> {
            Ok(None)
        }

        async fn insert(&mut self, shop: &Shop) -> Result<StoredShop, ShopRepositoryError> {
            Ok(stored_shop(shop.clone()))
        }

        async fn update(
            &mut self,
            shop: &Shop,
            _expected_version: ShopStorageVersion,
        ) -> Result<StoredShop, ShopRepositoryError> {
            Ok(stored_shop(shop.clone()))
        }
    }

    impl PartnerShopRepositoryFactory<FakeTx> for FakePartnerShopRepositoryFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl PartnerShopRepository + 'tx {
            FakePartnerShopRepository {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl PartnerShopRepository for FakePartnerShopRepository {
        async fn grant(
            &mut self,
            _user_id: UserId,
            _shop_id: ShopId,
        ) -> Result<(), PartnerShopRepositoryError> {
            with_state(&self.state, |state| {
                state.counts.grant += 1;
                match state.grant_error {
                    Some(kind) => Err(partner_repo_error(kind)),
                    None => Ok(()),
                }
            })
        }
    }

    #[tokio::test]
    async fn should_grant_partner_shop_when_shop_exists() {
        let state = shared_state();
        let existing = shop("Grant Shop");
        let shop_id = existing.id();
        let user_id = UserId::new();
        with_state(&state, |state| {
            state.shop_by_id = Some(stored_shop(existing))
        });
        let handler = GrantPartnerShopHandler::new(
            uow(&state),
            shop_repo(&state),
            partner_repo(&state),
            AllowAdmin,
        );

        let result = handler
            .execute(
                &system_context(),
                GrantPartnerShopCommand { user_id, shop_id },
            )
            .await;

        assert!(
            matches!(result, Ok(ref value) if value.user_id == user_id && value.shop_id == shop_id)
        );
        assert_counts(&state, |counts| {
            assert_eq!(1, counts.find_by_id);
            assert_eq!(1, counts.grant);
            assert_eq!(1, counts.commit);
        });
    }

    #[tokio::test]
    async fn should_cover_grant_partner_shop_errors() {
        let state = shared_state();
        let handler = GrantPartnerShopHandler::new(
            uow(&state),
            shop_repo(&state),
            partner_repo(&state),
            AllowAdmin,
        );
        let not_found = handler
            .execute(
                &system_context(),
                GrantPartnerShopCommand {
                    user_id: UserId::new(),
                    shop_id: ShopId::new(),
                },
            )
            .await;
        assert!(matches!(
            not_found,
            Err(GrantPartnerShopError::ShopNotFound { source: None })
        ));
        assert_counts(&state, |counts| {
            assert_eq!(0, counts.grant);
            assert_eq!(0, counts.commit);
        });

        let state = shared_state();
        with_state(&state, |state| state.begin_error = true);
        let handler = GrantPartnerShopHandler::new(
            uow(&state),
            shop_repo(&state),
            partner_repo(&state),
            AllowAdmin,
        );
        let begin = handler
            .execute(
                &system_context(),
                GrantPartnerShopCommand {
                    user_id: UserId::new(),
                    shop_id: ShopId::new(),
                },
            )
            .await;
        assert!(matches!(
            begin,
            Err(GrantPartnerShopError::BeginTransactionFailed)
        ));

        let state = shared_state();
        let existing = shop("Grant Error");
        let shop_id = existing.id();
        with_state(&state, |state| {
            state.shop_by_id = Some(stored_shop(existing));
            state.grant_error = Some(PartnerRepoErrorKind::UserNotFound);
        });
        let handler = GrantPartnerShopHandler::new(
            uow(&state),
            shop_repo(&state),
            partner_repo(&state),
            AllowAdmin,
        );
        let grant = handler
            .execute(
                &system_context(),
                GrantPartnerShopCommand {
                    user_id: UserId::new(),
                    shop_id,
                },
            )
            .await;
        assert!(matches!(
            grant,
            Err(GrantPartnerShopError::UserNotFound { source: Some(_) })
        ));
        assert_counts(&state, |counts| assert_eq!(0, counts.commit));

        let state = shared_state();
        let existing = shop("Commit Grant");
        let shop_id = existing.id();
        with_state(&state, |state| {
            state.shop_by_id = Some(stored_shop(existing));
            state.commit_error = true;
        });
        let handler = GrantPartnerShopHandler::new(
            uow(&state),
            shop_repo(&state),
            partner_repo(&state),
            AllowAdmin,
        );
        let commit = handler
            .execute(
                &system_context(),
                GrantPartnerShopCommand {
                    user_id: UserId::new(),
                    shop_id,
                },
            )
            .await;
        assert!(matches!(
            commit,
            Err(GrantPartnerShopError::CommitTransactionFailed)
        ));
    }

    #[tokio::test]
    async fn should_map_remaining_partner_repository_errors() {
        for (kind, assertion) in [
            (PartnerRepoErrorKind::ShopNotFound, "shop_not_found"),
            (
                PartnerRepoErrorKind::TemporarilyUnavailable,
                "temporarily_unavailable",
            ),
            (PartnerRepoErrorKind::Internal, "internal"),
        ] {
            let state = shared_state();
            let existing = shop("Grant Map");
            let shop_id = existing.id();
            with_state(&state, |state| {
                state.shop_by_id = Some(stored_shop(existing));
                state.grant_error = Some(kind);
            });
            let handler = GrantPartnerShopHandler::new(
                uow(&state),
                shop_repo(&state),
                partner_repo(&state),
                AllowAdmin,
            );
            let result = handler
                .execute(
                    &system_context(),
                    GrantPartnerShopCommand {
                        user_id: UserId::new(),
                        shop_id,
                    },
                )
                .await;
            match assertion {
                "shop_not_found" => assert!(matches!(
                    result,
                    Err(GrantPartnerShopError::ShopNotFound { source: Some(_) })
                )),
                "temporarily_unavailable" => assert!(matches!(
                    result,
                    Err(GrantPartnerShopError::TemporarilyUnavailable { .. })
                )),
                _ => assert!(matches!(
                    result,
                    Err(GrantPartnerShopError::Internal { .. })
                )),
            }
            assert_counts(&state, |counts| assert_eq!(0, counts.commit));
        }
    }

    fn shop_repo(state: &Arc<Mutex<State>>) -> FakeShopRepositoryFactory {
        FakeShopRepositoryFactory {
            state: Arc::clone(state),
        }
    }

    fn partner_repo(state: &Arc<Mutex<State>>) -> FakePartnerShopRepositoryFactory {
        FakePartnerShopRepositoryFactory {
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

    fn partner_repo_error(kind: PartnerRepoErrorKind) -> PartnerShopRepositoryError {
        match kind {
            PartnerRepoErrorKind::UserNotFound => PartnerShopRepositoryError::UserNotFound {
                source: static_error("user"),
            },
            PartnerRepoErrorKind::ShopNotFound => PartnerShopRepositoryError::ShopNotFound {
                source: static_error("shop"),
            },
            PartnerRepoErrorKind::TemporarilyUnavailable => {
                PartnerShopRepositoryError::TemporarilyUnavailable {
                    source: static_error("temporary"),
                }
            }
            PartnerRepoErrorKind::Internal => PartnerShopRepositoryError::Internal {
                source: static_error("internal"),
            },
        }
    }

    fn shop(name: &str) -> Shop {
        Shop::create(NewShop {
            id: ShopId::new(),
            name: ShopName::from(name),
            shop_type: ShopType::CommercialDealer,
            domains: HashSet::new(),
            shopify: None,
            woocommerce: None,
            presentation: ShopPresentation::default(),
            address: None,
            contact: ShopContact::default(),
            partner_status: ShopPartnerStatus::Scraped,
            affiliate_configuration: None,
        })
    }

    fn stored_shop(shop: Shop) -> StoredShop {
        StoredShop {
            shop,
            version: ShopStorageVersion::INITIAL,
            created: time::OffsetDateTime::now_utc(),
            updated: time::OffsetDateTime::now_utc(),
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
