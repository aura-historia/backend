use crate::ports::{
    SearchFilterMatchNotificationSource, SearchFilterMatchNotificationSourceReadError,
    SearchFilterMatchNotificationSourceReader, SearchFilterMatchNotificationSourceReaderFactory,
    SearchFilterMonthlyMatchQuotaReadError, SearchFilterMonthlyMatchQuotaReader,
    SearchFilterMonthlyMatchQuotaReaderFactory,
};
use crate::tier_policy::monthly_match_quota;
use application::error::{BoxError, box_error};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use notification_core::notification::{NotificationPayload, NotificationSearchFilterPayload};
use notification_service::use_cases::commands::create_notification::{
    CreateNotificationCommand, CreateNotificationResult, CreateNotificationUseCase,
};
use product_core::product_id::ProductId;
use product_service::ports::{
    ProductSearchFilterMatchSource, ProductSearchFilterMatchSourceReadError,
    ProductSearchFilterMatchSourceReader, ProductSearchFilterMatchSourceReaderFactory,
};
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use user_core::user_id::UserId;
use user_service::ports::{
    UserTierEntitlements, UserTierEntitlementsError, UserTierEntitlementsFactory,
};

#[derive(Debug, Clone, PartialEq)]
pub struct GenerateSearchFilterMatchNotificationCommand {
    pub user_id: UserId,
    pub search_filter_id: UserSearchFilterId,
    pub product_id: ProductId,
    pub origin_event_id: EventId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerateSearchFilterMatchNotificationResult {
    Created,
    AlreadyExists,
    SuppressedByQuota,
    SuppressedForMissingUser,
    SuppressedForMissingMatch,
    SuppressedForStaleMatch,
    SuppressedForNonSelectedFilter,
    SuppressedForMissingProduct,
    SuppressedForStaleProductEvent,
}

#[derive(Debug, thiserror::Error)]
pub enum GenerateSearchFilterMatchNotificationError {
    #[error("failed to begin search filter notification selection transaction")]
    BeginTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter match notification source read failed")]
    MatchSourceReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter match notification source persisted state is invalid")]
    MatchSourceStateInvalid {
        #[source]
        source: BoxError,
    },

    #[error("product notification source read failed")]
    ProductSourceReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("product notification source persisted state is invalid")]
    ProductSourceStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("product notification source does not match the requested event or product")]
    ProductSourceMismatch,
    #[error("user tier entitlement lock failed")]
    UserTierEntitlementsLockFailed {
        #[source]
        source: BoxError,
    },
    #[error("monthly search filter notification quota read failed")]
    MonthlyMatchQuotaReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to commit search filter notification selection transaction")]
    CommitTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter match notification creation failed")]
    NotificationCreateFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait GenerateSearchFilterMatchNotificationUseCase: Send + Sync {
    async fn execute(
        &self,
        command: GenerateSearchFilterMatchNotificationCommand,
    ) -> Result<
        GenerateSearchFilterMatchNotificationResult,
        GenerateSearchFilterMatchNotificationError,
    >;
}

pub struct GenerateSearchFilterMatchNotificationHandler<U, M, P, Q, A, N> {
    unit_of_work: U,
    matches: M,
    products: P,
    quotas: Q,
    tier_entitlements: A,
    notifications: N,
}

impl<U, M, P, Q, A, N> GenerateSearchFilterMatchNotificationHandler<U, M, P, Q, A, N> {
    pub fn new(
        unit_of_work: U,
        matches: M,
        products: P,
        quotas: Q,
        tier_entitlements: A,
        notifications: N,
    ) -> Self {
        Self {
            unit_of_work,
            matches,
            products,
            quotas,
            tier_entitlements,
            notifications,
        }
    }
}

#[async_trait::async_trait]
impl<U, M, P, Q, A, N> GenerateSearchFilterMatchNotificationUseCase
    for GenerateSearchFilterMatchNotificationHandler<U, M, P, Q, A, N>
where
    U: UnitOfWork,
    M: SearchFilterMatchNotificationSourceReaderFactory<U::Tx>,
    P: ProductSearchFilterMatchSourceReaderFactory<U::Tx>,
    Q: SearchFilterMonthlyMatchQuotaReaderFactory<U::Tx>,
    A: UserTierEntitlementsFactory<U::Tx>,
    N: CreateNotificationUseCase,
{
    #[tracing::instrument(
        name = "generate_search_filter_match_notification",
        skip_all,
        fields(
            origin_event_id = %command.origin_event_id,
            product_id = %command.product_id,
            user_id = %command.user_id,
            search_filter_id = %command.search_filter_id,
        )
    )]
    async fn execute(
        &self,
        command: GenerateSearchFilterMatchNotificationCommand,
    ) -> Result<
        GenerateSearchFilterMatchNotificationResult,
        GenerateSearchFilterMatchNotificationError,
    > {
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            GenerateSearchFilterMatchNotificationError::BeginTransactionFailed {
                source: box_error(source),
            }
        })?;

        let match_source = self
            .matches
            .in_transaction(&mut tx)
            .find_source(
                command.user_id,
                command.search_filter_id,
                command.product_id,
                command.origin_event_id,
            )
            .await
            .map_err(match_source_read_error)?;
        let Some(match_source) = match_source else {
            tx.commit().await.map_err(commit_error)?;
            return Ok(GenerateSearchFilterMatchNotificationResult::SuppressedForMissingMatch);
        };
        if !match_source_matches_command(&match_source, &command) {
            tx.commit().await.map_err(commit_error)?;
            return Ok(GenerateSearchFilterMatchNotificationResult::SuppressedForStaleMatch);
        }
        if !match_source.is_selected_filter {
            tx.commit().await.map_err(commit_error)?;
            return Ok(GenerateSearchFilterMatchNotificationResult::SuppressedForNonSelectedFilter);
        }

        let product = self
            .products
            .in_transaction(&mut tx)
            .find_source(command.origin_event_id, command.product_id)
            .await
            .map_err(product_source_read_error)?;
        let Some(product) = product else {
            tx.commit().await.map_err(commit_error)?;
            return Ok(GenerateSearchFilterMatchNotificationResult::SuppressedForMissingProduct);
        };
        if product.event_id != command.origin_event_id || product.product_id != command.product_id {
            return Err(GenerateSearchFilterMatchNotificationError::ProductSourceMismatch);
        }
        if product.current_event_id != command.origin_event_id {
            tx.commit().await.map_err(commit_error)?;
            return Ok(GenerateSearchFilterMatchNotificationResult::SuppressedForStaleProductEvent);
        }

        let tier = self
            .tier_entitlements
            .in_transaction(&mut tx)
            .lock_user_tier(match_source.user_id)
            .await
            .map_err(tier_entitlements_error)?;
        let Some(tier) = tier else {
            tx.commit().await.map_err(commit_error)?;
            return Ok(GenerateSearchFilterMatchNotificationResult::SuppressedForMissingUser);
        };
        let rank = self
            .quotas
            .in_transaction(&mut tx)
            .notification_selection_rank_for_user_in_month(
                match_source.user_id,
                match_source.matched_at,
                match_source.origin_event_id,
            )
            .await
            .map_err(monthly_match_quota_error)?;
        tx.commit().await.map_err(commit_error)?;

        if rank > monthly_match_quota(tier) {
            return Ok(GenerateSearchFilterMatchNotificationResult::SuppressedByQuota);
        }

        match create_notification(&self.notifications, match_source, product).await? {
            CreateNotificationResult::Created { .. } => {
                Ok(GenerateSearchFilterMatchNotificationResult::Created)
            }
            CreateNotificationResult::AlreadyExists => {
                Ok(GenerateSearchFilterMatchNotificationResult::AlreadyExists)
            }
        }
    }
}

fn match_source_matches_command(
    source: &SearchFilterMatchNotificationSource,
    command: &GenerateSearchFilterMatchNotificationCommand,
) -> bool {
    source.user_id == command.user_id
        && source.search_filter_id == command.search_filter_id
        && source.product_id == command.product_id
        && source.origin_event_id == command.origin_event_id
}

async fn create_notification<N>(
    notifications: &N,
    match_source: SearchFilterMatchNotificationSource,
    product: ProductSearchFilterMatchSource,
) -> Result<CreateNotificationResult, GenerateSearchFilterMatchNotificationError>
where
    N: CreateNotificationUseCase,
{
    notifications
        .execute(CreateNotificationCommand {
            user_id: match_source.user_id,
            origin_event_id: match_source.origin_event_id,
            notification_payload: NotificationPayload::SearchFilter {
                product_id: product.product_id,
                shop_id: product.shop_id,
                shops_product_id: product.shops_product_id,
                shop_slug_id: product.shop_slug_id,
                product_slug_id: product.product_slug_id,
                shop_name: product.shop_name,
                title: (!product.titles.is_empty()).then_some(product.titles),
                image: product.image,
                url: product.url,
                view_url: product.view_url,
                search_filter_payload: NotificationSearchFilterPayload {
                    user_search_filter_id: match_source.search_filter_id,
                    user_search_filter_name: match_source.search_filter_name,
                },
            },
            external: match_source.external,
        })
        .await
        .map_err(
            |source| GenerateSearchFilterMatchNotificationError::NotificationCreateFailed {
                source: box_error(source),
            },
        )
}

fn match_source_read_error(
    error: SearchFilterMatchNotificationSourceReadError,
) -> GenerateSearchFilterMatchNotificationError {
    match error {
        SearchFilterMatchNotificationSourceReadError::InvalidPersistedState { source } => {
            GenerateSearchFilterMatchNotificationError::MatchSourceStateInvalid { source }
        }
        error => GenerateSearchFilterMatchNotificationError::MatchSourceReadFailed {
            source: box_error(error),
        },
    }
}

fn product_source_read_error(
    error: ProductSearchFilterMatchSourceReadError,
) -> GenerateSearchFilterMatchNotificationError {
    match error {
        ProductSearchFilterMatchSourceReadError::InvalidPersistedState { source } => {
            GenerateSearchFilterMatchNotificationError::ProductSourceStateInvalid { source }
        }
        error => GenerateSearchFilterMatchNotificationError::ProductSourceReadFailed {
            source: box_error(error),
        },
    }
}

fn tier_entitlements_error(
    error: UserTierEntitlementsError,
) -> GenerateSearchFilterMatchNotificationError {
    match error {
        UserTierEntitlementsError::LockFailed { source }
        | UserTierEntitlementsError::ReconciliationFailed { source } => {
            GenerateSearchFilterMatchNotificationError::UserTierEntitlementsLockFailed { source }
        }
    }
}

fn monthly_match_quota_error(
    error: SearchFilterMonthlyMatchQuotaReadError,
) -> GenerateSearchFilterMatchNotificationError {
    GenerateSearchFilterMatchNotificationError::MonthlyMatchQuotaReadFailed {
        source: box_error(error),
    }
}

fn commit_error(
    source: application::transaction::TransactionError,
) -> GenerateSearchFilterMatchNotificationError {
    GenerateSearchFilterMatchNotificationError::CommitTransactionFailed {
        source: box_error(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::transaction::TransactionError;
    use indexmap::IndexSet;
    use product_core::shops_product_id::ShopsProductId;
    use product_core::{
        product::{ProductAddress, ProductAuction, ProductPricing},
        product_image::ProductImage,
        product_lifecycle::ProductLifecycle,
        product_slug_id::ProductSlugId,
        product_state::ProductState,
    };
    use product_service::ports::ProductSearchFilterMatchShopType;
    use product_service::ports::ProductSearchFilterMatchSourceEventKind;
    use search_filter_core::user_search_filter_name::UserSearchFilterName;
    use shop_core::seller_slug_id::SellerSlugId;
    use shop_core::shop_id::ShopId;
    use shop_core::shop_name::ShopName;
    use shop_core::shop_slug_id::ShopSlugId;
    use std::{
        error::Error,
        sync::{Arc, Mutex},
    };
    use time::OffsetDateTime;
    use url::Url;
    use user_core::tier::UserTier;

    #[derive(Default)]
    struct State {
        commits: usize,
        notification_commit_counts: Vec<usize>,
    }

    #[derive(Clone)]
    struct TestUnitOfWork(Arc<Mutex<State>>);

    struct TestTransaction(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl Transaction for TestTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = self.0.lock().map_err(|_| TransactionError::CommitFailed)?;
            state.commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for TestUnitOfWork {
        type Tx = TestTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            Ok(TestTransaction(Arc::clone(&self.0)))
        }
    }

    enum MatchSources {
        Found(Option<SearchFilterMatchNotificationSource>),
        QueryFailure,
    }
    struct MatchReader<'a>(&'a MatchSources);

    #[async_trait::async_trait]
    impl SearchFilterMatchNotificationSourceReader for MatchReader<'_> {
        async fn find_source(
            &mut self,
            _user_id: UserId,
            _search_filter_id: UserSearchFilterId,
            _product_id: ProductId,
            _origin_event_id: EventId,
        ) -> Result<
            Option<SearchFilterMatchNotificationSource>,
            SearchFilterMatchNotificationSourceReadError,
        > {
            match self.0 {
                MatchSources::Found(source) => Ok(source.clone()),
                MatchSources::QueryFailure => {
                    Err(SearchFilterMatchNotificationSourceReadError::ReadFailed {
                        source: box_error(std::io::Error::other("query failed")),
                    })
                }
            }
        }
    }

    impl SearchFilterMatchNotificationSourceReaderFactory<TestTransaction> for MatchSources {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl SearchFilterMatchNotificationSourceReader + 'tx {
            MatchReader(self)
        }
    }

    struct ProductSources(Option<ProductSearchFilterMatchSource>);
    struct ProductReader(Option<ProductSearchFilterMatchSource>);

    #[async_trait::async_trait]
    impl ProductSearchFilterMatchSourceReader for ProductReader {
        async fn find_source(
            &mut self,
            _event_id: EventId,
            _product_id: ProductId,
        ) -> Result<Option<ProductSearchFilterMatchSource>, ProductSearchFilterMatchSourceReadError>
        {
            Ok(self.0.clone())
        }
    }

    impl ProductSearchFilterMatchSourceReaderFactory<TestTransaction> for ProductSources {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl ProductSearchFilterMatchSourceReader + 'tx {
            ProductReader(self.0.clone())
        }
    }

    struct Quotas;
    struct QuotaReader;

    #[async_trait::async_trait]
    impl SearchFilterMonthlyMatchQuotaReader for QuotaReader {
        async fn notification_selection_rank_for_user_in_month(
            &mut self,
            _user_id: UserId,
            _matched_at: OffsetDateTime,
            _origin_event_id: EventId,
        ) -> Result<usize, SearchFilterMonthlyMatchQuotaReadError> {
            Ok(1)
        }
    }

    impl SearchFilterMonthlyMatchQuotaReaderFactory<TestTransaction> for Quotas {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl SearchFilterMonthlyMatchQuotaReader + 'tx {
            QuotaReader
        }
    }

    struct Tiers;
    struct TierReader;

    #[async_trait::async_trait]
    impl UserTierEntitlements for TierReader {
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

    impl UserTierEntitlementsFactory<TestTransaction> for Tiers {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl UserTierEntitlements + 'tx {
            TierReader
        }
    }

    struct Notifications(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl CreateNotificationUseCase for Notifications {
        async fn execute(
            &self,
            command: CreateNotificationCommand,
        ) -> Result<notification_service::use_cases::commands::create_notification::CreateNotificationResult, notification_service::use_cases::commands::create_notification::CreateNotificationError>{
            if let Ok(mut state) = self.0.lock() {
                let commits = state.commits;
                state.notification_commit_counts.push(commits);
            }
            Ok(notification_service::use_cases::commands::create_notification::CreateNotificationResult::Created {
                notification: notification_core::notification::Notification::new(
                    command.user_id,
                    command.origin_event_id,
                    command.notification_payload,
                    command.external,
                ),
            })
        }
    }

    struct DuplicateNotifications;

    #[async_trait::async_trait]
    impl CreateNotificationUseCase for DuplicateNotifications {
        async fn execute(
            &self,
            _command: CreateNotificationCommand,
        ) -> Result<
            notification_service::use_cases::commands::create_notification::CreateNotificationResult,
            notification_service::use_cases::commands::create_notification::CreateNotificationError,
        >{
            Ok(CreateNotificationResult::AlreadyExists)
        }
    }

    fn sources() -> Result<
        (
            GenerateSearchFilterMatchNotificationCommand,
            SearchFilterMatchNotificationSource,
            ProductSearchFilterMatchSource,
        ),
        url::ParseError,
    > {
        let user_id = UserId::new();
        let search_filter_id = UserSearchFilterId::new();
        let product_id = ProductId::new();
        let origin_event_id = EventId::new();
        let command = GenerateSearchFilterMatchNotificationCommand {
            user_id,
            search_filter_id,
            product_id,
            origin_event_id,
        };
        let match_source = SearchFilterMatchNotificationSource {
            user_id,
            search_filter_id,
            product_id,
            origin_event_id,
            search_filter_name: UserSearchFilterName::from("daily"),
            matched_at: OffsetDateTime::UNIX_EPOCH,
            external: true,
            is_selected_filter: true,
        };
        let url = Url::parse("https://example.test/product")?;
        let product = ProductSearchFilterMatchSource {
            event_id: origin_event_id,
            event_kind: ProductSearchFilterMatchSourceEventKind::Domain,
            origin_event_time: OffsetDateTime::UNIX_EPOCH,
            current_event_id: origin_event_id,
            projection_version: 1,
            product_id,
            product_slug_id: ProductSlugId::from("product"),
            shop_id: ShopId::new(),
            shop_slug_id: ShopSlugId::from("shop"),
            shop_name: ShopName::from("Shop"),
            shop_type: ProductSearchFilterMatchShopType::Marketplace,
            seller_id: ShopId::new(),
            seller_slug_id: SellerSlugId::from(ShopSlugId::from("seller")),
            seller_name: ShopName::from("Seller"),
            shops_product_id: ShopsProductId::from("sku-1"),
            address: ProductAddress::default(),
            product_title: None,
            product_description: None,
            titles: Default::default(),
            descriptions: Default::default(),
            pricing: ProductPricing::default(),
            sale_valuation: None,
            state: ProductState::Available,
            lifecycle: ProductLifecycle::Active,
            url: url.clone(),
            view_url: url,
            image: None,
            images: IndexSet::<ProductImage>::new(),
            embedding: None,
            auction: ProductAuction::default(),
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        };
        Ok((command, match_source, product))
    }

    #[tokio::test]
    async fn should_commit_selection_before_creating_notification() -> Result<(), Box<dyn Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let (command, match_source, product) = sources()?;
        let handler = GenerateSearchFilterMatchNotificationHandler::new(
            TestUnitOfWork(Arc::clone(&state)),
            MatchSources::Found(Some(match_source)),
            ProductSources(Some(product)),
            Quotas,
            Tiers,
            Notifications(Arc::clone(&state)),
        );

        assert_eq!(
            GenerateSearchFilterMatchNotificationResult::Created,
            handler.execute(command).await?
        );
        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(1, state.commits);
        assert_eq!(vec![1], state.notification_commit_counts);
        Ok(())
    }

    #[tokio::test]
    async fn should_report_exact_match_redelivery_as_deduplicated() -> Result<(), Box<dyn Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let (command, match_source, product) = sources()?;
        let handler = GenerateSearchFilterMatchNotificationHandler::new(
            TestUnitOfWork(Arc::clone(&state)),
            MatchSources::Found(Some(match_source)),
            ProductSources(Some(product)),
            Quotas,
            Tiers,
            DuplicateNotifications,
        );

        assert_eq!(
            GenerateSearchFilterMatchNotificationResult::AlreadyExists,
            handler.execute(command).await?
        );
        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(1, state.commits);
        Ok(())
    }

    #[tokio::test]
    async fn should_suppress_stale_match_source_without_creating_notification()
    -> Result<(), Box<dyn Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let (command, mut match_source, product) = sources()?;
        match_source.origin_event_id = EventId::new();
        let handler = GenerateSearchFilterMatchNotificationHandler::new(
            TestUnitOfWork(Arc::clone(&state)),
            MatchSources::Found(Some(match_source)),
            ProductSources(Some(product)),
            Quotas,
            Tiers,
            Notifications(Arc::clone(&state)),
        );

        assert_eq!(
            GenerateSearchFilterMatchNotificationResult::SuppressedForStaleMatch,
            handler.execute(command).await?
        );
        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(1, state.commits);
        assert!(state.notification_commit_counts.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_preserve_match_source_query_failure_in_service_error_chain()
    -> Result<(), Box<dyn Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let (command, _, product) = sources()?;
        let handler = GenerateSearchFilterMatchNotificationHandler::new(
            TestUnitOfWork(Arc::clone(&state)),
            MatchSources::QueryFailure,
            ProductSources(Some(product)),
            Quotas,
            Tiers,
            Notifications(state),
        );

        let error = handler
            .execute(command)
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("expected error"))?;
        assert!(matches!(
            error,
            GenerateSearchFilterMatchNotificationError::MatchSourceReadFailed { .. }
        ));
        assert!(Error::source(&error).is_some());
        Ok(())
    }
}
