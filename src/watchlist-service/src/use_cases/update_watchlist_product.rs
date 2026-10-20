use crate::ports::{
    WatchlistQuotaReadError, WatchlistQuotaReader, WatchlistQuotaReaderFactory,
    WatchlistRepository, WatchlistRepositoryError, WatchlistRepositoryFactory,
};
use crate::tier_policy::active_watchlist_quota;
use application::error::{BoxError, box_error};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use application::transaction::{Transaction, UnitOfWork};
use fxrate_service::ports::{FxRateSnapshotRepository, FxRateSnapshotRepositoryFactory};
use localization::Language;
use money::Currency;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::{
    listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
};
use product_listing_service::ports::{
    ProductListingDetailsReadRequest, ProductListingDetailsReader,
    ProductListingDetailsReaderFactory, ProductListingLifecycleGuard,
    ProductListingLifecycleGuardError, ProductListingLifecycleGuardFactory,
};
use product_listing_service::shipping::ShippingEnrichment;
use product_listing_service::use_cases::{
    GetProductListingError, PersonalizedProductListingDetailsView, ProductListingLookup,
    present_product_details, redact_hidden_product,
};
use product_listing_service::user_state::ProductListingUserStateDisclosure;
use time::OffsetDateTime;
use user_core::user_id::UserId;
use user_service::ports::{
    UserTierEntitlements, UserTierEntitlementsError, UserTierEntitlementsFactory,
};
use watchlist_core::watchlist_state::WatchlistState;

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateWatchlistProductListingCommand {
    pub user_id: UserId,
    pub product_listing_id: ProductListingId,
    pub notifications: Option<bool>,
    pub state: Option<WatchlistState>,
    pub language: Language,
    pub currency: Currency,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateWatchlistProductListingResult {
    pub product: PersonalizedProductListingDetailsView,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateWatchlistProductListingError {
    #[error("watchlist product presentation failed")]
    Presentation(
        #[from]
        #[source]
        GetProductListingError,
    ),
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("watchlist entry not found")]
    NotFound,
    #[error("watchlist entry changed concurrently")]
    ConcurrencyConflict,
    #[error("user not found")]
    UserNotFound,
    #[error("product listing not found")]
    ProductListingNotFound,
    #[error("product listing is unavailable")]
    ProductListingUnavailable,
    #[error("watchlist quota exceeded: {active_count}/{quota} active entries are already in use")]
    WatchlistQuotaExceeded { active_count: usize, quota: usize },
    #[error("user tier entitlement lock failed")]
    UserTierEntitlementsLockFailed {
        #[source]
        source: BoxError,
    },
    #[error("watchlist quota read failed")]
    WatchlistQuotaReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("temporary watchlist persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted watchlist state")]
    InvalidPersistedState,
    #[error("failed to begin watchlist transaction")]
    BeginTransactionFailed(#[source] application::error::BoxError),
    #[error("failed to commit watchlist transaction")]
    CommitTransactionFailed(#[source] application::error::BoxError),
}

#[async_trait::async_trait]
pub trait UpdateWatchlistProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateWatchlistProductListingCommand,
    ) -> Result<UpdateWatchlistProductListingResult, UpdateWatchlistProductListingError>;
}

pub struct UpdateWatchlistProductListingHandler<U, R, Q, A, G, D, F> {
    unit_of_work: U,
    watchlist: R,
    quotas: Q,
    tier_entitlements: A,
    product_listing_lifecycle: G,
    details_reader: D,
    fx_rates: F,
    shipping: ShippingEnrichment,
}

impl<U, R, Q, A, G, D, F> UpdateWatchlistProductListingHandler<U, R, Q, A, G, D, F> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        unit_of_work: U,
        watchlist: R,
        quotas: Q,
        tier_entitlements: A,
        product_listing_lifecycle: G,
        details_reader: D,
        fx_rates: F,
        shipping: ShippingEnrichment,
    ) -> Self {
        Self {
            unit_of_work,
            watchlist,
            quotas,
            tier_entitlements,
            product_listing_lifecycle,
            details_reader,
            fx_rates,
            shipping,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, Q, A, G, D, F> UpdateWatchlistProductListingUseCase
    for UpdateWatchlistProductListingHandler<U, R, Q, A, G, D, F>
where
    U: UnitOfWork,
    R: WatchlistRepositoryFactory<U::Tx>,
    Q: WatchlistQuotaReaderFactory<U::Tx>,
    A: UserTierEntitlementsFactory<U::Tx>,
    G: ProductListingLifecycleGuardFactory<U::Tx>,
    D: ProductListingDetailsReaderFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(name = "update_watchlist_product", skip_all, fields(user_id = %command.user_id, product_listing_id = %command.product_listing_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateWatchlistProductListingCommand,
    ) -> Result<UpdateWatchlistProductListingResult, UpdateWatchlistProductListingError> {
        authorize_write(context, command.user_id)?;

        let valuation_at = OffsetDateTime::now_utc();
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            UpdateWatchlistProductListingError::BeginTransactionFailed(Box::new(source))
        })?;
        if self
            .watchlist
            .in_transaction(&mut tx)
            .find_by_user_and_product(command.user_id, command.product_listing_id)
            .await?
            .is_none()
        {
            return Err(UpdateWatchlistProductListingError::NotFound);
        }

        let (mut product, snapshot, current_pricing) = async {
            let factual_details = self
                .details_reader
                .in_transaction(&mut tx)
                .find_details(&ProductListingDetailsReadRequest {
                    lookup: ProductListingLookup::ById(command.product_listing_id),
                    language: command.language,
                    user_id: Some(command.user_id),
                })
                .await?
                .ok_or(GetProductListingError::NotFound)?;
            let hidden = factual_details
                .user_state
                .as_ref()
                .ok_or(GetProductListingError::ProductListingDetailsReadModelInvalid)?
                .search_filter
                .hidden;
            let sale_observation = factual_details.item.sale_observation.filter(|_| {
                factual_details.item.availability == Some(ListingAvailability::SoldOut)
                    || factual_details.item.lifecycle == ListingLifecycle::Withdrawn
            });
            let snapshot = match sale_observation {
                Some(observation) => {
                    self.fx_rates
                        .in_transaction(&mut tx)
                        .find_by_id(observation.fx_rate_id())
                        .await?
                }
                None => {
                    self.fx_rates
                        .in_transaction(&mut tx)
                        .find_latest_at_or_before(valuation_at)
                        .await?
                }
            }
            .ok_or(GetProductListingError::PricingFxSnapshotMissing)?;
            let mut product =
                present_product_details(factual_details, &snapshot, command.currency)?;
            if hidden {
                redact_hidden_product(&mut product.item)?;
            }
            Ok::<_, GetProductListingError>((product, snapshot, sale_observation.is_none()))
        }
        .await?;
        // No writes have occurred. Release this connection before shipping opens its own
        // transactions on the single-connection API pool and locks the destination owner.
        drop(tx);
        self.shipping
            .details(
                context,
                std::slice::from_mut(&mut product),
                command.currency,
                current_pricing.then_some(&snapshot),
                valuation_at,
            )
            .await
            .map_err(GetProductListingError::from)?;

        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            UpdateWatchlistProductListingError::BeginTransactionFailed(Box::new(source))
        })?;
        // Revalidate ownership and load the authoritative version after response preparation.
        let loaded = self
            .watchlist
            .in_transaction(&mut tx)
            .find_by_user_and_product(command.user_id, command.product_listing_id)
            .await?
            .ok_or(UpdateWatchlistProductListingError::NotFound)?;
        let expected_version = loaded.version;
        let mut entry = loaded.value;
        let reactivating =
            matches!(command.state, Some(WatchlistState::Active)) && !entry.state().is_active();
        if reactivating {
            let tier = self
                .tier_entitlements
                .in_transaction(&mut tx)
                .lock_user_tier(command.user_id)
                .await
                .map_err(tier_entitlements_error)?
                .ok_or(UpdateWatchlistProductListingError::UserNotFound)?;
            match self
                .product_listing_lifecycle
                .in_transaction(&mut tx)
                .lock_and_find_lifecycle(command.product_listing_id)
                .await
                .map_err(product_listing_lifecycle_guard_error)?
            {
                None => return Err(UpdateWatchlistProductListingError::ProductListingNotFound),
                Some(ListingLifecycle::Withdrawn) => {
                    return Err(UpdateWatchlistProductListingError::ProductListingUnavailable);
                }
                Some(ListingLifecycle::Active) => {}
            }
            if let Some(quota) = active_watchlist_quota(tier) {
                let active_count = self
                    .quotas
                    .in_transaction(&mut tx)
                    .count_active_for_user(command.user_id)
                    .await
                    .map_err(watchlist_quota_read_error)?;
                if active_count >= quota {
                    return Err(UpdateWatchlistProductListingError::WatchlistQuotaExceeded {
                        active_count,
                        quota,
                    });
                }
            }
        }
        let mut changed = false;
        if let Some(notifications) = command.notifications
            && entry.notifications() != notifications
        {
            entry.change_notifications(notifications);
            changed = true;
        }
        if let Some(state) = command.state
            && entry.state() != state
        {
            entry.change_state(state);
            changed = true;
        }
        if changed {
            entry = match self
                .watchlist
                .in_transaction(&mut tx)
                .update(&entry, expected_version)
                .await
            {
                Ok(entry) => entry.into_value(),
                Err(WatchlistRepositoryError::ConcurrencyConflict) => {
                    tracing::warn!(
                        event = "watchlist_product.update_rejected",
                        actor_type = context.principal.kind(),
                        actor_id = ?context.principal.actor_id(),
                        user_id = %command.user_id,
                        product_listing_id = %command.product_listing_id,
                        outcome = "concurrency_conflict",
                    );
                    return Err(UpdateWatchlistProductListingError::ConcurrencyConflict);
                }
                Err(error) => return Err(error.into()),
            };
        }

        let user_state = product
            .user_state
            .as_mut()
            .ok_or(GetProductListingError::ProductListingDetailsReadModelInvalid)?;
        // The reader reports membership; the persisted aggregate owns activation and settings.
        user_state.watchlist.watching = entry.state().is_active();
        user_state.watchlist.notifications = entry.notifications();

        tx.commit().await.map_err(|source| {
            UpdateWatchlistProductListingError::CommitTransactionFailed(Box::new(source))
        })?;
        tracing::info!(
            event = "watchlist_product.updated",
            actor_type = context.principal.kind(),
            actor_id = ?context.principal.actor_id(),
            user_id = %command.user_id,
            product_listing_id = %command.product_listing_id,
            outcome = if changed { "success" } else { "unchanged" },
        );
        Ok(UpdateWatchlistProductListingResult {
            product: ProductListingUserStateDisclosure::for_principal(&context.principal)
                .disclose(product),
        })
    }
}

fn authorize_write(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), UpdateWatchlistProductListingError> {
    context
        .require()
        .credential_capability(CredentialCapability::WatchlistWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<UpdateWatchlistProductListingError>()
}

impl From<OperationAuthorizationError> for UpdateWatchlistProductListingError {
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

fn tier_entitlements_error(error: UserTierEntitlementsError) -> UpdateWatchlistProductListingError {
    match error {
        UserTierEntitlementsError::LockFailed { source }
        | UserTierEntitlementsError::ReconciliationFailed { source } => {
            UpdateWatchlistProductListingError::UserTierEntitlementsLockFailed { source }
        }
    }
}

fn product_listing_lifecycle_guard_error(
    error: ProductListingLifecycleGuardError,
) -> UpdateWatchlistProductListingError {
    match error {
        ProductListingLifecycleGuardError::LockFailed { source } => {
            UpdateWatchlistProductListingError::TemporarilyUnavailable { source }
        }
        ProductListingLifecycleGuardError::InvalidListingLifecyclePersisted => {
            UpdateWatchlistProductListingError::InvalidPersistedState
        }
    }
}

fn watchlist_quota_read_error(
    error: WatchlistQuotaReadError,
) -> UpdateWatchlistProductListingError {
    UpdateWatchlistProductListingError::WatchlistQuotaReadFailed {
        source: box_error(error),
    }
}

impl From<WatchlistRepositoryError> for UpdateWatchlistProductListingError {
    fn from(value: WatchlistRepositoryError) -> Self {
        match value {
            WatchlistRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            WatchlistRepositoryError::InvalidPersistedState => Self::InvalidPersistedState,
            WatchlistRepositoryError::LookupFailed { source }
            | WatchlistRepositoryError::InsertFailed { source }
            | WatchlistRepositoryError::UpdateFailed { source }
            | WatchlistRepositoryError::DeleteFailed { source } => {
                Self::TemporarilyUnavailable { source }
            }
            error @ WatchlistRepositoryError::AlreadyExists => Self::TemporarilyUnavailable {
                source: box_error(error),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(dead_code)]

    use super::*;

    use application::error::static_error;
    use application::personalized::Personalized;
    use domain_primitives::event_id::EventId;
    use fxrate_core::{
        FX_RATE_SCALE, FxRateGeneration, FxRateId, FxRateQuote, FxRateSnapshot, FxRateSource,
        NewFxRateSnapshot,
    };
    use fxrate_service::ports::{FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError};
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use localization::Localized;
    use money::Price;
    use product_listing_core::{
        content_policy::{ContentPolicyDecision, SensitiveContentCategory},
        description::Description,
        listing_lifecycle::ListingLifecycle,
        product_listing::{ListingSaleObservation, ProductListingPricing},
        product_listing_image::ProductListingImage,
        product_listing_price::ProductListingPrice,
        product_listing_slug_id::ProductListingSlugId,
        source_listing_id::SourceListingId,
        title::Title,
    };
    use product_listing_service::ports::{
        ListingSourceSummary, PersonalizedProductListingDetailsReadModel,
        ProductListingDetailsReadError, ProductListingDetailsReadModel,
    };
    use product_listing_service::ports::{
        ProductListingLifecycleGuard, ProductListingLifecycleGuardError,
        ProductListingLifecycleGuardFactory,
    };
    use product_listing_service::use_cases::ProductListingPricingValuation;
    use product_listing_service::user_state::{
        ProductListingUserState, ProductListingUserStateView, WatchlistUserState,
    };
    use strum::IntoEnumIterator;
    use url::Url;

    use crate::ports::{
        VersionedWatchlistProductListing, WatchlistProductListingView, WatchlistQuotaReadError,
        WatchlistQuotaReader, WatchlistQuotaReaderFactory, WatchlistReadError, WatchlistReader,
        WatchlistReaderFactory, WatchlistRepository, WatchlistRepositoryError,
        WatchlistRepositoryFactory, WatchlistStorageVersion,
    };
    use application::operation_context::{
        CorrelationId, CredentialCapability, OperationContext, Principal, RequestId,
    };
    use application::transaction::{Transaction, TransactionError, UnitOfWork};
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;
    use user_core::tier::UserTier;
    use user_service::ports::{
        UserTierEntitlements, UserTierEntitlementsError, UserTierEntitlementsFactory,
    };
    use watchlist_core::watchlist_state::WatchlistState;
    use watchlist_core::{NewWatchlistProductListing, WatchlistProductListing};

    #[derive(Clone, Default)]
    struct TestUnitOfWork {
        state: SharedState,
        fail_begin: bool,
        fail_commit: bool,
    }

    struct TestTransaction {
        state: SharedState,
        persisted: SharedState,
        fail_commit: bool,
    }

    #[derive(Default)]
    struct PresentationState {
        details_result: Option<
            Result<
                Option<PersonalizedProductListingDetailsReadModel>,
                ProductListingDetailsReadError,
            >,
        >,
        snapshot_result: Option<Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError>>,
        details_requests: Vec<ProductListingDetailsReadRequest>,
        current_snapshot_requests: Vec<OffsetDateTime>,
        sale_snapshot_requests: Vec<FxRateId>,
    }

    struct TestPresentationFactory;

    struct TestPresentationPort {
        state: SharedState,
    }

    #[derive(Clone, Default)]
    struct TestWatchlistFactory {
        state: SharedState,
    }

    #[derive(Clone)]
    struct TestAccountFactory {
        tier: UserTier,
        state: SharedState,
    }

    #[derive(Clone, Copy)]
    enum TestLifecycleGuardOutcome {
        Active,
        Missing,
        Withdrawn,
        LockFailed,
        InvalidPersistedState,
    }

    #[derive(Clone)]
    struct TestLifecycleGuardFactory {
        state: SharedState,
        outcome: TestLifecycleGuardOutcome,
    }

    struct TestLifecycleGuard {
        state: SharedState,
        outcome: TestLifecycleGuardOutcome,
    }

    struct TestAccountReader {
        tier: UserTier,
        state: SharedState,
    }

    #[derive(Clone, Default)]
    struct SharedState {
        entries: Arc<Mutex<Vec<VersionedWatchlistProductListing>>>,
        committed: Arc<Mutex<bool>>,
        updated: Arc<Mutex<usize>>,
        deleted: Arc<Mutex<usize>>,
        lifecycle_guard_calls: Arc<Mutex<usize>>,
        begin_count: Arc<Mutex<usize>>,
        open_transactions: Arc<Mutex<usize>>,
        presentation: Arc<Mutex<PresentationState>>,
        entries_before_mutation: Option<Vec<VersionedWatchlistProductListing>>,
        persisted_entry: Option<WatchlistProductListing>,
        force_concurrency_conflict: bool,
    }

    struct TestWatchlistPort {
        state: SharedState,
    }

    impl SharedState {
        fn with_entry(entry: WatchlistProductListing) -> Self {
            let state = Self::default();
            state.push(entry);
            state
        }

        fn push(&self, entry: WatchlistProductListing) {
            if let Ok(mut entries) = self.entries.lock() {
                entries.push(VersionedWatchlistProductListing::new(
                    entry,
                    WatchlistStorageVersion::INITIAL,
                ));
            }
        }

        fn committed(&self) -> bool {
            self.committed.lock().map(|value| *value).unwrap_or(false)
        }

        fn updated(&self) -> usize {
            self.updated.lock().map(|value| *value).unwrap_or(0)
        }

        fn deleted(&self) -> usize {
            self.deleted.lock().map(|value| *value).unwrap_or(0)
        }

        fn lifecycle_guard_calls(&self) -> usize {
            self.lifecycle_guard_calls
                .lock()
                .map(|value| *value)
                .unwrap_or(0)
        }
    }

    impl Drop for TestTransaction {
        fn drop(&mut self) {
            *self
                .state
                .open_transactions
                .lock()
                .expect("open transactions") -= 1;
        }
    }

    #[async_trait::async_trait]
    impl Transaction for TestTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            if self.fail_commit {
                return Err(TransactionError::CommitFailed(
                    application::error::static_error("test transaction failure"),
                ));
            }
            *self.persisted.entries.lock().expect("persisted entries") = self
                .state
                .entries
                .lock()
                .expect("transaction entries")
                .clone();
            self.persisted
                .committed
                .lock()
                .map(|mut committed| *committed = true)
                .map_err(|_| {
                    TransactionError::CommitFailed(application::error::static_error(
                        "test transaction failure",
                    ))
                })
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for TestUnitOfWork {
        type Tx = TestTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            if self.fail_begin {
                return Err(TransactionError::BeginFailed(
                    application::error::static_error("test transaction failure"),
                ));
            }
            let begin_count = {
                let mut count = self.state.begin_count.lock().expect("begin count");
                *count += 1;
                *count
            };
            let mut open = self
                .state
                .open_transactions
                .lock()
                .expect("open transactions");
            assert_eq!(0, *open, "the API pool has only one connection");
            *open += 1;
            if begin_count == 2
                && let Some(entries) = &self.state.entries_before_mutation
            {
                *self.state.entries.lock().expect("persisted entries") = entries.clone();
            }
            let mut state = self.state.clone();
            state.entries = Arc::new(Mutex::new(
                self.state
                    .entries
                    .lock()
                    .expect("persisted entries")
                    .clone(),
            ));
            Ok(TestTransaction {
                state,
                persisted: self.state.clone(),
                fail_commit: self.fail_commit,
            })
        }
    }

    impl WatchlistRepositoryFactory<TestTransaction> for TestWatchlistFactory {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut TestTransaction,
        ) -> impl WatchlistRepository + 'tx {
            TestWatchlistPort {
                state: tx.state.clone(),
            }
        }
    }

    impl WatchlistQuotaReaderFactory<TestTransaction> for TestWatchlistFactory {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut TestTransaction,
        ) -> impl WatchlistQuotaReader + 'tx {
            TestWatchlistPort {
                state: tx.state.clone(),
            }
        }
    }

    impl<Tx> UserTierEntitlementsFactory<Tx> for TestAccountFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut Tx) -> impl UserTierEntitlements + 'tx {
            TestAccountReader {
                tier: self.tier,
                state: self.state.clone(),
            }
        }
    }

    impl<Tx> ProductListingLifecycleGuardFactory<Tx> for TestLifecycleGuardFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut Tx,
        ) -> impl ProductListingLifecycleGuard + 'tx {
            TestLifecycleGuard {
                state: self.state.clone(),
                outcome: self.outcome,
            }
        }
    }

    impl<Tx> WatchlistReaderFactory<Tx> for TestWatchlistFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut Tx) -> impl WatchlistReader + 'tx {
            TestWatchlistPort {
                state: self.state.clone(),
            }
        }
    }

    #[async_trait::async_trait]
    impl UserTierEntitlements for TestAccountReader {
        async fn lock_user_tier(
            &mut self,
            _user_id: UserId,
        ) -> Result<Option<UserTier>, UserTierEntitlementsError> {
            let presentation = self.state.presentation.lock().expect("presentation state");
            assert_eq!(
                1,
                presentation.details_requests.len(),
                "prepare the response before taking the user lock"
            );
            assert_eq!(
                1,
                presentation.current_snapshot_requests.len()
                    + presentation.sale_snapshot_requests.len()
            );
            assert_eq!(0, self.state.updated());
            Ok(Some(self.tier))
        }

        async fn reconcile_for_tier(
            &mut self,
            _user_id: UserId,
            _tier: UserTier,
        ) -> Result<(), UserTierEntitlementsError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl ProductListingLifecycleGuard for TestLifecycleGuard {
        async fn lock_and_find_lifecycle(
            &mut self,
            _product_listing_id: ProductListingId,
        ) -> Result<Option<ListingLifecycle>, ProductListingLifecycleGuardError> {
            self.state
                .lifecycle_guard_calls
                .lock()
                .map(|mut calls| *calls += 1)
                .map_err(|_| ProductListingLifecycleGuardError::LockFailed {
                    source: static_error(
                        "watchlist lifecycle guard test counter mutex is poisoned",
                    ),
                })?;
            match self.outcome {
                TestLifecycleGuardOutcome::Active => Ok(Some(ListingLifecycle::Active)),
                TestLifecycleGuardOutcome::Missing => Ok(None),
                TestLifecycleGuardOutcome::Withdrawn => Ok(Some(ListingLifecycle::Withdrawn)),
                TestLifecycleGuardOutcome::LockFailed => {
                    Err(ProductListingLifecycleGuardError::LockFailed {
                        source: static_error("product listing lifecycle guard test lock failure"),
                    })
                }
                TestLifecycleGuardOutcome::InvalidPersistedState => {
                    Err(ProductListingLifecycleGuardError::InvalidListingLifecyclePersisted)
                }
            }
        }
    }

    #[async_trait::async_trait]
    impl WatchlistQuotaReader for TestWatchlistPort {
        async fn count_active_for_user(
            &mut self,
            user_id: UserId,
        ) -> Result<usize, WatchlistQuotaReadError> {
            self.state
                .entries
                .lock()
                .map_err(|_poisoned| WatchlistQuotaReadError::ReadFailed {
                    source: static_error("watchlist quota test state mutex is poisoned"),
                })
                .map(|entries| {
                    entries
                        .iter()
                        .filter(|entry| {
                            entry.value.user_id() == user_id && entry.value.state().is_active()
                        })
                        .count()
                })
        }
    }

    #[async_trait::async_trait]
    impl WatchlistRepository for TestWatchlistPort {
        async fn find_by_user_and_product(
            &mut self,
            user_id: UserId,
            product_listing_id: ProductListingId,
        ) -> Result<Option<VersionedWatchlistProductListing>, WatchlistRepositoryError> {
            self.state
                .entries
                .lock()
                .map_err(|_| WatchlistRepositoryError::LookupFailed {
                    source: static_error("watchlist test lookup mutex is poisoned"),
                })
                .map(|entries| {
                    entries
                        .iter()
                        .find(|entry| {
                            entry.value.user_id() == user_id
                                && entry.value.product_listing_id() == product_listing_id
                        })
                        .cloned()
                })
        }

        async fn insert(
            &mut self,
            entry: &WatchlistProductListing,
        ) -> Result<VersionedWatchlistProductListing, WatchlistRepositoryError> {
            let mut entries =
                self.state
                    .entries
                    .lock()
                    .map_err(|_| WatchlistRepositoryError::InsertFailed {
                        source: static_error("watchlist test insert mutex is poisoned"),
                    })?;
            if entries.iter().any(|existing| {
                existing.value.user_id() == entry.user_id()
                    && existing.value.product_listing_id() == entry.product_listing_id()
            }) {
                return Err(WatchlistRepositoryError::AlreadyExists);
            }
            let persisted = VersionedWatchlistProductListing::new(
                entry.clone(),
                WatchlistStorageVersion::INITIAL,
            );
            entries.push(persisted.clone());
            Ok(persisted)
        }

        async fn update(
            &mut self,
            entry: &WatchlistProductListing,
            expected_version: WatchlistStorageVersion,
        ) -> Result<VersionedWatchlistProductListing, WatchlistRepositoryError> {
            if self.state.force_concurrency_conflict {
                return Err(WatchlistRepositoryError::ConcurrencyConflict);
            }
            let mut entries =
                self.state
                    .entries
                    .lock()
                    .map_err(|_| WatchlistRepositoryError::UpdateFailed {
                        source: static_error("watchlist test update mutex is poisoned"),
                    })?;
            let Some(existing) = entries.iter_mut().find(|existing| {
                existing.value.user_id() == entry.user_id()
                    && existing.value.product_listing_id() == entry.product_listing_id()
            }) else {
                return Err(WatchlistRepositoryError::UpdateFailed {
                    source: static_error("watchlist test entry is missing"),
                });
            };
            if existing.version != expected_version {
                return Err(WatchlistRepositoryError::ConcurrencyConflict);
            }
            let persisted = VersionedWatchlistProductListing::new(
                self.state
                    .persisted_entry
                    .clone()
                    .unwrap_or_else(|| entry.clone()),
                expected_version.next(),
            );
            *existing = persisted.clone();
            self.state
                .updated
                .lock()
                .map(|mut updated| *updated += 1)
                .map_err(|_| WatchlistRepositoryError::UpdateFailed {
                    source: static_error("watchlist test update counter mutex is poisoned"),
                })?;
            Ok(persisted)
        }

        async fn delete(
            &mut self,
            user_id: UserId,
            product_listing_id: ProductListingId,
            expected_version: WatchlistStorageVersion,
        ) -> Result<(), WatchlistRepositoryError> {
            let mut entries =
                self.state
                    .entries
                    .lock()
                    .map_err(|_| WatchlistRepositoryError::DeleteFailed {
                        source: static_error("watchlist test delete mutex is poisoned"),
                    })?;
            let Some(index) = entries.iter().position(|entry| {
                entry.value.user_id() == user_id
                    && entry.value.product_listing_id() == product_listing_id
            }) else {
                return Err(WatchlistRepositoryError::ConcurrencyConflict);
            };
            if entries[index].version != expected_version {
                return Err(WatchlistRepositoryError::ConcurrencyConflict);
            }
            entries.remove(index);
            self.state
                .deleted
                .lock()
                .map(|mut deleted| *deleted += 1)
                .map_err(|_| WatchlistRepositoryError::DeleteFailed {
                    source: static_error("watchlist test delete counter mutex is poisoned"),
                })
        }
    }

    #[async_trait::async_trait]
    impl WatchlistReader for TestWatchlistPort {
        async fn find_for_user(
            &mut self,
            user_id: UserId,
        ) -> Result<Vec<WatchlistProductListingView>, WatchlistReadError> {
            self.state
                .entries
                .lock()
                .map_err(|_| WatchlistReadError::ReadFailed)
                .map(|entries| {
                    entries
                        .iter()
                        .filter(|entry| entry.value.user_id() == user_id)
                        .map(|entry| WatchlistProductListingView {
                            user_id: entry.value.user_id(),
                            product_listing_id: entry.value.product_listing_id(),
                            notifications: entry.value.notifications(),
                            state: entry.value.state(),
                            created: OffsetDateTime::UNIX_EPOCH,
                            updated: OffsetDateTime::UNIX_EPOCH,
                        })
                        .collect()
                })
        }

        async fn find_user_ids_for_product(
            &mut self,
            product_listing_id: ProductListingId,
        ) -> Result<Vec<UserId>, WatchlistReadError> {
            self.state
                .entries
                .lock()
                .map_err(|_| WatchlistReadError::ReadFailed)
                .map(|entries| {
                    entries
                        .iter()
                        .filter(|entry| entry.value.product_listing_id() == product_listing_id)
                        .map(|entry| entry.value.user_id())
                        .collect()
                })
        }
    }

    impl ProductListingDetailsReaderFactory<TestTransaction> for TestPresentationFactory {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut TestTransaction,
        ) -> impl ProductListingDetailsReader + 'tx {
            TestPresentationPort {
                state: tx.state.clone(),
            }
        }
    }

    impl FxRateSnapshotRepositoryFactory<TestTransaction> for TestPresentationFactory {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut TestTransaction,
        ) -> impl FxRateSnapshotRepository + 'tx {
            TestPresentationPort {
                state: tx.state.clone(),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductListingDetailsReader for TestPresentationPort {
        async fn find_details(
            &mut self,
            request: &ProductListingDetailsReadRequest,
        ) -> Result<
            Option<PersonalizedProductListingDetailsReadModel>,
            ProductListingDetailsReadError,
        > {
            assert!(!self.state.committed());
            let mut presentation = self.state.presentation.lock().expect("presentation state");
            presentation.details_requests.push(request.clone());
            let ProductListingLookup::ById(id) = request.lookup else {
                panic!("watchlist update must look up listing by ID");
            };
            presentation
                .details_result
                .take()
                .unwrap_or_else(|| Ok(Some(details(id, request.language))))
        }
    }

    #[async_trait::async_trait]
    impl FxRateSnapshotRepository for TestPresentationPort {
        async fn find_latest(
            &mut self,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            panic!("pricing must use a bounded valuation time");
        }

        async fn find_latest_at_or_before(
            &mut self,
            timestamp: OffsetDateTime,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            assert!(!self.state.committed());
            let mut presentation = self.state.presentation.lock().expect("presentation state");
            presentation.current_snapshot_requests.push(timestamp);
            presentation
                .snapshot_result
                .take()
                .unwrap_or_else(|| Ok(Some(snapshot(FxRateId::new()))))
        }

        async fn find_by_id(
            &mut self,
            id: FxRateId,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            assert!(!self.state.committed());
            let mut presentation = self.state.presentation.lock().expect("presentation state");
            presentation.sale_snapshot_requests.push(id);
            presentation
                .snapshot_result
                .take()
                .unwrap_or_else(|| Ok(Some(snapshot(id))))
        }

        async fn find_by_ids(
            &mut self,
            _ids: &[FxRateId],
        ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            panic!("single listing does not need batch FX lookup");
        }

        async fn insert(
            &mut self,
            _snapshot: &NewFxRateSnapshot,
            _source_event_id: &str,
        ) -> Result<FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError> {
            panic!("watchlist update must not write FX snapshots");
        }
    }

    fn snapshot(id: FxRateId) -> FxRateSnapshot {
        NewFxRateSnapshot::capture_eur(
            id,
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| {
                FxRateQuote::new(
                    currency,
                    if currency == Currency::Eur {
                        FX_RATE_SCALE
                    } else {
                        1_250_000
                    },
                )
            }),
        )
        .expect("valid test FX snapshot")
        .into_persisted(FxRateGeneration::try_from(1).expect("valid test generation"))
    }

    fn details(
        id: ProductListingId,
        language: Language,
    ) -> PersonalizedProductListingDetailsReadModel {
        let url = Url::parse("https://example.test/product").expect("valid test URL");
        Personalized {
            item: ProductListingDetailsReadModel {
                product_listing_id: id,
                product_listing_title_slug_id: ProductListingSlugId::raw("product-a1b2c3")
                    .expect("valid test slug"),
                event_id: EventId::new(),
                source: ListingSourceSummary {
                    listing_source_id: ListingSourceId::new(),
                    name: ListingSourceName::try_from("Source").expect("valid source name"),
                    slug_id: ListingSourceSlugId::raw("source").expect("valid source slug"),
                },
                source_listing_id: SourceListingId::try_from("product")
                    .expect("valid source listing ID"),
                product_title: Some(Localized::new(language, Title::from("Product"))),
                product_description: Some(Localized::new(
                    language,
                    Description::from("Description"),
                )),
                title: Some(Localized::new(language, Title::from("Listing"))),
                description: Some(Localized::new(language, Description::from("Description"))),
                pricing: ProductListingPricing {
                    price: Some(Price::new(100_u64.into(), Currency::Eur).into()),
                    price_estimate_min: Some(Price::new(80_u64.into(), Currency::Eur)),
                    price_estimate_max: Some(Price::new(120_u64.into(), Currency::Eur)),
                },
                sale_observation: None,
                availability: Some(ListingAvailability::Available),
                lifecycle: ListingLifecycle::Active,
                url: url.clone(),
                view_url: url.clone(),
                images: [ProductListingImage::new(url)].into_iter().collect(),
                content_policy: Some(ContentPolicyDecision::Allowed),
                auction: None,
                lot: None,
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            },
            user_state: Some(ProductListingUserState::default()),
        }
    }

    fn command(
        user_id: UserId,
        product_listing_id: ProductListingId,
    ) -> UpdateWatchlistProductListingCommand {
        UpdateWatchlistProductListingCommand {
            user_id,
            product_listing_id,
            notifications: None,
            state: None,
            language: Language::En,
            currency: Currency::Eur,
        }
    }

    fn context_for_user(user_id: UserId) -> OperationContext {
        OperationContext {
            principal: Principal::User(user_id),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn delegated_context(
        user_id: UserId,
        capabilities: BTreeSet<CredentialCapability>,
    ) -> OperationContext {
        OperationContext {
            principal: Principal::DelegatedUser {
                user_id,
                capabilities,
            },
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn entry(
        user_id: UserId,
        product_listing_id: ProductListingId,
        notifications: bool,
    ) -> WatchlistProductListing {
        WatchlistProductListing::create(NewWatchlistProductListing {
            user_id,
            product_listing_id,
            notifications,
            state: WatchlistState::Active,
        })
    }

    fn handler(
        state: SharedState,
        tier: UserTier,
        lifecycle_outcome: TestLifecycleGuardOutcome,
    ) -> UpdateWatchlistProductListingHandler<
        TestUnitOfWork,
        TestWatchlistFactory,
        TestWatchlistFactory,
        TestAccountFactory,
        TestLifecycleGuardFactory,
        TestPresentationFactory,
        TestPresentationFactory,
    > {
        UpdateWatchlistProductListingHandler::new(
            TestUnitOfWork {
                state: state.clone(),
                ..Default::default()
            },
            TestWatchlistFactory {
                state: state.clone(),
            },
            TestWatchlistFactory {
                state: state.clone(),
            },
            TestAccountFactory {
                tier,
                state: state.clone(),
            },
            TestLifecycleGuardFactory {
                state,
                outcome: lifecycle_outcome,
            },
            TestPresentationFactory,
            TestPresentationFactory,
            ShippingEnrichment::for_tests(),
        )
    }

    #[tokio::test]
    async fn should_update_notifications_when_entry_exists() -> Result<(), String> {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        let state = SharedState::with_entry(entry(user_id, product_listing_id, true));

        let result = handler(
            state.clone(),
            UserTier::Free,
            TestLifecycleGuardOutcome::Active,
        )
        .execute(
            &context_for_user(user_id),
            UpdateWatchlistProductListingCommand {
                user_id,
                product_listing_id,
                notifications: Some(false),
                state: None,
                language: Language::En,
                currency: Currency::Eur,
            },
        )
        .await
        .map_err(|error| error.to_string())?;

        assert!(
            !result
                .product
                .user_state
                .as_ref()
                .expect("user state")
                .watchlist
                .expect("watchlist state")
                .notifications
        );
        assert_eq!(product_listing_id, result.product.item.product_listing_id);
        assert_eq!(1, state.updated());
        assert!(state.committed());
        Ok(())
    }

    #[tokio::test]
    async fn should_skip_persistence_when_update_command_changes_nothing() -> Result<(), String> {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        let original = entry(user_id, product_listing_id, true);
        let state = SharedState::with_entry(original.clone());

        let result = handler(
            state.clone(),
            UserTier::Free,
            TestLifecycleGuardOutcome::Active,
        )
        .execute(
            &context_for_user(user_id),
            UpdateWatchlistProductListingCommand {
                user_id,
                product_listing_id,
                notifications: Some(true),
                state: Some(WatchlistState::Active),
                language: Language::En,
                currency: Currency::Eur,
            },
        )
        .await
        .map_err(|error| error.to_string())?;

        let user_state = result.product.user_state.as_ref().expect("user state");
        assert!(user_state.watchlist.expect("watchlist state").notifications);
        assert!(user_state.watchlist.expect("watchlist state").watching);
        assert_eq!(product_listing_id, result.product.item.product_listing_id);
        assert_eq!(0, state.updated());
        assert!(state.committed());
        let persisted = state.entries.lock().expect("persisted entries");
        assert_eq!(original, persisted[0].value);
        assert_eq!(WatchlistStorageVersion::INITIAL, persisted[0].version);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_concurrency_conflict_when_entry_changes_before_update() {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        let mut state = SharedState::with_entry(entry(user_id, product_listing_id, true));
        state.force_concurrency_conflict = true;

        let result = handler(
            state.clone(),
            UserTier::Free,
            TestLifecycleGuardOutcome::Active,
        )
        .execute(
            &context_for_user(user_id),
            UpdateWatchlistProductListingCommand {
                user_id,
                product_listing_id,
                notifications: Some(false),
                state: None,
                language: Language::En,
                currency: Currency::Eur,
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(UpdateWatchlistProductListingError::ConcurrencyConflict)
        ));
        assert_eq!(0, state.updated());
        assert!(!state.committed());
    }

    #[tokio::test]
    async fn should_return_not_found_when_update_entry_missing() {
        let user_id = UserId::new();
        let state = SharedState::default();

        let result = handler(state, UserTier::Free, TestLifecycleGuardOutcome::Active)
            .execute(
                &context_for_user(user_id),
                UpdateWatchlistProductListingCommand {
                    user_id,
                    product_listing_id: ProductListingId::new(),
                    notifications: Some(false),
                    state: None,
                    language: Language::En,
                    currency: Currency::Eur,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(UpdateWatchlistProductListingError::NotFound)
        ));
    }

    #[tokio::test]
    async fn should_not_read_details_when_only_another_users_watchlist_entry_exists() {
        let user_id = UserId::new();
        let id = ProductListingId::new();
        let original = entry(UserId::new(), id, true);
        let state = SharedState::with_entry(original.clone());

        let result = handler(
            state.clone(),
            UserTier::Free,
            TestLifecycleGuardOutcome::Active,
        )
        .execute(
            &context_for_user(user_id),
            UpdateWatchlistProductListingCommand {
                notifications: Some(false),
                ..command(user_id, id)
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(UpdateWatchlistProductListingError::NotFound)
        ));
        assert_eq!(0, state.updated());
        assert!(!state.committed());
        assert_eq!(
            original,
            state.entries.lock().expect("persisted entries")[0].value
        );
        assert!(
            state
                .presentation
                .lock()
                .expect("presentation")
                .details_requests
                .is_empty()
        );
    }

    #[tokio::test]
    async fn should_reload_owned_watchlist_state_after_response_preparation() {
        for deleted in [false, true] {
            let user_id = UserId::new();
            let id = ProductListingId::new();
            let mut state = SharedState::with_entry(entry(user_id, id, true));
            state.entries_before_mutation = Some(if deleted {
                vec![]
            } else {
                vec![VersionedWatchlistProductListing::new(
                    inactive_entry(user_id, id),
                    WatchlistStorageVersion::INITIAL.next(),
                )]
            });
            let result = handler(
                state.clone(),
                UserTier::Free,
                TestLifecycleGuardOutcome::Active,
            )
            .execute(
                &context_for_user(user_id),
                UpdateWatchlistProductListingCommand {
                    notifications: Some(false),
                    ..command(user_id, id)
                },
            )
            .await;

            if deleted {
                assert!(matches!(
                    result,
                    Err(UpdateWatchlistProductListingError::NotFound)
                ));
                assert_eq!(0, state.updated());
                assert!(!state.committed());
            } else {
                let watchlist = result
                    .expect("updated response")
                    .product
                    .user_state
                    .expect("user state")
                    .watchlist
                    .expect("watchlist state");
                assert!(!watchlist.watching);
                assert!(!watchlist.notifications);
                assert_eq!(1, state.updated());
                assert!(state.committed());
                assert_eq!(
                    WatchlistStorageVersion::INITIAL.next().next(),
                    state.entries.lock().expect("persisted entries")[0].version
                );
            }
            assert_eq!(2, *state.begin_count.lock().expect("begin count"));
            assert_eq!(
                0,
                *state.open_transactions.lock().expect("open transactions")
            );
        }
    }

    fn inactive_entry(
        user_id: UserId,
        product_listing_id: ProductListingId,
    ) -> WatchlistProductListing {
        let mut entry = entry(user_id, product_listing_id, true);
        entry.change_state(WatchlistState::InactiveByUser);
        entry
    }

    #[tokio::test]
    async fn should_reactivate_when_listing_is_active() -> Result<(), String> {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        let state = SharedState::with_entry(inactive_entry(user_id, product_listing_id));

        let result = handler(
            state.clone(),
            UserTier::Free,
            TestLifecycleGuardOutcome::Active,
        )
        .execute(
            &context_for_user(user_id),
            UpdateWatchlistProductListingCommand {
                user_id,
                product_listing_id,
                notifications: None,
                state: Some(WatchlistState::Active),
                language: Language::En,
                currency: Currency::Eur,
            },
        )
        .await
        .map_err(|error| error.to_string())?;

        assert!(
            result
                .product
                .user_state
                .as_ref()
                .expect("user state")
                .watchlist
                .expect("watchlist state")
                .watching
        );
        assert_eq!(1, state.lifecycle_guard_calls());
        assert_eq!(1, state.updated());
        assert!(state.committed());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_reactivation_when_listing_is_missing_or_withdrawn() {
        for (outcome, expected_missing) in [
            (TestLifecycleGuardOutcome::Missing, true),
            (TestLifecycleGuardOutcome::Withdrawn, false),
        ] {
            let user_id = UserId::new();
            let product_listing_id = ProductListingId::new();
            let state = SharedState::with_entry(inactive_entry(user_id, product_listing_id));

            let result = handler(state.clone(), UserTier::Free, outcome)
                .execute(
                    &context_for_user(user_id),
                    UpdateWatchlistProductListingCommand {
                        user_id,
                        product_listing_id,
                        notifications: None,
                        state: Some(WatchlistState::Active),
                        language: Language::En,
                        currency: Currency::Eur,
                    },
                )
                .await;

            if expected_missing {
                assert!(matches!(
                    result,
                    Err(UpdateWatchlistProductListingError::ProductListingNotFound)
                ));
            } else {
                assert!(matches!(
                    result,
                    Err(UpdateWatchlistProductListingError::ProductListingUnavailable)
                ));
            }
            assert_eq!(1, state.lifecycle_guard_calls());
            assert_eq!(0, state.updated());
            assert!(!state.committed());
        }
    }

    #[tokio::test]
    async fn should_preserve_lifecycle_guard_lock_failure_and_reject_invalid_persisted_state()
    -> Result<(), String> {
        for outcome in [
            TestLifecycleGuardOutcome::LockFailed,
            TestLifecycleGuardOutcome::InvalidPersistedState,
        ] {
            let user_id = UserId::new();
            let product_listing_id = ProductListingId::new();
            let state = SharedState::with_entry(inactive_entry(user_id, product_listing_id));

            let result = handler(state.clone(), UserTier::Free, outcome)
                .execute(
                    &context_for_user(user_id),
                    UpdateWatchlistProductListingCommand {
                        user_id,
                        product_listing_id,
                        notifications: None,
                        state: Some(WatchlistState::Active),
                        language: Language::En,
                        currency: Currency::Eur,
                    },
                )
                .await;

            match (outcome, result) {
                (
                    TestLifecycleGuardOutcome::LockFailed,
                    Err(UpdateWatchlistProductListingError::TemporarilyUnavailable { source }),
                ) => assert_eq!(
                    "product listing lifecycle guard test lock failure",
                    source.to_string()
                ),
                (
                    TestLifecycleGuardOutcome::InvalidPersistedState,
                    Err(UpdateWatchlistProductListingError::InvalidPersistedState),
                ) => {}
                (_, error) => {
                    return Err(format!("unexpected lifecycle guard result: {error:?}"));
                }
            }
            assert_eq!(1, state.lifecycle_guard_calls());
            assert!(!state.committed());
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_bypass_lifecycle_guard_for_retained_withdrawn_operations() -> Result<(), String>
    {
        for command in [
            UpdateWatchlistProductListingCommand {
                user_id: UserId::new(),
                product_listing_id: ProductListingId::new(),
                notifications: Some(false),
                state: None,
                language: Language::En,
                currency: Currency::Eur,
            },
            UpdateWatchlistProductListingCommand {
                user_id: UserId::new(),
                product_listing_id: ProductListingId::new(),
                notifications: None,
                state: Some(WatchlistState::InactiveByUser),
                language: Language::En,
                currency: Currency::Eur,
            },
        ] {
            let state =
                SharedState::with_entry(entry(command.user_id, command.product_listing_id, true));
            handler(
                state.clone(),
                UserTier::Free,
                TestLifecycleGuardOutcome::Withdrawn,
            )
            .execute(&context_for_user(command.user_id), command)
            .await
            .map_err(|error| error.to_string())?;

            assert_eq!(0, state.lifecycle_guard_calls());
            assert_eq!(1, state.updated());
            assert!(state.committed());
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_bypass_lifecycle_guard_for_active_idempotent_update() -> Result<(), String> {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        let state = SharedState::with_entry(entry(user_id, product_listing_id, true));

        handler(
            state.clone(),
            UserTier::Free,
            TestLifecycleGuardOutcome::Withdrawn,
        )
        .execute(
            &context_for_user(user_id),
            UpdateWatchlistProductListingCommand {
                user_id,
                product_listing_id,
                notifications: None,
                state: Some(WatchlistState::Active),
                language: Language::En,
                currency: Currency::Eur,
            },
        )
        .await
        .map_err(|error| error.to_string())?;

        assert_eq!(0, state.lifecycle_guard_calls());
        assert_eq!(0, state.updated());
        assert!(state.committed());
        Ok(())
    }

    #[tokio::test]
    async fn should_present_no_op_with_requested_language_currency_and_full_user_state() {
        let user_id = UserId::new();
        let id = ProductListingId::new();
        let state = SharedState::with_entry(entry(user_id, id, true));
        let mut factual = details(id, Language::De);
        let user_state = factual.user_state.as_mut().expect("user state");
        user_state
            .content_visibility
            .show_unassessed_or_sensitive_content = true;
        user_state.notification.unseen_notification_ids = vec![Default::default()];
        user_state.search_filter.matched = true;
        user_state.search_filter.match_feedback = Some(false);
        let expected_user_state = ProductListingUserStateView {
            shipping: user_state.shipping.clone(),
            watchlist: Some(WatchlistUserState {
                watching: true,
                notifications: true,
            }),
            content_visibility: Some(user_state.content_visibility),
            notification: Some(user_state.notification.clone()),
            search_filter: Some(user_state.search_filter.clone()),
        };
        let expected_item = factual.item.clone();
        state
            .presentation
            .lock()
            .expect("presentation")
            .details_result = Some(Ok(Some(factual)));
        let before = OffsetDateTime::now_utc();

        let result = handler(
            state.clone(),
            UserTier::Free,
            TestLifecycleGuardOutcome::Active,
        )
        .execute(
            &context_for_user(user_id),
            UpdateWatchlistProductListingCommand {
                language: Language::De,
                currency: Currency::Usd,
                ..command(user_id, id)
            },
        )
        .await
        .expect("present no-op");

        let product = result.product;
        assert_eq!(Some(expected_user_state), product.user_state);
        assert_eq!(expected_item.title, product.item.title);
        assert_eq!(expected_item.description, product.item.description);
        assert_eq!(expected_item.source, product.item.source);
        assert_eq!(expected_item.event_id, product.item.event_id);
        assert_eq!(expected_item.pricing, product.item.pricing.source);
        assert_eq!(
            Some(ProductListingPrice::Monetary(Price::new(
                125_u64.into(),
                Currency::Usd
            ))),
            product.item.pricing.display.price
        );
        assert_eq!(
            Some(Price::new(100_u64.into(), Currency::Usd)),
            product.item.pricing.display.price_estimate_min
        );
        assert_eq!(
            Some(Price::new(150_u64.into(), Currency::Usd)),
            product.item.pricing.display.price_estimate_max
        );
        assert!(product.item.images[0].url.is_some());
        assert_eq!(0, state.updated());
        assert!(state.committed());
        let presentation = state.presentation.lock().expect("presentation");
        assert_eq!(
            vec![ProductListingDetailsReadRequest {
                lookup: ProductListingLookup::ById(id),
                language: Language::De,
                user_id: Some(user_id),
            }],
            presentation.details_requests
        );
        assert_eq!(1, presentation.current_snapshot_requests.len());
        assert!(presentation.current_snapshot_requests[0] >= before);
        assert!(presentation.current_snapshot_requests[0] <= OffsetDateTime::now_utc());
        assert!(presentation.sale_snapshot_requests.is_empty());
    }

    #[tokio::test]
    async fn should_preserve_notifications_when_deactivating_and_when_already_inactive() {
        for changed in [true, false] {
            let user_id = UserId::new();
            let id = ProductListingId::new();
            let original = if changed {
                entry(user_id, id, true)
            } else {
                inactive_entry(user_id, id)
            };
            let state = SharedState::with_entry(original);
            let result = handler(
                state.clone(),
                UserTier::Free,
                TestLifecycleGuardOutcome::Active,
            )
            .execute(
                &context_for_user(user_id),
                UpdateWatchlistProductListingCommand {
                    state: Some(WatchlistState::InactiveByUser),
                    ..command(user_id, id)
                },
            )
            .await
            .expect("deactivated response");
            let watchlist = result
                .product
                .user_state
                .expect("user state")
                .watchlist
                .expect("watchlist state");
            assert!(!watchlist.watching);
            assert!(watchlist.notifications);
            assert_eq!(usize::from(changed), state.updated());
            assert!(state.committed());
            assert_eq!(0, state.lifecycle_guard_calls());
        }
    }

    #[tokio::test]
    async fn should_use_repository_returned_entry_instead_of_command_or_reader_watchlist_state() {
        let user_id = UserId::new();
        let id = ProductListingId::new();
        let mut state = SharedState::with_entry(entry(user_id, id, true));
        state.persisted_entry = Some(inactive_entry(user_id, id));
        let result = handler(
            state.clone(),
            UserTier::Free,
            TestLifecycleGuardOutcome::Active,
        )
        .execute(
            &context_for_user(user_id),
            UpdateWatchlistProductListingCommand {
                notifications: Some(false),
                ..command(user_id, id)
            },
        )
        .await
        .expect("persisted response");
        let watchlist = result
            .product
            .user_state
            .expect("user state")
            .watchlist
            .expect("watchlist state");
        assert!(!watchlist.watching);
        assert!(watchlist.notifications);
        assert_eq!(1, state.updated());
        assert!(state.committed());
    }

    #[tokio::test]
    async fn should_redact_hidden_active_and_withdrawn_details_without_losing_watchlist_state() {
        for lifecycle in [ListingLifecycle::Active, ListingLifecycle::Withdrawn] {
            let user_id = UserId::new();
            let id = ProductListingId::new();
            let state = SharedState::with_entry(entry(user_id, id, true));
            let mut factual = details(id, Language::De);
            factual.item.lifecycle = lifecycle;
            if lifecycle == ListingLifecycle::Withdrawn {
                factual.item.availability = None;
            }
            factual
                .user_state
                .as_mut()
                .expect("user state")
                .search_filter
                .hidden = true;
            state
                .presentation
                .lock()
                .expect("presentation")
                .details_result = Some(Ok(Some(factual)));

            let product = handler(
                state.clone(),
                UserTier::Free,
                TestLifecycleGuardOutcome::Active,
            )
            .execute(
                &context_for_user(user_id),
                UpdateWatchlistProductListingCommand {
                    notifications: Some(false),
                    language: Language::De,
                    ..command(user_id, id)
                },
            )
            .await
            .expect("hidden response")
            .product;

            assert_eq!(id, product.item.product_listing_id);
            assert_eq!(lifecycle, product.item.lifecycle);
            assert!(product.item.product_listing_title_slug_id.is_none());
            assert_eq!("Hidden", product.item.source.name.as_ref());
            assert!(product.item.product_title.is_none());
            assert!(product.item.product_description.is_none());
            let title = product.item.title.expect("hidden title");
            assert_eq!(Language::De, title.localization);
            assert_ne!(Title::from("Listing"), title.payload);
            assert!(product.item.description.is_none());
            assert_eq!(
                ProductListingPricing::default(),
                product.item.pricing.source
            );
            assert!(product.item.pricing.display.price.is_none());
            assert!(product.item.pricing.display.price_estimate_min.is_none());
            assert!(product.item.pricing.display.price_estimate_max.is_none());
            assert!(product.item.availability.is_none());
            assert_eq!(
                "https://aura-historia.com/pricing",
                product.item.url.as_str()
            );
            assert_eq!(product.item.url, product.item.view_url);
            assert!(product.item.images.is_empty());
            assert!(product.item.auction.is_none());
            assert!(product.item.lot.is_none());
            assert!(product.item.shipping.is_none());
            let user_state = product.user_state.expect("user state");
            assert!(
                user_state
                    .search_filter
                    .as_ref()
                    .expect("search filter state")
                    .hidden
            );
            assert!(user_state.watchlist.expect("watchlist state").watching);
            assert!(!user_state.watchlist.expect("watchlist state").notifications);
            assert!(user_state.shipping.is_none());
            assert!(state.committed());
        }
    }

    #[tokio::test]
    async fn should_present_retained_withdrawn_details_with_content_redaction_and_sale_valuation() {
        for notifications in [None, Some(false)] {
            let user_id = UserId::new();
            let id = ProductListingId::new();
            let state = SharedState::with_entry(entry(user_id, id, true));
            let sale_snapshot = snapshot(FxRateId::new());
            let mut factual = details(id, Language::En);
            factual.item.lifecycle = ListingLifecycle::Withdrawn;
            factual.item.availability = None;
            factual.item.sale_observation = Some(ListingSaleObservation::new(
                OffsetDateTime::UNIX_EPOCH,
                sale_snapshot.id(),
            ));
            factual.item.content_policy = Some(ContentPolicyDecision::RequiresConsent(
                SensitiveContentCategory::NaziGermany,
            ));
            {
                let mut presentation = state.presentation.lock().expect("presentation");
                presentation.details_result = Some(Ok(Some(factual)));
                presentation.snapshot_result = Some(Ok(Some(sale_snapshot.clone())));
            }
            let product = handler(
                state.clone(),
                UserTier::Free,
                TestLifecycleGuardOutcome::Withdrawn,
            )
            .execute(
                &context_for_user(user_id),
                UpdateWatchlistProductListingCommand {
                    notifications,
                    currency: Currency::Usd,
                    ..command(user_id, id)
                },
            )
            .await
            .expect("retained withdrawn response")
            .product;
            assert_eq!(ListingLifecycle::Withdrawn, product.item.lifecycle);
            assert!(product.item.availability.is_none());
            assert_eq!(1, product.item.images.len());
            assert!(product.item.images[0].url.is_none());
            assert!(product.item.shipping.is_none());
            assert_eq!(
                ProductListingPricingValuation::SaleObservation {
                    fx_rate_id: sale_snapshot.id(),
                    captured_at: sale_snapshot.captured_at(),
                    observed_at: OffsetDateTime::UNIX_EPOCH,
                },
                product.item.pricing.valuation
            );
            assert_eq!(
                Some(ProductListingPrice::Monetary(Price::new(
                    125_u64.into(),
                    Currency::Usd
                ))),
                product.item.pricing.display.price
            );
            let watchlist = product
                .user_state
                .expect("user state")
                .watchlist
                .expect("watchlist state");
            assert!(watchlist.watching);
            assert_eq!(notifications.unwrap_or(true), watchlist.notifications);
            assert_eq!(0, state.lifecycle_guard_calls());
            assert!(state.committed());
            let presentation = state.presentation.lock().expect("presentation");
            assert!(presentation.current_snapshot_requests.is_empty());
            assert_eq!(
                vec![sale_snapshot.id()],
                presentation.sale_snapshot_requests
            );
        }
    }

    #[tokio::test]
    async fn should_use_sale_snapshot_only_for_sold_or_withdrawn_listings() {
        for (availability, lifecycle, historical) in [
            (
                Some(ListingAvailability::Available),
                ListingLifecycle::Active,
                false,
            ),
            (
                Some(ListingAvailability::SoldOut),
                ListingLifecycle::Active,
                true,
            ),
            (None, ListingLifecycle::Withdrawn, true),
        ] {
            let user_id = UserId::new();
            let id = ProductListingId::new();
            let state = SharedState::with_entry(entry(user_id, id, true));
            let sale_id = FxRateId::new();
            let mut factual = details(id, Language::En);
            factual.item.availability = availability;
            factual.item.lifecycle = lifecycle;
            factual.item.sale_observation = Some(ListingSaleObservation::new(
                OffsetDateTime::UNIX_EPOCH,
                sale_id,
            ));
            state
                .presentation
                .lock()
                .expect("presentation")
                .details_result = Some(Ok(Some(factual)));
            let product = handler(
                state.clone(),
                UserTier::Free,
                TestLifecycleGuardOutcome::Active,
            )
            .execute(&context_for_user(user_id), command(user_id, id))
            .await
            .expect("pricing response")
            .product;
            assert_eq!(
                historical,
                matches!(
                    product.item.pricing.valuation,
                    ProductListingPricingValuation::SaleObservation { .. }
                )
            );
            let presentation = state.presentation.lock().expect("presentation");
            assert_eq!(
                usize::from(!historical),
                presentation.current_snapshot_requests.len()
            );
            assert_eq!(
                if historical { vec![sale_id] } else { vec![] },
                presentation.sale_snapshot_requests
            );
        }
    }

    #[tokio::test]
    async fn should_not_mutate_or_commit_changed_or_no_op_commands_when_presentation_fails() {
        #[derive(Clone, Copy, Debug)]
        enum Failure {
            MissingDetails,
            Query,
            InvalidDetails,
            MissingUserState,
            MissingFx,
            FxRead,
            InvalidFx,
            FxMismatch,
            Conversion,
        }

        for changed in [false, true] {
            for failure in [
                Failure::MissingDetails,
                Failure::Query,
                Failure::InvalidDetails,
                Failure::MissingUserState,
                Failure::MissingFx,
                Failure::FxRead,
                Failure::InvalidFx,
                Failure::FxMismatch,
                Failure::Conversion,
            ] {
                let user_id = UserId::new();
                let id = ProductListingId::new();
                let original = entry(user_id, id, true);
                let state = SharedState::with_entry(original.clone());
                {
                    let mut presentation = state.presentation.lock().expect("presentation");
                    let mut factual = details(id, Language::En);
                    match failure {
                        Failure::MissingDetails => presentation.details_result = Some(Ok(None)),
                        Failure::Query => {
                            presentation.details_result = Some(Err(
                                ProductListingDetailsReadError::ProductListingDetailsQueryFailed,
                            ))
                        }
                        Failure::InvalidDetails => presentation.details_result = Some(Err(
                            ProductListingDetailsReadError::ProductListingDetailsReadModelInvalid,
                        )),
                        Failure::MissingUserState => {
                            factual.user_state = None;
                            presentation.details_result = Some(Ok(Some(factual)));
                        }
                        Failure::MissingFx => presentation.snapshot_result = Some(Ok(None)),
                        Failure::FxRead => {
                            presentation.snapshot_result =
                                Some(Err(FxRateSnapshotRepositoryError::ReadFailed {
                                    source: static_error("FX read failed"),
                                }))
                        }
                        Failure::InvalidFx => {
                            presentation.snapshot_result = Some(Err(
                                FxRateSnapshotRepositoryError::InvalidPersistedSnapshot {
                                    source: static_error("FX persisted state invalid"),
                                },
                            ))
                        }
                        Failure::FxMismatch => {
                            factual.item.availability = Some(ListingAvailability::SoldOut);
                            factual.item.sale_observation = Some(ListingSaleObservation::new(
                                OffsetDateTime::UNIX_EPOCH,
                                FxRateId::new(),
                            ));
                            presentation.details_result = Some(Ok(Some(factual)));
                            presentation.snapshot_result =
                                Some(Ok(Some(snapshot(FxRateId::new()))));
                        }
                        Failure::Conversion => {
                            factual.item.pricing.price =
                                Some(Price::new(u64::MAX.into(), Currency::Eur).into());
                            presentation.details_result = Some(Ok(Some(factual)));
                        }
                    }
                }
                let result = handler(
                    state.clone(),
                    UserTier::Free,
                    TestLifecycleGuardOutcome::Active,
                )
                .execute(
                    &context_for_user(user_id),
                    UpdateWatchlistProductListingCommand {
                        notifications: changed.then_some(false),
                        currency: Currency::Usd,
                        ..command(user_id, id)
                    },
                )
                .await;
                match (failure, result) {
                    (
                        Failure::MissingDetails,
                        Err(UpdateWatchlistProductListingError::Presentation(
                            GetProductListingError::NotFound,
                        )),
                    )
                    | (
                        Failure::Query,
                        Err(UpdateWatchlistProductListingError::Presentation(
                            GetProductListingError::ProductListingDetailsQueryFailed,
                        )),
                    )
                    | (
                        Failure::InvalidDetails | Failure::MissingUserState,
                        Err(UpdateWatchlistProductListingError::Presentation(
                            GetProductListingError::ProductListingDetailsReadModelInvalid,
                        )),
                    )
                    | (
                        Failure::MissingFx,
                        Err(UpdateWatchlistProductListingError::Presentation(
                            GetProductListingError::PricingFxSnapshotMissing,
                        )),
                    )
                    | (
                        Failure::FxMismatch,
                        Err(UpdateWatchlistProductListingError::Presentation(
                            GetProductListingError::SaleObservationFxSnapshotMismatch { .. },
                        )),
                    )
                    | (
                        Failure::Conversion,
                        Err(UpdateWatchlistProductListingError::Presentation(
                            GetProductListingError::ProductListingPriceConversionFailed { .. },
                        )),
                    ) => {}
                    (
                        Failure::FxRead,
                        Err(UpdateWatchlistProductListingError::Presentation(
                            GetProductListingError::PricingFxSnapshotUnavailable { source },
                        )),
                    ) => assert_eq!("FX read failed", source.to_string()),
                    (
                        Failure::InvalidFx,
                        Err(UpdateWatchlistProductListingError::Presentation(
                            GetProductListingError::PricingFxSnapshotInvalid { source },
                        )),
                    ) => assert_eq!("FX persisted state invalid", source.to_string()),
                    (failure, result) => panic!("unexpected result for {failure:?}: {result:?}"),
                }
                assert!(!state.committed());
                assert_eq!(0, state.updated());
                let persisted = state.entries.lock().expect("persisted entries");
                assert_eq!(original, persisted[0].value);
                assert_eq!(WatchlistStorageVersion::INITIAL, persisted[0].version);
            }
        }
    }

    #[tokio::test]
    async fn should_roll_back_on_commit_failure() {
        let user_id = UserId::new();
        let id = ProductListingId::new();
        let original = entry(user_id, id, true);
        let state = SharedState::with_entry(original.clone());
        let mut handler = handler(
            state.clone(),
            UserTier::Free,
            TestLifecycleGuardOutcome::Active,
        );
        handler.unit_of_work.fail_commit = true;
        let result = handler
            .execute(
                &context_for_user(user_id),
                UpdateWatchlistProductListingCommand {
                    notifications: Some(false),
                    ..command(user_id, id)
                },
            )
            .await;
        let error = result.expect_err("commit must fail");
        let UpdateWatchlistProductListingError::CommitTransactionFailed(source) = error else {
            panic!("unexpected commit failure: {error:?}");
        };
        assert!(matches!(
            source.downcast_ref::<TransactionError>(),
            Some(TransactionError::CommitFailed(_))
        ));
        assert_eq!(
            "test transaction failure",
            std::error::Error::source(source.as_ref())
                .expect("commit technical cause")
                .to_string()
        );
        assert!(!state.committed());
        assert_eq!(1, state.updated());
        assert_eq!(
            1,
            state
                .presentation
                .lock()
                .expect("presentation")
                .details_requests
                .len()
        );
        assert_eq!(
            original,
            state.entries.lock().expect("persisted entries")[0].value
        );
    }

    #[test]
    fn should_preserve_shipping_failure_source_chain() {
        let shipping_error = product_listing_service::shipping::ShippingReadError::FxRead(
            fxrate_service::ports::FxRateSnapshotReadError::ReadFailed {
                source: static_error("shipping FX read failed"),
            },
        );
        let error =
            UpdateWatchlistProductListingError::from(GetProductListingError::from(shipping_error));
        let mut source = std::error::Error::source(&error).expect("presentation source");
        assert!(source.is::<GetProductListingError>());
        source = source.source().expect("shipping source");
        assert!(source.is::<product_listing_service::shipping::ShippingReadError>());
        source = source.source().expect("FX read source");
        assert!(source.is::<fxrate_service::ports::FxRateSnapshotReadError>());
        assert_eq!(
            "shipping FX read failed",
            source.source().expect("technical cause").to_string()
        );
    }

    #[tokio::test]
    async fn should_reject_reactivation_at_quota_without_persisting() {
        let user_id = UserId::new();
        let id = ProductListingId::new();
        let state = SharedState::with_entry(inactive_entry(user_id, id));
        for _ in 0..20 {
            state.push(entry(user_id, ProductListingId::new(), true));
        }
        let result = handler(
            state.clone(),
            UserTier::Free,
            TestLifecycleGuardOutcome::Active,
        )
        .execute(
            &context_for_user(user_id),
            UpdateWatchlistProductListingCommand {
                state: Some(WatchlistState::Active),
                ..command(user_id, id)
            },
        )
        .await;
        assert!(matches!(
            result,
            Err(UpdateWatchlistProductListingError::WatchlistQuotaExceeded {
                active_count: 20,
                quota: 20
            })
        ));
        assert_eq!(0, state.updated());
        assert!(!state.committed());
        assert_eq!(1, state.lifecycle_guard_calls());
        let persisted = state.entries.lock().expect("persisted entries");
        assert_eq!(WatchlistState::InactiveByUser, persisted[0].value.state());
        assert_eq!(WatchlistStorageVersion::INITIAL, persisted[0].version);
    }

    #[tokio::test]
    async fn should_disclose_updated_watchlist_only_with_watchlist_read() {
        let user_id = UserId::new();
        let id = ProductListingId::new();
        let state = SharedState::with_entry(entry(user_id, id, true));
        let mut factual = details(id, Language::En);
        let user_state = factual.user_state.as_mut().expect("user state");
        user_state.notification.unseen_notification_ids = vec![Default::default()];
        user_state.search_filter.matched = true;
        state
            .presentation
            .lock()
            .expect("presentation")
            .details_result = Some(Ok(Some(factual)));

        let result = handler(
            state.clone(),
            UserTier::Free,
            TestLifecycleGuardOutcome::Active,
        )
        .execute(
            &delegated_context(
                user_id,
                BTreeSet::from([
                    CredentialCapability::WatchlistWrite,
                    CredentialCapability::WatchlistRead,
                ]),
            ),
            UpdateWatchlistProductListingCommand {
                notifications: Some(false),
                ..command(user_id, id)
            },
        )
        .await
        .expect("read-write delegated actor");

        assert_eq!(
            Some(ProductListingUserStateView {
                watchlist: Some(WatchlistUserState {
                    watching: true,
                    notifications: false,
                }),
                ..Default::default()
            }),
            result.product.user_state
        );
        assert!(state.committed());
    }

    #[tokio::test]
    async fn should_require_only_watchlist_write_and_reject_unauthorized_actors_before_begin() {
        let user_id = UserId::new();
        let id = ProductListingId::new();
        let state = SharedState::with_entry(entry(user_id, id, true));
        let result = handler(
            state.clone(),
            UserTier::Free,
            TestLifecycleGuardOutcome::Active,
        )
        .execute(
            &delegated_context(
                user_id,
                BTreeSet::from([CredentialCapability::WatchlistWrite]),
            ),
            UpdateWatchlistProductListingCommand {
                notifications: Some(false),
                ..command(user_id, id)
            },
        )
        .await
        .expect("write-only delegated actor");
        // A write scope authorizes the update, not reading the private state in its response.
        assert_eq!(None, result.product.user_state);
        assert!(state.committed());

        for principal in [
            Principal::Anonymous,
            Principal::User(UserId::new()),
            delegated_context(
                user_id,
                BTreeSet::from([CredentialCapability::WatchlistRead]),
            )
            .principal,
        ] {
            let state = SharedState::with_entry(entry(user_id, id, true));
            let context = OperationContext {
                principal,
                ..context_for_user(user_id)
            };
            let result = handler(
                state.clone(),
                UserTier::Free,
                TestLifecycleGuardOutcome::Active,
            )
            .execute(&context, command(user_id, id))
            .await;
            assert!(matches!(
                result,
                Err(
                    UpdateWatchlistProductListingError::AuthenticatedActorRequired
                        | UpdateWatchlistProductListingError::Forbidden
                )
            ));
            assert_eq!(0, *state.begin_count.lock().expect("begin count"));
            assert!(
                state
                    .presentation
                    .lock()
                    .expect("presentation")
                    .details_requests
                    .is_empty()
            );
        }
    }

    #[tokio::test]
    async fn should_forbid_delegated_user_without_watchlist_write() {
        let user_id = UserId::new();
        let state = SharedState::default();

        let result = handler(state, UserTier::Free, TestLifecycleGuardOutcome::Active)
            .execute(
                &delegated_context(user_id, BTreeSet::new()),
                UpdateWatchlistProductListingCommand {
                    user_id,
                    product_listing_id: ProductListingId::new(),
                    notifications: Some(false),
                    state: None,
                    language: Language::En,
                    currency: Currency::Eur,
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(UpdateWatchlistProductListingError::Forbidden)
        ));
    }
}
