use aws_lambda_events::sqs::SqsEvent;
use common::dynamodb_stream::extract_from_dynamodb_stream;
use lambda_runtime::LambdaEvent;
use product_watchlist::{
    core::quota::WatchlistQuota,
    dynamodb::{
        record::WatchlistProductRecord, record_update::WatchlistProductRecordUpdate,
        repository::WatchlistProductDynamoDbRepository,
        watchlist_product_state_record::WatchlistProductStateRecord,
    },
};
use search_filter::{
    core::quota::SearchFilterQuota,
    dynamodb::{
        repository::UserSearchFilterDynamoDbRepository,
        user_search_filter_record::UserSearchFilterRecord,
        user_search_filter_record_update::UserSearchFilterRecordUpdate,
        user_search_filter_state_record::UserSearchFilterStateRecord,
    },
};
use time::OffsetDateTime;
use tracing::{error, info};
use user::dynamodb::user_record::UserRecord;

const LATEST_FIRST: bool = false;

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
        .query_watchlist_records_all(&user_id, LATEST_FIRST)
        .await?;
    let watchlist_quota = tier.watchlist_quota() as usize;
    let mut active_watchlist_count = 0usize;
    for record in watchlist_records.drain(..) {
        if matches!(record.state, WatchlistProductStateRecord::InactiveByUser) {
            continue;
        }

        active_watchlist_count += 1;
        let target_state = if active_watchlist_count <= watchlist_quota {
            WatchlistProductStateRecord::Active
        } else {
            WatchlistProductStateRecord::InactiveByRestrictedPlan
        };

        update_watchlist_state(watchlist_repository, record, target_state).await?;
    }

    let mut search_filter_records = search_filter_repository
        .query_user_search_filter_records(&user_id, LATEST_FIRST)
        .await?;
    let search_filter_quota = tier.search_filter_quota() as usize;
    let mut active_search_filter_count = 0usize;
    for record in search_filter_records.drain(..) {
        if matches!(record.state, UserSearchFilterStateRecord::InactiveByUser) {
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
            UserSearchFilterStateRecord::Active
        } else {
            UserSearchFilterStateRecord::InactiveByRestrictedPlan
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
        && filter
            .enhanced_search_description
            .as_ref()
            .map(|desc| tier.check_enhanced_search_filter_description(desc).is_ok())
            .unwrap_or(true)
}

async fn update_watchlist_state(
    repository: &(impl WatchlistProductDynamoDbRepository + Sync),
    record: WatchlistProductRecord,
    target_state: WatchlistProductStateRecord,
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
                updated: OffsetDateTime::now_utc(),
            },
        )
        .await
        .map(|_| ())
        .map_err(|err| {
            error!(error = ?err, userId = %record.user_id, "Failed updating watchlist product state.");
            lambda_runtime::Error::from(err)
        })
}

async fn update_search_filter_state(
    repository: &(impl UserSearchFilterDynamoDbRepository + Sync),
    user_id: common::user_id::UserId,
    search_filter_id: common::user_search_filter_id::UserSearchFilterId,
    current_state: UserSearchFilterStateRecord,
    target_state: UserSearchFilterStateRecord,
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
            error!(error = ?err, userId = %user_id, "Failed updating search filter state.");
            lambda_runtime::Error::from(err)
        })
}

fn search_filter_state_update(state: UserSearchFilterStateRecord) -> UserSearchFilterRecordUpdate {
    UserSearchFilterRecordUpdate {
        name: None,
        notifications: None,
        state: Some(state),
        product_query: None,
        category_id: None,
        period_id: None,
        shop_name_query: None,
        exclude_shop_name_query: None,
        seller_name_query: None,
        exclude_seller_name_query: None,
        shop_slug_id_query: None,
        exclude_shop_slug_id_query: None,
        seller_slug_id_query: None,
        exclude_seller_slug_id_query: None,
        shop_type_query: None,
        price_query: None,
        state_query: None,
        created_query: None,
        updated_query: None,
        origin_year_query: None,
        authenticity_query: None,
        condition_query: None,
        provenance_query: None,
        restoration_query: None,
        auction_start_query: None,
        auction_end_query: None,
        language: None,
        currency: None,
        updated: OffsetDateTime::now_utc(),
    }
}
