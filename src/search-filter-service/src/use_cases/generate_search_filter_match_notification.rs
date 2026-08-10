use crate::ports::{
    SearchFilterMatchNotificationSource, SearchFilterMonthlyMatchQuotaReadError,
    SearchFilterMonthlyMatchQuotaReader, SearchFilterMonthlyMatchQuotaReaderFactory,
};
use crate::tier_policy::monthly_match_quota;
use common::error::boxed::{BoxError, box_error};
use common::transaction::{Transaction, UnitOfWork};
use notification_core::notification::{NotificationPayload, NotificationSearchFilterPayload};
use notification_service::use_cases::commands::create_notification::{
    CreateNotificationCommand, CreateNotificationUseCase,
};
use product_service::ports::ProductSearchFilterMatchSource;
use user_service::ports::{
    UserTierEntitlements, UserTierEntitlementsError, UserTierEntitlementsFactory,
};

#[derive(Debug, Clone, PartialEq)]
pub struct GenerateSearchFilterMatchNotificationCommand {
    pub match_source: SearchFilterMatchNotificationSource,
    pub product: ProductSearchFilterMatchSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerateSearchFilterMatchNotificationResult {
    Created,
    SuppressedByQuota,
    SuppressedForMissingUser,
}

#[derive(Debug, thiserror::Error)]
pub enum GenerateSearchFilterMatchNotificationError {
    #[error("failed to begin search filter notification selection transaction")]
    BeginTransactionFailed {
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

pub struct GenerateSearchFilterMatchNotificationHandler<U, Q, A, N> {
    unit_of_work: U,
    quotas: Q,
    tier_entitlements: A,
    notifications: N,
}

impl<U, Q, A, N> GenerateSearchFilterMatchNotificationHandler<U, Q, A, N> {
    pub fn new(unit_of_work: U, quotas: Q, tier_entitlements: A, notifications: N) -> Self {
        Self {
            unit_of_work,
            quotas,
            tier_entitlements,
            notifications,
        }
    }
}

#[async_trait::async_trait]
impl<U, Q, A, N> GenerateSearchFilterMatchNotificationUseCase
    for GenerateSearchFilterMatchNotificationHandler<U, Q, A, N>
where
    U: UnitOfWork,
    Q: SearchFilterMonthlyMatchQuotaReaderFactory<U::Tx>,
    A: UserTierEntitlementsFactory<U::Tx>,
    N: CreateNotificationUseCase,
{
    #[tracing::instrument(
        name = "generate_search_filter_match_notification",
        skip_all,
        fields(
            origin_event_id = %command.match_source.origin_event_id,
            product_id = %command.product.product_id,
            user_id = %command.match_source.user_id,
            search_filter_id = %command.match_source.search_filter_id,
        )
    )]
    async fn execute(
        &self,
        command: GenerateSearchFilterMatchNotificationCommand,
    ) -> Result<
        GenerateSearchFilterMatchNotificationResult,
        GenerateSearchFilterMatchNotificationError,
    > {
        let match_source = command.match_source;
        let product = command.product;
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            GenerateSearchFilterMatchNotificationError::BeginTransactionFailed {
                source: box_error(source),
            }
        })?;
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

        self.notifications
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
            .map_err(|source| {
                GenerateSearchFilterMatchNotificationError::NotificationCreateFailed {
                    source: box_error(source),
                }
            })?;

        Ok(GenerateSearchFilterMatchNotificationResult::Created)
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
    source: common::transaction::TransactionError,
) -> GenerateSearchFilterMatchNotificationError {
    GenerateSearchFilterMatchNotificationError::CommitTransactionFailed {
        source: box_error(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{
        event_id::EventId, product_slug_id::ProductSlugId, shop_id::ShopId, shop_name::ShopName,
        shop_slug_id::ShopSlugId, shops_product_id::ShopsProductId, transaction::TransactionError,
        user_id::UserId, user_search_filter_id::UserSearchFilterId,
        user_search_filter_name::UserSearchFilterName,
    };
    use indexmap::IndexSet;

    use product_core::{
        product::{ProductAddress, ProductAuction, ProductPricing},
        product_image::ProductImage,
    };
    use product_service::ports::ProductSearchFilterMatchShopType;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use time::OffsetDateTime;
    use url::Url;
    use user_core::tier::UserTier;

    #[derive(Clone, Default)]
    struct TestUnitOfWork(Arc<Mutex<usize>>);

    struct TestTransaction(Arc<Mutex<usize>>);

    #[async_trait::async_trait]
    impl Transaction for TestTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut commits = self.0.lock().map_err(|_| TransactionError::CommitFailed)?;
            *commits += 1;
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

    struct Quotas {
        rank: usize,
    }

    struct Rank(usize);

    #[async_trait::async_trait]
    impl SearchFilterMonthlyMatchQuotaReader for Rank {
        async fn notification_selection_rank_for_user_in_month(
            &mut self,
            _user_id: UserId,
            _matched_at: OffsetDateTime,
            _origin_event_id: EventId,
        ) -> Result<usize, SearchFilterMonthlyMatchQuotaReadError> {
            Ok(self.0)
        }
    }

    impl SearchFilterMonthlyMatchQuotaReaderFactory<TestTransaction> for Quotas {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut TestTransaction,
        ) -> impl SearchFilterMonthlyMatchQuotaReader + 'tx {
            Rank(self.rank)
        }
    }

    struct Tiers;

    struct FreeTier;

    #[async_trait::async_trait]
    impl UserTierEntitlements for FreeTier {
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
            FreeTier
        }
    }

    struct Notifications(Arc<AtomicUsize>);

    #[async_trait::async_trait]
    impl CreateNotificationUseCase for Notifications {
        async fn execute(
            &self,
            command: CreateNotificationCommand,
        ) -> Result<
            notification_service::use_cases::commands::create_notification::CreateNotificationResult,
            notification_service::use_cases::commands::create_notification::CreateNotificationError,
        >{
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(
                notification_service::use_cases::commands::create_notification::CreateNotificationResult {
                    notification: notification_core::notification::Notification::new(
                        command.user_id,
                        command.origin_event_id,
                        command.notification_payload,
                        command.external,
                    ),
                },
            )
        }
    }

    fn command() -> Result<GenerateSearchFilterMatchNotificationCommand, url::ParseError> {
        let user_id = UserId::new();
        let event_id = EventId::new();
        let url = Url::parse("https://example.test/product")?;
        Ok(GenerateSearchFilterMatchNotificationCommand {
            match_source: SearchFilterMatchNotificationSource {
                user_id,
                search_filter_id: UserSearchFilterId::new(),
                search_filter_name: UserSearchFilterName::from("daily"),
                product_id: common::product_id::ProductId::new(),
                origin_event_id: event_id,
                matched_at: OffsetDateTime::UNIX_EPOCH,
                external: true,
            },
            product: ProductSearchFilterMatchSource {
                event_id,
                current_event_id: event_id,
                product_id: common::product_id::ProductId::new(),
                product_slug_id: ProductSlugId::from("product"),
                shop_id: ShopId::new(),
                shop_slug_id: ShopSlugId::from("shop"),
                shop_name: ShopName::from("Shop"),
                shop_type: ProductSearchFilterMatchShopType::Marketplace,
                seller_id: ShopId::new(),
                seller_slug_id: common::seller_slug_id::SellerSlugId::from("seller"),
                seller_name: ShopName::from("Seller"),
                shops_product_id: ShopsProductId::from("sku-1"),
                address: ProductAddress::default(),
                product_title: None,
                product_description: None,
                titles: std::collections::HashMap::new(),
                descriptions: std::collections::HashMap::new(),
                pricing: ProductPricing::default(),
                state: common::product_state::domain::ProductState::Available,
                lifecycle: common::product_lifecycle::domain::ProductLifecycle::Active,
                url: url.clone(),
                view_url: url,
                image: None,
                images: IndexSet::<ProductImage>::new(),
                auction: ProductAuction::default(),
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            },
        })
    }

    #[tokio::test]
    async fn should_suppress_notification_when_selected_event_exceeds_monthly_quota()
    -> Result<(), Box<dyn std::error::Error>> {
        let commits = Arc::new(Mutex::new(0));
        let notification_calls = Arc::new(AtomicUsize::new(0));
        let handler = GenerateSearchFilterMatchNotificationHandler::new(
            TestUnitOfWork(Arc::clone(&commits)),
            Quotas { rank: 11 },
            Tiers,
            Notifications(Arc::clone(&notification_calls)),
        );

        let result = handler.execute(command()?).await?;

        assert_eq!(
            GenerateSearchFilterMatchNotificationResult::SuppressedByQuota,
            result
        );
        assert_eq!(0, notification_calls.load(Ordering::Relaxed));
        assert_eq!(
            1,
            *commits
                .lock()
                .map_err(|_| std::io::Error::other("test mutex poisoned"))?
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_create_notification_when_selected_event_is_within_monthly_quota()
    -> Result<(), Box<dyn std::error::Error>> {
        let commits = Arc::new(Mutex::new(0));
        let notification_calls = Arc::new(AtomicUsize::new(0));
        let handler = GenerateSearchFilterMatchNotificationHandler::new(
            TestUnitOfWork(Arc::clone(&commits)),
            Quotas { rank: 1 },
            Tiers,
            Notifications(Arc::clone(&notification_calls)),
        );

        let result = handler.execute(command()?).await?;

        assert_eq!(GenerateSearchFilterMatchNotificationResult::Created, result);
        assert_eq!(1, notification_calls.load(Ordering::Relaxed));
        assert_eq!(
            1,
            *commits
                .lock()
                .map_err(|_| std::io::Error::other("test mutex poisoned"))?
        );
        Ok(())
    }
}
