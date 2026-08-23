use crate::ports::{ShopRepository, ShopRepositoryError, ShopRepositoryFactory};
use application::error::BoxError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use application::transaction::{Transaction, UnitOfWork};
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_slug_id::ShopSlugId;
use shop_core::{partner_status::ShopPartnerStatus, shop::Shop};
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeShopPartnerStatusCommand {
    pub shop_id: ShopId,
    pub partner_status: ShopPartnerStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeShopPartnerStatusResult {
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,
    pub partner_status: ShopPartnerStatus,
}

#[derive(Debug, thiserror::Error)]
pub enum ChangeShopPartnerStatusError {
    #[error("authenticated actor required to change shop partner status")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("shop not found")]
    ShopNotFound,
    #[error("concurrent shop update")]
    ConcurrencyConflict,
    #[error("temporary shop persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted shop state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal shop persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin change shop partner status transaction")]
    BeginTransactionFailed,
    #[error("failed to commit change shop partner status transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait ChangeShopPartnerStatusUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: ChangeShopPartnerStatusCommand,
    ) -> Result<ChangeShopPartnerStatusResult, ChangeShopPartnerStatusError>;
}

pub struct ChangeShopPartnerStatusHandler<U, R, A> {
    unit_of_work: U,
    shops: R,
    check_user_admin: A,
}

impl<U, R, A> ChangeShopPartnerStatusHandler<U, R, A> {
    pub fn new(unit_of_work: U, shops: R, check_user_admin: A) -> Self {
        Self {
            unit_of_work,
            shops,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> ChangeShopPartnerStatusUseCase for ChangeShopPartnerStatusHandler<U, R, A>
where
    U: UnitOfWork,
    R: ShopRepositoryFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "change_shop_partner_status",
        skip_all,
        fields(
            shop_id = %command.shop_id,
            partner_status = ?command.partner_status,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: ChangeShopPartnerStatusCommand,
    ) -> Result<ChangeShopPartnerStatusResult, ChangeShopPartnerStatusError> {
        context
            .require()
            .credential_capability(CredentialCapability::ShopsWrite)
            .authorize::<ChangeShopPartnerStatusError>()?;
        ensure_admin_or_internal(context, &self.check_user_admin).await?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ChangeShopPartnerStatusError::BeginTransactionFailed)?;

        let crate::ports::StoredShop {
            mut shop, version, ..
        } = self
            .shops
            .in_transaction(&mut tx)
            .find_by_id(command.shop_id)
            .await?
            .ok_or(ChangeShopPartnerStatusError::ShopNotFound)?;

        let outcome = shop.change_partner_status(command.partner_status);
        if outcome.changed() {
            shop = self
                .shops
                .in_transaction(&mut tx)
                .update(&shop, version)
                .await?
                .shop;
        }

        tx.commit()
            .await
            .map_err(|_| ChangeShopPartnerStatusError::CommitTransactionFailed)?;

        tracing::info!(
            event = "shop.partner_status_changed",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            shop_id = %shop.id(),
            partner_status = ?shop.partner_status(),
            changed = outcome.changed(),
            outcome = "success",
        );

        Ok(ChangeShopPartnerStatusResult::from(&shop))
    }
}

impl From<&Shop> for ChangeShopPartnerStatusResult {
    fn from(shop: &Shop) -> Self {
        Self {
            shop_id: shop.id(),
            shop_slug_id: shop.slug_id().clone(),
            name: shop.name().clone(),
            partner_status: shop.partner_status(),
        }
    }
}

async fn ensure_admin_or_internal<A>(
    context: &OperationContext,
    check_user_admin: &A,
) -> Result<(), ChangeShopPartnerStatusError>
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
        Principal::Anonymous => Err(ChangeShopPartnerStatusError::AuthenticatedActorRequired),
    }
}

fn map_admin_error(error: CheckUserAdminError) -> ChangeShopPartnerStatusError {
    match error {
        CheckUserAdminError::AuthenticatedActorRequired => {
            ChangeShopPartnerStatusError::AuthenticatedActorRequired
        }
        CheckUserAdminError::Forbidden => ChangeShopPartnerStatusError::Forbidden,
        CheckUserAdminError::TemporarilyUnavailable { source } => {
            ChangeShopPartnerStatusError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source }
        | CheckUserAdminError::Internal { source } => {
            ChangeShopPartnerStatusError::Internal { source }
        }
        CheckUserAdminError::BeginTransactionFailed
        | CheckUserAdminError::CommitTransactionFailed => {
            ChangeShopPartnerStatusError::TemporarilyUnavailable {
                source: application::error::static_error("check user admin transaction failed"),
            }
        }
    }
}

impl From<OperationAuthorizationError> for ChangeShopPartnerStatusError {
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

impl From<ShopRepositoryError> for ChangeShopPartnerStatusError {
    fn from(error: ShopRepositoryError) -> Self {
        match error {
            ShopRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            ShopRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ShopRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            ShopRepositoryError::SlugConflict { source }
            | ShopRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{ShopRepository, ShopRepositoryFactory, ShopStorageVersion, StoredShop};
    use application::error::static_error;
    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use application::transaction::{TransactionError, UnitOfWork};

    use shop_core::shop::{NewShop, ShopContact, ShopPresentation};
    use shop_core::shop_type::ShopType;
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Copy)]
    enum RepoErrorKind {
        ConcurrencyConflict,
        Internal,
    }

    #[derive(Clone, Copy)]
    struct AllowAdmin;

    #[derive(Default)]
    struct Counts {
        begin: usize,
        commit: usize,
        find_by_id: usize,
        update: usize,
    }

    #[derive(Default)]
    struct State {
        begin_error: bool,
        commit_error: bool,
        shop_by_id: Option<StoredShop>,
        find_by_id_error: Option<RepoErrorKind>,
        update_error: Option<RepoErrorKind>,
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

    struct FakeTx {
        state: Arc<Mutex<State>>,
    }

    struct FakeShopRepository {
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
                match state.find_by_id_error {
                    Some(kind) => Err(shop_repo_error(kind)),
                    None => Ok(state.shop_by_id.clone()),
                }
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
            with_state(&self.state, |state| {
                state.counts.update += 1;
                match state.update_error {
                    Some(kind) => Err(shop_repo_error(kind)),
                    None => Ok(stored_shop(shop.clone())),
                }
            })
        }
    }

    #[tokio::test]
    async fn should_change_partner_status_and_skip_noop_update() {
        let state = shared_state();
        let existing = shop("Partner Shop");
        let shop_id = existing.id();
        with_state(&state, |state| {
            state.shop_by_id = Some(stored_shop(existing))
        });
        let handler =
            ChangeShopPartnerStatusHandler::new(uow(&state), shop_repo(&state), AllowAdmin);

        let changed = handler
            .execute(
                &system_context(),
                ChangeShopPartnerStatusCommand {
                    shop_id,
                    partner_status: ShopPartnerStatus::Partnered,
                },
            )
            .await;

        assert!(
            matches!(changed, Ok(ref value) if value.partner_status == ShopPartnerStatus::Partnered)
        );
        assert_counts(&state, |counts| assert_eq!(1, counts.update));

        let state = shared_state();
        let existing = partnered_shop("Partner Shop");
        let shop_id = existing.id();
        with_state(&state, |state| {
            state.shop_by_id = Some(stored_shop(existing))
        });
        let handler =
            ChangeShopPartnerStatusHandler::new(uow(&state), shop_repo(&state), AllowAdmin);
        let noop = handler
            .execute(
                &system_context(),
                ChangeShopPartnerStatusCommand {
                    shop_id,
                    partner_status: ShopPartnerStatus::Partnered,
                },
            )
            .await;
        assert!(noop.is_ok());
        assert_counts(&state, |counts| {
            assert_eq!(0, counts.update);
            assert_eq!(1, counts.commit);
        });
    }

    #[tokio::test]
    async fn should_cover_change_partner_status_errors() {
        let state = shared_state();
        let handler =
            ChangeShopPartnerStatusHandler::new(uow(&state), shop_repo(&state), AllowAdmin);
        let not_found = handler
            .execute(
                &system_context(),
                ChangeShopPartnerStatusCommand {
                    shop_id: ShopId::new(),
                    partner_status: ShopPartnerStatus::Partnered,
                },
            )
            .await;
        assert!(matches!(
            not_found,
            Err(ChangeShopPartnerStatusError::ShopNotFound)
        ));
        assert_counts(&state, |counts| assert_eq!(0, counts.commit));

        let state = shared_state();
        with_state(&state, |state| state.begin_error = true);
        let handler =
            ChangeShopPartnerStatusHandler::new(uow(&state), shop_repo(&state), AllowAdmin);
        let begin = handler
            .execute(
                &system_context(),
                ChangeShopPartnerStatusCommand {
                    shop_id: ShopId::new(),
                    partner_status: ShopPartnerStatus::Partnered,
                },
            )
            .await;
        assert!(matches!(
            begin,
            Err(ChangeShopPartnerStatusError::BeginTransactionFailed)
        ));

        let state = shared_state();
        let existing = shop("Bad Update");
        let shop_id = existing.id();
        with_state(&state, |state| {
            state.shop_by_id = Some(stored_shop(existing));
            state.update_error = Some(RepoErrorKind::ConcurrencyConflict);
        });
        let handler =
            ChangeShopPartnerStatusHandler::new(uow(&state), shop_repo(&state), AllowAdmin);
        let repo = handler
            .execute(
                &system_context(),
                ChangeShopPartnerStatusCommand {
                    shop_id,
                    partner_status: ShopPartnerStatus::Partnered,
                },
            )
            .await;
        assert!(matches!(
            repo,
            Err(ChangeShopPartnerStatusError::ConcurrencyConflict)
        ));
        assert_counts(&state, |counts| assert_eq!(0, counts.commit));

        let state = shared_state();
        let existing = shop("Commit Fail");
        let shop_id = existing.id();
        with_state(&state, |state| {
            state.shop_by_id = Some(stored_shop(existing));
            state.commit_error = true;
        });
        let handler =
            ChangeShopPartnerStatusHandler::new(uow(&state), shop_repo(&state), AllowAdmin);
        let commit = handler
            .execute(
                &system_context(),
                ChangeShopPartnerStatusCommand {
                    shop_id,
                    partner_status: ShopPartnerStatus::Partnered,
                },
            )
            .await;
        assert!(matches!(
            commit,
            Err(ChangeShopPartnerStatusError::CommitTransactionFailed)
        ));
    }

    #[tokio::test]
    async fn should_map_internal_when_find_by_id_fails() {
        let state = shared_state();
        with_state(&state, |state| {
            state.find_by_id_error = Some(RepoErrorKind::Internal)
        });
        let handler =
            ChangeShopPartnerStatusHandler::new(uow(&state), shop_repo(&state), AllowAdmin);

        let result = handler
            .execute(
                &system_context(),
                ChangeShopPartnerStatusCommand {
                    shop_id: ShopId::new(),
                    partner_status: ShopPartnerStatus::Partnered,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(ChangeShopPartnerStatusError::Internal { .. })
        ));
    }

    fn shop_repo(state: &Arc<Mutex<State>>) -> FakeShopRepositoryFactory {
        FakeShopRepositoryFactory {
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

    fn shop_repo_error(kind: RepoErrorKind) -> ShopRepositoryError {
        match kind {
            RepoErrorKind::ConcurrencyConflict => ShopRepositoryError::ConcurrencyConflict,
            RepoErrorKind::Internal => ShopRepositoryError::Internal {
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

    fn partnered_shop(name: &str) -> Shop {
        Shop::create(NewShop {
            partner_status: ShopPartnerStatus::Partnered,
            ..new_shop(name)
        })
    }

    fn new_shop(name: &str) -> NewShop {
        NewShop {
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
        }
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
