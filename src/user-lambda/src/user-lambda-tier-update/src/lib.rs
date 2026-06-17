use aws_lambda_events::sqs::SqsEvent;
use common::{
    actor::record::ActorRecord, dynamodb_stream::extract_from_dynamodb_stream,
    resource_state::record::ResourceStateRecord,
};
use lambda_runtime::LambdaEvent;
use product_watchlist::{
    core::quota::WatchlistQuota,
    dynamodb::{
        record::WatchlistProductRecord, record_update::WatchlistProductRecordUpdate,
        repository::WatchlistProductDynamoDbRepository,
    },
};
use search_filter::{
    core::quota::SearchFilterQuota,
    dynamodb::{
        repository::UserSearchFilterDynamoDbRepository,
        user_search_filter_record::UserSearchFilterRecord,
        user_search_filter_record_update::UserSearchFilterRecordUpdate,
    },
};
use time::OffsetDateTime;
use tracing::{info, warn};
use user::dynamodb::user_record::UserRecord;

const ASCENDING_KEYS: bool = true;

#[tracing::instrument(skip(event, watchlist_repository, search_filter_repository), fields(requestId = %event.context.request_id))]
pub async fn handler(
    watchlist_repository: &(impl WatchlistProductDynamoDbRepository + Sync),
    search_filter_repository: &(impl UserSearchFilterDynamoDbRepository + Sync),
    event: LambdaEvent<SqsEvent>,
) -> Result<(), lambda_runtime::Error> {
    let messages = event.payload.records;
    let (users, failed_message_ids) = extract_from_dynamodb_stream::<UserRecord>(messages);

    if !failed_message_ids.is_empty() {
        return Err("Failed extracting user records from event.".into());
    }

    for user in users.into_values() {
        enforce_tier(watchlist_repository, search_filter_repository, user).await?;
    }

    Ok(())
}

pub async fn enforce_tier(
    watchlist_repository: &(impl WatchlistProductDynamoDbRepository + Sync),
    search_filter_repository: &(impl UserSearchFilterDynamoDbRepository + Sync),
    user: UserRecord,
) -> Result<(), lambda_runtime::Error> {
    let user_id = user.user_id;
    let tier: user::core::tier::UserTier = user.tier.into();

    let mut watchlist_records = watchlist_repository
        .query_watchlist_records_all(&user_id, ASCENDING_KEYS)
        .await?;
    watchlist_records.sort_by_key(|record| std::cmp::Reverse(record.created));
    let watchlist_quota = tier.watchlist_quota() as usize;
    let mut active_watchlist_count = 0usize;
    for record in watchlist_records.drain(..) {
        if matches!(record.state, ResourceStateRecord::InactiveByUser) {
            continue;
        }

        active_watchlist_count += 1;
        let target_state = if active_watchlist_count <= watchlist_quota {
            ResourceStateRecord::Active
        } else {
            ResourceStateRecord::InactiveByRestrictedPlan
        };

        update_watchlist_state(watchlist_repository, record, target_state).await?;
    }

    let mut search_filter_records = search_filter_repository
        .query_user_search_filter_records(&user_id, ASCENDING_KEYS)
        .await?;
    search_filter_records.sort_by_key(|record| std::cmp::Reverse(record.created));
    let search_filter_quota = tier.search_filter_quota() as usize;
    let mut active_search_filter_count = 0usize;
    for record in search_filter_records.drain(..) {
        if matches!(record.state, ResourceStateRecord::InactiveByUser) {
            continue;
        }

        let record_state = record.state;
        let record_user_id = record.user_id;
        let record_search_filter_id = record.user_search_filter_id;
        let feature_allowed = search_filter_features_allowed(&tier, record);
        if feature_allowed {
            active_search_filter_count += 1;
        }
        let target_state = if feature_allowed && active_search_filter_count <= search_filter_quota {
            ResourceStateRecord::Active
        } else {
            ResourceStateRecord::InactiveByRestrictedPlan
        };

        update_search_filter_state(
            search_filter_repository,
            record_user_id,
            record_search_filter_id,
            record_state,
            target_state,
        )
        .await?;
    }

    info!(userId = %user_id, tier = ?tier, "Enforced tier quotas.");
    Ok(())
}

fn search_filter_features_allowed(
    tier: &user::core::tier::UserTier,
    record: UserSearchFilterRecord,
) -> bool {
    let filter = search_filter::core::user_search_filter::UserSearchFilter::from(record);
    tier.check_search_filter_features(&filter.search).is_ok()
}

async fn update_watchlist_state(
    repository: &(impl WatchlistProductDynamoDbRepository + Sync),
    record: WatchlistProductRecord,
    target_state: ResourceStateRecord,
) -> Result<(), lambda_runtime::Error> {
    if record.state == target_state {
        return Ok(());
    }

    repository
        .update_watchlist_record(
            &record.user_id,
            &record.shop_id,
            &record.shops_product_id,
            WatchlistProductRecordUpdate {
                notifications: None,
                state: Some(target_state),
                updated_by: ActorRecord::System,
                updated: OffsetDateTime::now_utc(),
            },
        )
        .await
        .map(|_| ())
        .map_err(|err| {
            warn!(error = ?err, userId = %record.user_id, "Failed updating watchlist product state.");
            lambda_runtime::Error::from(err)
        })
}

async fn update_search_filter_state(
    repository: &(impl UserSearchFilterDynamoDbRepository + Sync),
    user_id: common::user_id::UserId,
    search_filter_id: common::user_search_filter_id::UserSearchFilterId,
    current_state: ResourceStateRecord,
    target_state: ResourceStateRecord,
) -> Result<(), lambda_runtime::Error> {
    if current_state == target_state {
        return Ok(());
    }

    repository
        .update_user_search_filter_record(
            &user_id,
            &search_filter_id,
            search_filter_state_update(target_state),
        )
        .await
        .map(|_| ())
        .map_err(|err| {
            warn!(error = ?err, userId = %user_id, "Failed updating search filter state.");
            lambda_runtime::Error::from(err)
        })
}

fn search_filter_state_update(state: ResourceStateRecord) -> UserSearchFilterRecordUpdate {
    UserSearchFilterRecordUpdate {
        name: None,
        notifications: None,
        state: Some(state),
        enhanced_search_description: None,
        product_query: None,
        shop_name_query: None,
        exclude_shop_name_query: None,
        seller_name_query: None,
        exclude_seller_name_query: None,
        shop_slug_id_query: None,
        exclude_shop_slug_id_query: None,
        seller_slug_id_query: None,
        exclude_seller_slug_id_query: None,
        shop_type_query: None,
        country_query: None,
        continent_query: None,
        geo_address_distance_query: None,
        price_query: None,
        state_query: None,
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        language: None,
        currency: None,
        updated_by: ActorRecord::System,
        updated: OffsetDateTime::now_utc(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_dynamodb::error::SdkError;
    use common::{
        actor::domain::Actor, currency::domain::Currency, language::domain::Language,
        resource_state::record::ResourceStateRecord,
    };
    use fake::{Fake, Faker};
    use product_watchlist::dynamodb::repository::MockWatchlistProductDynamoDbRepository;
    use search_filter::dynamodb::repository::MockUserSearchFilterDynamoDbRepository;
    use user::{core::tier::UserTier, dynamodb::tier_record::UserTierRecord};

    fn user_record(tier: UserTier) -> UserRecord {
        let mut user = Faker.fake::<UserRecord>();
        user.tier = UserTierRecord::from(tier);
        user
    }

    fn watchlist_record(user: &UserRecord, state: ResourceStateRecord) -> WatchlistProductRecord {
        let mut record = Faker.fake::<WatchlistProductRecord>();
        record.user_id = user.user_id;
        record.pk = product_watchlist::dynamodb::record::mk_pk(&user.user_id);
        record.state = state;
        record
    }

    fn search_filter_record(
        user: &UserRecord,
        state: ResourceStateRecord,
    ) -> UserSearchFilterRecord {
        let filter = search_filter::core::user_search_filter::UserSearchFilter {
            user_id: user.user_id,
            user_search_filter_id: Faker.fake(),
            name: Faker.fake(),
            notifications: true,
            state: state.into(),
            search: product::core::product_search::ProductSearch::new(Language::En, Currency::Eur),
            created_by: Actor::User(user.user_id),
            updated_by: Actor::User(user.user_id),
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        };
        filter.into()
    }

    #[tokio::test]
    async fn should_deactivate_oldest_watchlist_products_when_active_quota_is_reduced() {
        let user = user_record(UserTier::Free);
        let records = (0..=UserTier::Free.watchlist_quota())
            .map(|_| watchlist_record(&user, ResourceStateRecord::Active))
            .collect::<Vec<_>>();
        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::new();
        watchlist_repository
            .expect_query_watchlist_records_all()
            .withf({
                let user_id = user.user_id;
                move |actual_user_id, ascending_keys| *actual_user_id == user_id && *ascending_keys
            })
            .return_once(move |_, _| Box::pin(async move { Ok(records) }));
        watchlist_repository
            .expect_update_watchlist_record()
            .times(1)
            .withf(|_, _, _, update| {
                update.state == Some(ResourceStateRecord::InactiveByRestrictedPlan)
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(None) }));

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::new();
        search_filter_repository
            .expect_query_user_search_filter_records()
            .return_once(|_, _| Box::pin(async { Ok(vec![]) }));

        enforce_tier(&watchlist_repository, &search_filter_repository, user)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn should_deactivate_oldest_search_filters_when_active_quota_is_reduced() {
        let user = user_record(UserTier::Free);
        let records = vec![
            search_filter_record(&user, ResourceStateRecord::Active),
            search_filter_record(&user, ResourceStateRecord::Active),
        ];
        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::new();
        watchlist_repository
            .expect_query_watchlist_records_all()
            .return_once(|_, _| Box::pin(async { Ok(vec![]) }));

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::new();
        search_filter_repository
            .expect_query_user_search_filter_records()
            .withf({
                let user_id = user.user_id;
                move |actual_user_id, ascending_keys| *actual_user_id == user_id && *ascending_keys
            })
            .return_once(move |_, _| Box::pin(async move { Ok(records) }));
        search_filter_repository
            .expect_update_user_search_filter_record()
            .times(1)
            .withf(|_, _, update| {
                update.state == Some(ResourceStateRecord::InactiveByRestrictedPlan)
            })
            .returning(|_, _, _| Box::pin(async { Ok(None) }));

        enforce_tier(&watchlist_repository, &search_filter_repository, user)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn should_reactivate_plan_restricted_resources_when_tier_allows_them() {
        let user = user_record(UserTier::Ultimate);
        let watchlist_records = vec![
            watchlist_record(&user, ResourceStateRecord::InactiveByRestrictedPlan),
            watchlist_record(&user, ResourceStateRecord::InactiveByUser),
        ];
        let search_filter_records = vec![
            search_filter_record(&user, ResourceStateRecord::InactiveByRestrictedPlan),
            search_filter_record(&user, ResourceStateRecord::InactiveByUser),
        ];
        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::new();
        watchlist_repository
            .expect_query_watchlist_records_all()
            .return_once(move |_, _| Box::pin(async move { Ok(watchlist_records) }));
        watchlist_repository
            .expect_update_watchlist_record()
            .times(1)
            .withf(|_, _, _, update| update.state == Some(ResourceStateRecord::Active))
            .returning(|_, _, _, _| Box::pin(async { Ok(None) }));

        let mut search_filter_repository = MockUserSearchFilterDynamoDbRepository::new();
        search_filter_repository
            .expect_query_user_search_filter_records()
            .return_once(move |_, _| Box::pin(async move { Ok(search_filter_records) }));
        search_filter_repository
            .expect_update_user_search_filter_record()
            .times(1)
            .withf(|_, _, update| update.state == Some(ResourceStateRecord::Active))
            .returning(|_, _, _| Box::pin(async { Ok(None) }));

        enforce_tier(&watchlist_repository, &search_filter_repository, user)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn should_return_error_when_watchlist_query_fails() {
        let user = user_record(UserTier::Free);
        let mut watchlist_repository = MockWatchlistProductDynamoDbRepository::new();
        watchlist_repository
            .expect_query_watchlist_records_all()
            .return_once(|_, _| {
                Box::pin(async {
                    Err(SdkError::construction_failure(
                        "failed querying watchlist records",
                    ))
                })
            });
        let search_filter_repository = MockUserSearchFilterDynamoDbRepository::new();

        let actual = enforce_tier(&watchlist_repository, &search_filter_repository, user).await;

        assert!(actual.is_err());
    }
}
