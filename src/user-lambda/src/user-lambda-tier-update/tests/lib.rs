use common::resource_state::record::ResourceStateRecord;
use common::{currency::domain::Currency, language::domain::Language};
use fake::{Fake, Faker};
use product_watchlist::core::quota::WatchlistQuota;
use product_watchlist::dynamodb::{
    record::WatchlistProductRecord, repository::WatchlistProductDynamoDbRepository,
    repository::WatchlistProductDynamoDbRepositoryImpl,
};
use search_filter::dynamodb::{
    repository::UserSearchFilterDynamoDbRepository,
    repository::UserSearchFilterDynamoDbRepositoryImpl,
    user_search_filter_record::UserSearchFilterRecord,
};
use test_api::*;
use time::OffsetDateTime;
use user::{core::tier::UserTier, dynamodb::tier_record::UserTierRecord};
use user_lambda_tier_update::enforce_tier;

fn user_record(tier: UserTier) -> user::dynamodb::user_record::UserRecord {
    let mut user = Faker.fake::<user::dynamodb::user_record::UserRecord>();
    user.tier = UserTierRecord::from(tier);
    user
}

fn watchlist_record(
    user: &user::dynamodb::user_record::UserRecord,
    state: ResourceStateRecord,
    created: OffsetDateTime,
) -> WatchlistProductRecord {
    let mut record = Faker.fake::<WatchlistProductRecord>();
    record.user_id = user.user_id;
    record.pk = product_watchlist::dynamodb::record::mk_pk(&user.user_id);
    record.lsi1_sk = product_watchlist::dynamodb::record::mk_lsi1_sk(&created);
    record.state = state;
    record.created = created;
    record.updated = created;
    record
}

fn search_filter_record(
    user: &user::dynamodb::user_record::UserRecord,
    state: ResourceStateRecord,
    created: OffsetDateTime,
) -> UserSearchFilterRecord {
    let filter = search_filter::core::user_search_filter::UserSearchFilter {
        user_id: user.user_id,
        user_search_filter_id: Faker.fake(),
        name: Faker.fake(),
        notifications: true,
        state: state.into(),
        search: product::core::product_search::ProductSearch::new(Language::En, Currency::Eur),
        enhanced_search_description: None,
        created,
        updated: created,
        last_hybrid_search_matched: created,
    };
    filter.into()
}

#[localstack_test(services = [DynamoDB()])]
async fn should_deactivate_over_quota_resources_when_tier_is_downgraded() {
    let ddb = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb, "table_1");
    let search_filter_repository = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user = user_record(UserTier::Free);
    let old = OffsetDateTime::now_utc() - time::Duration::days(1);
    let new = OffsetDateTime::now_utc();
    let mut watch_records = Vec::new();
    for i in 0..=UserTier::Free.watchlist_quota() {
        watch_records.push(watchlist_record(
            &user,
            ResourceStateRecord::Active,
            old + time::Duration::seconds(i.into()),
        ));
    }
    let oldest_watch = watch_records.first().unwrap().clone();
    let newest_watch = watch_records.last().unwrap().clone();
    let old_filter = search_filter_record(&user, ResourceStateRecord::Active, old);
    let new_filter = search_filter_record(&user, ResourceStateRecord::Active, new);

    for record in watch_records {
        watchlist_repository
            .put_watchlist_record(record)
            .await
            .unwrap();
    }
    for record in [old_filter.clone(), new_filter.clone()] {
        search_filter_repository
            .put_user_search_filter_record(record)
            .await
            .unwrap();
    }

    enforce_tier(&watchlist_repository, &search_filter_repository, user)
        .await
        .unwrap();

    let watchlist = watchlist_repository
        .query_watchlist_records_all(&oldest_watch.user_id, false)
        .await
        .unwrap();
    let newest_watch_record = watchlist
        .iter()
        .find(|record| record.sk == newest_watch.sk)
        .unwrap();
    assert_eq!(ResourceStateRecord::Active, newest_watch_record.state);
    let old_watch_record = watchlist
        .iter()
        .find(|record| record.sk == oldest_watch.sk)
        .unwrap();
    assert_eq!(
        ResourceStateRecord::InactiveByRestrictedPlan,
        old_watch_record.state
    );

    let filters = search_filter_repository
        .query_user_search_filter_records(&old_filter.user_id, false)
        .await
        .unwrap();
    assert_eq!(ResourceStateRecord::Active, filters[0].state);
    let old_filter_record = filters
        .iter()
        .find(|record| record.sk == old_filter.sk)
        .unwrap();
    assert_eq!(
        ResourceStateRecord::InactiveByRestrictedPlan,
        old_filter_record.state
    );
}

#[localstack_test(services = [DynamoDB()])]
async fn should_reactivate_restricted_resources_when_tier_is_upgraded() {
    let ddb = get_dynamodb_client().await;
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb, "table_1");
    let search_filter_repository = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user = user_record(UserTier::Ultimate);
    let now = OffsetDateTime::now_utc();
    let watch = watchlist_record(&user, ResourceStateRecord::InactiveByRestrictedPlan, now);
    let filter = search_filter_record(&user, ResourceStateRecord::InactiveByRestrictedPlan, now);

    watchlist_repository
        .put_watchlist_record(watch.clone())
        .await
        .unwrap();
    search_filter_repository
        .put_user_search_filter_record(filter.clone())
        .await
        .unwrap();

    enforce_tier(&watchlist_repository, &search_filter_repository, user)
        .await
        .unwrap();

    let watch = watchlist_repository
        .get_watchlist_record(&watch.user_id, &watch.shop_id, &watch.shops_product_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ResourceStateRecord::Active, watch.state);

    let filter = search_filter_repository
        .get_user_search_filter_record(&filter.user_id, &filter.user_search_filter_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ResourceStateRecord::Active, filter.state);
}
