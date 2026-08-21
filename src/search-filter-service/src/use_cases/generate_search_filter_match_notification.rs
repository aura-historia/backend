use crate::ports::{
    SearchFilterMatchNotificationSource, SearchFilterMatchNotificationSourceReadError,
    SearchFilterMatchNotificationSourceReader, SearchFilterMatchNotificationSourceReaderFactory,
    SearchFilterMonthlyMatchQuotaReadError, SearchFilterMonthlyMatchQuotaReader,
    SearchFilterMonthlyMatchQuotaReaderFactory,
};
use crate::tier_policy::monthly_match_quota;
use application::{
    error::{BoxError, box_error},
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use domain_primitives::event_id::EventId;
use notification_core::notification::{NotificationContent, ProductNotificationSnapshot};
use notification_service::ports::notification_creator::{
    ExternalDeliveryRequest, NewNotification, NotificationCreationError,
    NotificationCreationOutcome, NotificationCreator, NotificationCreatorFactory,
};
use product_core::product_id::ProductId;
use product_service::ports::{
    ProductCurrentRevisionCheck, ProductCurrentRevisionCheckError, ProductCurrentRevisionGuard,
    ProductCurrentRevisionGuardFactory, ProductSearchFilterMatchSource,
    ProductSearchFilterMatchSourceReadError, ProductSearchFilterMatchSourceReader,
    ProductSearchFilterMatchSourceReaderFactory,
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
    #[error("product current revision check failed")]
    ProductCurrentRevisionCheckFailed {
        #[source]
        source: BoxError,
    },
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

pub struct GenerateSearchFilterMatchNotificationHandler<U, M, P, Q, A, G, N> {
    unit_of_work: U,
    matches: M,
    products: P,
    quotas: Q,
    tier_entitlements: A,
    product_revision_guard: G,
    notifications: N,
}

impl<U, M, P, Q, A, G, N> GenerateSearchFilterMatchNotificationHandler<U, M, P, Q, A, G, N> {
    pub fn new(
        unit_of_work: U,
        matches: M,
        products: P,
        quotas: Q,
        tier_entitlements: A,
        product_revision_guard: G,
        notifications: N,
    ) -> Self {
        Self {
            unit_of_work,
            matches,
            products,
            quotas,
            tier_entitlements,
            product_revision_guard,
            notifications,
        }
    }
}

#[async_trait::async_trait]
impl<U, M, P, Q, A, G, N> GenerateSearchFilterMatchNotificationUseCase
    for GenerateSearchFilterMatchNotificationHandler<U, M, P, Q, A, G, N>
where
    U: UnitOfWork,
    M: SearchFilterMatchNotificationSourceReaderFactory<U::Tx>,
    P: ProductSearchFilterMatchSourceReaderFactory<U::Tx>,
    Q: SearchFilterMonthlyMatchQuotaReaderFactory<U::Tx>,
    A: UserTierEntitlementsFactory<U::Tx>,
    G: ProductCurrentRevisionGuardFactory<U::Tx>,
    N: NotificationCreatorFactory<U::Tx>,
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
        let revision = self
            .product_revision_guard
            .in_transaction(&mut tx)
            .lock_and_check(command.product_id, command.origin_event_id)
            .await
            .map_err(|source: ProductCurrentRevisionCheckError| {
                GenerateSearchFilterMatchNotificationError::ProductCurrentRevisionCheckFailed {
                    source: box_error(source),
                }
            })?;
        if revision == ProductCurrentRevisionCheck::Stale {
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

        if rank > monthly_match_quota(tier) {
            tx.commit().await.map_err(commit_error)?;
            return Ok(GenerateSearchFilterMatchNotificationResult::SuppressedByQuota);
        }

        let outcome = create_notification(
            &mut self.notifications.in_transaction(&mut tx),
            match_source,
            product,
        )
        .await?;
        tx.commit().await.map_err(commit_error)?;

        match outcome {
            NotificationCreationOutcome::Inserted { .. } => {
                Ok(GenerateSearchFilterMatchNotificationResult::Created)
            }
            NotificationCreationOutcome::Duplicate => {
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

async fn create_notification(
    notifications: &mut impl NotificationCreator,
    match_source: SearchFilterMatchNotificationSource,
    product: ProductSearchFilterMatchSource,
) -> Result<NotificationCreationOutcome, GenerateSearchFilterMatchNotificationError> {
    let notification = NewNotification {
        notification: notification_core::notification::Notification::new(
            Default::default(),
            match_source.user_id,
            NotificationContent::SearchFilter {
                origin_event_id: match_source.origin_event_id,
                product_id: product.product_id,
                user_search_filter_id: match_source.search_filter_id,
                snapshot: ProductNotificationSnapshot {
                    shop_id: product.shop_id,
                    shops_product_id: product.shops_product_id,
                    shop_slug_id: product.shop_slug_id,
                    product_slug_id: product.product_slug_id,
                    shop_name: product.shop_name,
                    title: (!product.titles.is_empty()).then_some(product.titles),
                    image: product.image,
                    url: product.url,
                    view_url: product.view_url,
                },
                user_search_filter_name: match_source.search_filter_name,
            },
        ),
        external_delivery: if match_source.external_delivery_requested {
            ExternalDeliveryRequest::Requested
        } else {
            ExternalDeliveryRequest::None
        },
    };
    let mut outcomes = notifications.create_many(&[notification]).await.map_err(
        |source: NotificationCreationError| {
            GenerateSearchFilterMatchNotificationError::NotificationCreateFailed {
                source: box_error(source),
            }
        },
    )?;
    outcomes.pop().ok_or_else(|| {
        GenerateSearchFilterMatchNotificationError::NotificationCreateFailed {
            source: box_error(std::io::Error::other(
                "notification creator returned no outcome",
            )),
        }
    })
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

fn commit_error(source: TransactionError) -> GenerateSearchFilterMatchNotificationError {
    GenerateSearchFilterMatchNotificationError::CommitTransactionFailed {
        source: box_error(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexSet;
    use product_core::{
        product::{ProductAddress, ProductAuction, ProductPricing},
        product_image::ProductImage,
        product_lifecycle::ProductLifecycle,
        product_slug_id::ProductSlugId,
        product_state::ProductState,
        shops_product_id::ShopsProductId,
    };
    use product_service::ports::ProductSearchFilterMatchShopType;
    use product_service::ports::ProductSearchFilterMatchSourceEventKind;
    use search_filter_core::user_search_filter_name::UserSearchFilterName;
    use shop_core::{
        seller_slug_id::SellerSlugId, shop_id::ShopId, shop_name::ShopName,
        shop_slug_id::ShopSlugId,
    };
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
        quota_reads: usize,
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

    #[derive(Clone, Copy)]
    enum ProductRevisionCheckOutcome {
        Current,
        Stale,
        Error,
    }

    struct ProductRevisionGuards(ProductRevisionCheckOutcome);
    struct ProductRevisionGuard(ProductRevisionCheckOutcome);

    #[async_trait::async_trait]
    impl ProductCurrentRevisionGuard for ProductRevisionGuard {
        async fn lock_and_check(
            &mut self,
            _product_id: ProductId,
            _expected_event_id: EventId,
        ) -> Result<ProductCurrentRevisionCheck, ProductCurrentRevisionCheckError> {
            match self.0 {
                ProductRevisionCheckOutcome::Current => Ok(ProductCurrentRevisionCheck::Current),
                ProductRevisionCheckOutcome::Stale => Ok(ProductCurrentRevisionCheck::Stale),
                ProductRevisionCheckOutcome::Error => {
                    Err(ProductCurrentRevisionCheckError::CheckFailed {
                        source: box_error(std::io::Error::other("guard read failed")),
                    })
                }
            }
        }
    }

    impl ProductCurrentRevisionGuardFactory<TestTransaction> for ProductRevisionGuards {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl ProductCurrentRevisionGuard + 'tx {
            ProductRevisionGuard(self.0)
        }
    }

    struct Quotas(Arc<Mutex<State>>);
    struct QuotaReader(Arc<Mutex<State>>);

    #[async_trait::async_trait]
    impl SearchFilterMonthlyMatchQuotaReader for QuotaReader {
        async fn notification_selection_rank_for_user_in_month(
            &mut self,
            _user_id: UserId,
            _matched_at: OffsetDateTime,
            _origin_event_id: EventId,
        ) -> Result<usize, SearchFilterMonthlyMatchQuotaReadError> {
            if let Ok(mut state) = self.0.lock() {
                state.quota_reads += 1;
            }
            Ok(1)
        }
    }

    impl SearchFilterMonthlyMatchQuotaReaderFactory<TestTransaction> for Quotas {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl SearchFilterMonthlyMatchQuotaReader + 'tx {
            QuotaReader(Arc::clone(&self.0))
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
    struct TestNotificationCreator(Arc<Mutex<State>>);

    impl NotificationCreatorFactory<TestTransaction> for Notifications {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl NotificationCreator + 'tx {
            TestNotificationCreator(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl NotificationCreator for TestNotificationCreator {
        async fn create_many(
            &mut self,
            notifications: &[NewNotification],
        ) -> Result<Vec<NotificationCreationOutcome>, NotificationCreationError> {
            if let Ok(mut state) = self.0.lock() {
                let commits = state.commits;
                state.notification_commit_counts.push(commits);
            }
            Ok(notifications
                .iter()
                .map(|notification| NotificationCreationOutcome::Inserted {
                    notification_id: notification.notification.notification_id(),
                })
                .collect())
        }
    }

    struct DuplicateNotifications;
    struct DuplicateNotificationCreator;

    impl NotificationCreatorFactory<TestTransaction> for DuplicateNotifications {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl NotificationCreator + 'tx {
            DuplicateNotificationCreator
        }
    }

    #[async_trait::async_trait]
    impl NotificationCreator for DuplicateNotificationCreator {
        async fn create_many(
            &mut self,
            notifications: &[NewNotification],
        ) -> Result<Vec<NotificationCreationOutcome>, NotificationCreationError> {
            Ok(notifications
                .iter()
                .map(|_| NotificationCreationOutcome::Duplicate)
                .collect())
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
            external_delivery_requested: true,
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
    async fn should_create_notification_before_committing_selection() -> Result<(), Box<dyn Error>>
    {
        let state = Arc::new(Mutex::new(State::default()));
        let (command, match_source, product) = sources()?;
        let handler = GenerateSearchFilterMatchNotificationHandler::new(
            TestUnitOfWork(Arc::clone(&state)),
            MatchSources::Found(Some(match_source)),
            ProductSources(Some(product)),
            Quotas(Arc::clone(&state)),
            Tiers,
            ProductRevisionGuards(ProductRevisionCheckOutcome::Current),
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
        assert_eq!(vec![0], state.notification_commit_counts);
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
            Quotas(Arc::clone(&state)),
            Tiers,
            ProductRevisionGuards(ProductRevisionCheckOutcome::Current),
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
            Quotas(Arc::clone(&state)),
            Tiers,
            ProductRevisionGuards(ProductRevisionCheckOutcome::Current),
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
    async fn should_suppress_stale_product_event_without_reading_quota_or_creating_notification()
    -> Result<(), Box<dyn Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let (command, match_source, product) = sources()?;
        let handler = GenerateSearchFilterMatchNotificationHandler::new(
            TestUnitOfWork(Arc::clone(&state)),
            MatchSources::Found(Some(match_source)),
            ProductSources(Some(product)),
            Quotas(Arc::clone(&state)),
            Tiers,
            ProductRevisionGuards(ProductRevisionCheckOutcome::Stale),
            Notifications(Arc::clone(&state)),
        );

        assert_eq!(
            GenerateSearchFilterMatchNotificationResult::SuppressedForStaleProductEvent,
            handler.execute(command).await?
        );
        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(1, state.commits);
        assert_eq!(0, state.quota_reads);
        assert!(state.notification_commit_counts.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_return_typed_revision_check_failure_without_reading_quota_or_creating_notification()
    -> Result<(), Box<dyn Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let (command, match_source, product) = sources()?;
        let handler = GenerateSearchFilterMatchNotificationHandler::new(
            TestUnitOfWork(Arc::clone(&state)),
            MatchSources::Found(Some(match_source)),
            ProductSources(Some(product)),
            Quotas(Arc::clone(&state)),
            Tiers,
            ProductRevisionGuards(ProductRevisionCheckOutcome::Error),
            Notifications(Arc::clone(&state)),
        );

        let error = handler
            .execute(command)
            .await
            .err()
            .ok_or_else(|| std::io::Error::other("expected error"))?;
        assert!(matches!(
            &error,
            GenerateSearchFilterMatchNotificationError::ProductCurrentRevisionCheckFailed { .. }
        ));
        assert!(matches!(
            Error::source(&error)
                .and_then(|source| { source.downcast_ref::<ProductCurrentRevisionCheckError>() }),
            Some(ProductCurrentRevisionCheckError::CheckFailed { .. })
        ));
        let state = state
            .lock()
            .map_err(|_| std::io::Error::other("test mutex poisoned"))?;
        assert_eq!(0, state.commits);
        assert_eq!(0, state.quota_reads);
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
            Quotas(Arc::clone(&state)),
            Tiers,
            ProductRevisionGuards(ProductRevisionCheckOutcome::Current),
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
