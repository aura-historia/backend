use common::actor::{domain::Actor, record::ActorRecord};
use common::resource_state::record::ResourceStateRecord;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use fake::{Fake, Faker};
use notification::core::notification::Notification;
use notification::core::notification_id::NotificationId;
use notification::service::notification_service::MockNotificationService;
use product::dynamodb::product_state_record::ProductStateRecord;
use product::opensearch::product_document::ProductDocument;
use product::opensearch::product_state_document::ProductStateDocument;
use product::opensearch::repository::{
    ProductOpenSearchRepository, ProductOpenSearchRepositoryImpl,
};
use product::service::query_service::QueryProductServiceImpl;
use product_pipeline_embed_text::service::MockMultimodalEmbeddingService;
use search_filter::core::quota::SearchFilterQuota;
use search_filter::dynamodb::repository::{
    UserSearchFilterDynamoDbRepository, UserSearchFilterDynamoDbRepositoryImpl,
};
use search_filter::dynamodb::user_search_filter_match_record::{
    UserSearchFilterMatchRecord, mk_lsi1_sk, mk_pk, mk_sk,
};
use search_filter::dynamodb::user_search_filter_record::UserSearchFilterRecord;
use search_filter::opensearch::repository::{
    UserSearchFilterOpenSearchRepository, UserSearchFilterOpenSearchRepositoryImpl,
};
use search_filter::opensearch::user_search_filter_document::UserSearchFilterDocument;
use search_filter::service::enhanced_search_match_service::MockEnhancedSearchMatchService;
use search_filter::service::user_search_filter_service::{
    UserSearchFilterService, UserSearchFilterServiceImpl,
};
use search_filter_lambda_periodic_match::service::{
    PeriodicMatcherService, PeriodicMatcherServiceImpl,
};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use test_api::*;
use time::OffsetDateTime;
use time::macros::datetime;
use user::core::tier::UserTier;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::user_service::{UserService, UserServiceImpl};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Builds a `UserSearchFilterRecord` that matches Listed products updated after
/// `last_hybrid_search_matched`. The `state_query` is `[Listed]` so the periodic
/// matcher will return matching products. All other filter fields are empty or
/// permissive so that matching is controlled purely by product state and timestamp.
fn mk_search_filter_record_with_state(
    user_id: UserId,
    filter_id: UserSearchFilterId,
    state: ResourceStateRecord,
) -> UserSearchFilterRecord {
    UserSearchFilterRecord {
        pk: format!("user#{user_id}"),
        sk: format!("search_filter#{filter_id}"),
        user_id,
        user_search_filter_id: filter_id,
        name: "Integration Test Filter".into(),
        enhanced_search_description: None,
        notifications: true,
        state,
        product_query: None,
        shop_name_query: HashSet::new(),
        exclude_shop_name_query: HashSet::new(),
        seller_name_query: HashSet::new(),
        exclude_seller_name_query: HashSet::new(),
        shop_slug_id_query: HashSet::new(),
        exclude_shop_slug_id_query: HashSet::new(),
        seller_slug_id_query: HashSet::new(),
        exclude_seller_slug_id_query: HashSet::new(),
        shop_type_query: HashSet::new(),
        country_query: HashSet::new(),
        continent_query: HashSet::new(),
        geo_address_distance_query: None,
        price_query: None,
        state_query: HashSet::from_iter([ProductStateRecord::Listed]),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        language: common::language::record::LanguageRecord::En,
        currency: common::currency::record::CurrencyRecord::Eur,
        created_by: ActorRecord::User(user_id),
        updated_by: ActorRecord::User(user_id),
        created: datetime!(2024-01-01 00:00:00 UTC),
        updated: datetime!(2024-01-02 00:00:00 UTC),
        last_hybrid_search_matched: datetime!(2024-01-02 00:00:00 UTC),
    }
}

/// Builds an Active `UserSearchFilterRecord` matching Listed products
/// updated after the 2024-01-02 watermark.
fn mk_search_filter_record(
    user_id: UserId,
    filter_id: UserSearchFilterId,
) -> UserSearchFilterRecord {
    mk_search_filter_record_with_state(user_id, filter_id, ResourceStateRecord::Active)
}

/// Build a `ProductDocument` for a Listed product in a known shop,
/// with `updated` set to 2024-01-03 — after the filter's watermark —
/// so the periodic matcher will pick it up.
fn mk_product_document(shop_id: ShopId, shops_product_id: &ShopsProductId) -> ProductDocument {
    let mut doc: ProductDocument = Faker.fake();
    doc.shop_id = shop_id;
    doc.seller_id = shop_id;
    doc.shops_product_id = shops_product_id.clone();
    doc.state = ProductStateDocument::Listed;
    doc.updated = datetime!(2024-01-03 00:00:00 UTC);
    doc.embedding = None;
    doc
}

/// Index a search-filter document in OpenSearch and wait for it to become visible.
async fn index_filter(
    os_repo: &UserSearchFilterOpenSearchRepositoryImpl<'_>,
    record: UserSearchFilterRecord,
) -> UserSearchFilterDocument {
    let document: UserSearchFilterDocument = record.try_into().unwrap();
    os_repo.index_document(document.clone()).await.unwrap();
    refresh_index("user_search_filters").await;
    document
}

/// Seed a filter in both DynamoDB (needed for `update_user_search_filter` during
/// watermark write-back) and OpenSearch (needed for `search_user_search_filters`
/// during the Active-filter scan).
async fn seed_filter(
    ddb_repo: &UserSearchFilterDynamoDbRepositoryImpl<'_>,
    os_repo: &UserSearchFilterOpenSearchRepositoryImpl<'_>,
    record: UserSearchFilterRecord,
) -> UserSearchFilterDocument {
    ddb_repo
        .put_user_search_filter_record(record.clone())
        .await
        .unwrap();
    index_filter(os_repo, record).await
}

/// Index a product document in OpenSearch and wait for it to become visible.
async fn index_product(os_repo: &ProductOpenSearchRepositoryImpl<'_>, doc: ProductDocument) {
    os_repo.create_product_documents(vec![doc]).await.unwrap();
    refresh_index("products").await;
}

/// Create a user via the service and return its `UserId`.
async fn create_user(user_service: &impl UserService, email: &str) -> UserId {
    let user_id = UserId::new();
    user_service
        .create_user(
            &common::actor::RequestContext {
                actor: Actor::User(user_id),
            },
            user::service::command::CreateUserCommand {
                id: user_id,
                email: email.parse().unwrap(),
            },
        )
        .await
        .unwrap();
    user_id
}

/// Pre-populate `count` match records for `(user_id, filter_id)` with
/// `created = now`, so that `count_user_search_filter_matches_for_this_month`
/// reflects them and the quota check sees the correct remaining balance.
async fn fill_quota(
    search_filter_repo: &(impl UserSearchFilterDynamoDbRepository + Sync),
    user_id: UserId,
    filter_id: UserSearchFilterId,
    count: u32,
) {
    let now = OffsetDateTime::now_utc();
    for i in 0..count {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let created = now - time::Duration::seconds(i as i64);
        let mut record = Faker.fake::<UserSearchFilterMatchRecord>();
        record.pk = mk_pk(&user_id);
        record.sk = mk_sk(&filter_id, &shop_id, &shops_product_id);
        record.lsi1_sk = mk_lsi1_sk(&created);
        record.user_id = user_id;
        record.user_search_filter_id = filter_id;
        record.shop_id = shop_id;
        record.shops_product_id = shops_product_id;
        record.created = created;
        record.updated = created;
        search_filter_repo
            .put_user_search_filter_match_record(record)
            .await
            .unwrap();
    }
}

/// Build a `MockNotificationService` that counts every `create_notification` call.
/// Returns the mock and an `Arc<AtomicUsize>` that tracks the number of calls.
fn mock_notification_service_counting_calls() -> (MockNotificationService, Arc<AtomicUsize>) {
    let call_count = Arc::new(AtomicUsize::new(0));
    let counter = call_count.clone();
    let mut svc = MockNotificationService::default();
    svc.expect_create_notification()
        .returning(move |_, event_id, cmd| {
            counter.fetch_add(1, Ordering::SeqCst);
            let user_id = cmd.user_id;
            let eid = *event_id;
            let payload: notification::core::notification::NotificationPayload = Faker.fake();
            Box::pin(async move {
                Ok(Notification {
                    user_id,
                    origin_event_id: eid,
                    notification_id: NotificationId::new(),
                    notification_type: None,
                    notification_payload: payload,
                    seen: false,
                    external: true,
                    created_by: Actor::System,
                    updated_by: Actor::System,
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                })
            })
        });
    (svc, call_count)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Happy path: one active filter + one Listed product updated after the
/// filter's watermark. Expects one match record, one notification, and the
/// watermark bumped past the original value.
#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_create_match_and_notification_when_filter_matches_product() {
    let ddb = get_dynamodb_client().await;
    let os = get_opensearch_client().await;

    let sf_ddb_repo = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_os_repo = UserSearchFilterOpenSearchRepositoryImpl::new(os);
    let product_os_repo = ProductOpenSearchRepositoryImpl::new(os);
    let user_repo = UserDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_service = UserServiceImpl::new(&user_repo);
    let sf_service =
        UserSearchFilterServiceImpl::with_opensearch(&sf_ddb_repo, &user_service, &sf_os_repo);
    let query_product_service = QueryProductServiceImpl::new(&product_os_repo);
    let embedding_service = MockMultimodalEmbeddingService::default();
    let enhanced_service = MockEnhancedSearchMatchService::default();

    // Set up user + active filter
    let user_id = create_user(&user_service, "happy-path@test.com").await;
    let filter_id = UserSearchFilterId::new();
    seed_filter(
        &sf_ddb_repo,
        &sf_os_repo,
        mk_search_filter_record(user_id, filter_id),
    )
    .await;

    // Set up product updated after the watermark
    let shop_id = ShopId::new();
    let shops_product_id = ShopsProductId::new();
    let doc = mk_product_document(shop_id, &shops_product_id);
    index_product(&product_os_repo, doc).await;

    let (notification_service, notification_count) = mock_notification_service_counting_calls();
    let service = PeriodicMatcherServiceImpl::new(
        &sf_service,
        &query_product_service,
        &embedding_service,
        &enhanced_service,
        &notification_service,
        &user_service,
    );

    let result = service.match_active_filters().await.unwrap();

    assert_eq!(result.filters_processed, 1);
    assert_eq!(result.matches_created, 1);
    assert_eq!(result.notifications_created, 1);
    assert_eq!(result.filters_failed, 0);

    // Match record must be persisted in DynamoDB
    let persisted_match = sf_service
        .find_search_filter_product_match(&user_id, &filter_id, &shop_id, &shops_product_id)
        .await
        .unwrap();
    assert!(
        persisted_match.is_some(),
        "Expected match record to be created"
    );

    // Notification must have been issued
    assert_eq!(
        notification_count.load(Ordering::SeqCst),
        1,
        "Expected exactly one notification"
    );

    // Watermark must have been bumped
    let updated_filter = sf_service
        .find_user_search_filter(&user_id, &filter_id)
        .await
        .unwrap();
    assert!(
        updated_filter.last_hybrid_search_matched > datetime!(2024-01-02 00:00:00 UTC),
        "Expected watermark to be updated"
    );
}

/// Active filter exists but no products are in OpenSearch — the filter should
/// still be counted as processed (watermark updated) with zero matches and
/// zero notifications.
#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_not_create_match_or_notification_when_no_products_match() {
    let ddb = get_dynamodb_client().await;
    let os = get_opensearch_client().await;

    let sf_ddb_repo = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_os_repo = UserSearchFilterOpenSearchRepositoryImpl::new(os);
    let product_os_repo = ProductOpenSearchRepositoryImpl::new(os);
    let user_repo = UserDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_service = UserServiceImpl::new(&user_repo);
    let sf_service =
        UserSearchFilterServiceImpl::with_opensearch(&sf_ddb_repo, &user_service, &sf_os_repo);
    let query_product_service = QueryProductServiceImpl::new(&product_os_repo);
    let embedding_service = MockMultimodalEmbeddingService::default();
    let enhanced_service = MockEnhancedSearchMatchService::default();

    // Active filter, no product indexed
    let user_id = create_user(&user_service, "no-products@test.com").await;
    let filter_id = UserSearchFilterId::new();
    seed_filter(
        &sf_ddb_repo,
        &sf_os_repo,
        mk_search_filter_record(user_id, filter_id),
    )
    .await;

    // Default mock panics on unexpected calls
    let notification_service = MockNotificationService::default();
    let service = PeriodicMatcherServiceImpl::new(
        &sf_service,
        &query_product_service,
        &embedding_service,
        &enhanced_service,
        &notification_service,
        &user_service,
    );

    let result = service.match_active_filters().await.unwrap();

    assert_eq!(result.filters_processed, 1);
    assert_eq!(result.matches_created, 0);
    assert_eq!(result.notifications_created, 0);
    assert_eq!(result.filters_failed, 0);

    // Watermark should still have been bumped even though no matches were created
    let updated_filter = sf_service
        .find_user_search_filter(&user_id, &filter_id)
        .await
        .unwrap();
    assert!(
        updated_filter.last_hybrid_search_matched > datetime!(2024-01-02 00:00:00 UTC),
        "Expected watermark to be updated even with zero matches"
    );
}

/// An InactiveByUser filter is never returned by the Active-filter scan,
/// so it must not be processed regardless of what products exist.
#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_not_process_inactive_filter() {
    let ddb = get_dynamodb_client().await;
    let os = get_opensearch_client().await;

    let sf_ddb_repo = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_os_repo = UserSearchFilterOpenSearchRepositoryImpl::new(os);
    let product_os_repo = ProductOpenSearchRepositoryImpl::new(os);
    let user_repo = UserDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_service = UserServiceImpl::new(&user_repo);
    let sf_service =
        UserSearchFilterServiceImpl::with_opensearch(&sf_ddb_repo, &user_service, &sf_os_repo);
    let query_product_service = QueryProductServiceImpl::new(&product_os_repo);
    let embedding_service = MockMultimodalEmbeddingService::default();
    let enhanced_service = MockEnhancedSearchMatchService::default();

    // Inactive filter — indexed in OpenSearch but will not be returned by
    // the Active-state scan
    let user_id = create_user(&user_service, "inactive-filter@test.com").await;
    let filter_id = UserSearchFilterId::new();
    index_filter(
        &sf_os_repo,
        mk_search_filter_record_with_state(user_id, filter_id, ResourceStateRecord::InactiveByUser),
    )
    .await;

    // Listed product that would match if the filter were active
    let shop_id = ShopId::new();
    let shops_product_id = ShopsProductId::new();
    index_product(
        &product_os_repo,
        mk_product_document(shop_id, &shops_product_id),
    )
    .await;

    // Default mock — panics if any notification method is invoked
    let notification_service = MockNotificationService::default();
    let service = PeriodicMatcherServiceImpl::new(
        &sf_service,
        &query_product_service,
        &embedding_service,
        &enhanced_service,
        &notification_service,
        &user_service,
    );

    let result = service.match_active_filters().await.unwrap();

    assert_eq!(result.filters_processed, 0);
    assert_eq!(result.matches_created, 0);
    assert_eq!(result.notifications_created, 0);
    assert_eq!(result.filters_failed, 0);

    // No match record should have been created for the inactive user
    let persisted_match = sf_service
        .find_search_filter_product_match(&user_id, &filter_id, &shop_id, &shops_product_id)
        .await
        .unwrap();
    assert!(
        persisted_match.is_none(),
        "Inactive filter must not produce a match record"
    );
}

/// When one Active and one InactiveByUser filter coexist for different users,
/// only the active user should receive a match record and notification.
#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_create_match_and_notification_only_for_active_filter_when_active_and_inactive_filters_coexist()
 {
    let ddb = get_dynamodb_client().await;
    let os = get_opensearch_client().await;

    let sf_ddb_repo = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_os_repo = UserSearchFilterOpenSearchRepositoryImpl::new(os);
    let product_os_repo = ProductOpenSearchRepositoryImpl::new(os);
    let user_repo = UserDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_service = UserServiceImpl::new(&user_repo);
    let sf_service =
        UserSearchFilterServiceImpl::with_opensearch(&sf_ddb_repo, &user_service, &sf_os_repo);
    let query_product_service = QueryProductServiceImpl::new(&product_os_repo);
    let embedding_service = MockMultimodalEmbeddingService::default();
    let enhanced_service = MockEnhancedSearchMatchService::default();

    // User A: active filter — seeded in DDB + OS
    let active_user_id = create_user(&user_service, "active-coexist@test.com").await;
    let active_filter_id = UserSearchFilterId::new();
    seed_filter(
        &sf_ddb_repo,
        &sf_os_repo,
        mk_search_filter_record(active_user_id, active_filter_id),
    )
    .await;

    // User B: inactive filter — indexed in OS only (will not be returned by Active scan)
    let inactive_user_id = create_user(&user_service, "inactive-coexist@test.com").await;
    let inactive_filter_id = UserSearchFilterId::new();
    index_filter(
        &sf_os_repo,
        mk_search_filter_record_with_state(
            inactive_user_id,
            inactive_filter_id,
            ResourceStateRecord::InactiveByUser,
        ),
    )
    .await;

    // Shared product
    let shop_id = ShopId::new();
    let shops_product_id = ShopsProductId::new();
    index_product(
        &product_os_repo,
        mk_product_document(shop_id, &shops_product_id),
    )
    .await;

    let (notification_service, notification_count) = mock_notification_service_counting_calls();
    let service = PeriodicMatcherServiceImpl::new(
        &sf_service,
        &query_product_service,
        &embedding_service,
        &enhanced_service,
        &notification_service,
        &user_service,
    );

    let result = service.match_active_filters().await.unwrap();

    // Only the active filter is processed
    assert_eq!(result.filters_processed, 1);
    assert_eq!(result.matches_created, 1);
    assert_eq!(result.notifications_created, 1);
    assert_eq!(result.filters_failed, 0);

    // Active user gets a match record
    let active_match = sf_service
        .find_search_filter_product_match(
            &active_user_id,
            &active_filter_id,
            &shop_id,
            &shops_product_id,
        )
        .await
        .unwrap();
    assert!(
        active_match.is_some(),
        "Active user must have a match record"
    );

    // Inactive user must not have a match record
    let inactive_match = sf_service
        .find_search_filter_product_match(
            &inactive_user_id,
            &inactive_filter_id,
            &shop_id,
            &shops_product_id,
        )
        .await
        .unwrap();
    assert!(
        inactive_match.is_none(),
        "Inactive user must not have a match record"
    );

    assert_eq!(
        notification_count.load(Ordering::SeqCst),
        1,
        "Exactly one notification expected (active user only)"
    );
}

/// If a match record for the same filter+product already exists in DynamoDB,
/// the service must skip that product — no duplicate match record and no
/// additional notification.
#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_skip_already_matched_product() {
    let ddb = get_dynamodb_client().await;
    let os = get_opensearch_client().await;

    let sf_ddb_repo = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_os_repo = UserSearchFilterOpenSearchRepositoryImpl::new(os);
    let product_os_repo = ProductOpenSearchRepositoryImpl::new(os);
    let user_repo = UserDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_service = UserServiceImpl::new(&user_repo);
    let sf_service =
        UserSearchFilterServiceImpl::with_opensearch(&sf_ddb_repo, &user_service, &sf_os_repo);
    let query_product_service = QueryProductServiceImpl::new(&product_os_repo);
    let embedding_service = MockMultimodalEmbeddingService::default();
    let enhanced_service = MockEnhancedSearchMatchService::default();

    let user_id = create_user(&user_service, "dedup-product@test.com").await;
    let filter_id = UserSearchFilterId::new();
    seed_filter(
        &sf_ddb_repo,
        &sf_os_repo,
        mk_search_filter_record(user_id, filter_id),
    )
    .await;

    let shop_id = ShopId::new();
    let shops_product_id = ShopsProductId::new();
    index_product(
        &product_os_repo,
        mk_product_document(shop_id, &shops_product_id),
    )
    .await;

    // Pre-insert a match record so the service should skip this product
    let now = OffsetDateTime::now_utc();
    let mut existing_match = Faker.fake::<UserSearchFilterMatchRecord>();
    existing_match.pk = mk_pk(&user_id);
    existing_match.sk = mk_sk(&filter_id, &shop_id, &shops_product_id);
    existing_match.lsi1_sk = mk_lsi1_sk(&now);
    existing_match.user_id = user_id;
    existing_match.user_search_filter_id = filter_id;
    existing_match.shop_id = shop_id;
    existing_match.shops_product_id = shops_product_id.clone();
    existing_match.created = now;
    existing_match.updated = now;
    sf_ddb_repo
        .put_user_search_filter_match_record(existing_match)
        .await
        .unwrap();

    // Default mock — panics if create_notification is called unexpectedly
    let notification_service = MockNotificationService::default();
    let service = PeriodicMatcherServiceImpl::new(
        &sf_service,
        &query_product_service,
        &embedding_service,
        &enhanced_service,
        &notification_service,
        &user_service,
    );

    let result = service.match_active_filters().await.unwrap();

    assert_eq!(result.filters_processed, 1);
    assert_eq!(result.matches_created, 0, "Duplicate match must be skipped");
    assert_eq!(result.notifications_created, 0);
    assert_eq!(result.filters_failed, 0);
}

/// When the user's monthly notification quota is already exhausted, the match
/// record IS created (the product is new) but no notification is emitted.
#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_create_match_but_not_notification_when_quota_exhausted() {
    let ddb = get_dynamodb_client().await;
    let os = get_opensearch_client().await;

    let sf_ddb_repo = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_os_repo = UserSearchFilterOpenSearchRepositoryImpl::new(os);
    let product_os_repo = ProductOpenSearchRepositoryImpl::new(os);
    let user_repo = UserDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_service = UserServiceImpl::new(&user_repo);
    let sf_service =
        UserSearchFilterServiceImpl::with_opensearch(&sf_ddb_repo, &user_service, &sf_os_repo);
    let query_product_service = QueryProductServiceImpl::new(&product_os_repo);
    let embedding_service = MockMultimodalEmbeddingService::default();
    let enhanced_service = MockEnhancedSearchMatchService::default();

    let user_id = create_user(&user_service, "quota-exhausted@test.com").await;
    let filter_id = UserSearchFilterId::new();
    seed_filter(
        &sf_ddb_repo,
        &sf_os_repo,
        mk_search_filter_record(user_id, filter_id),
    )
    .await;

    // Fill the Free tier quota entirely
    let free_quota = UserTier::Free.search_filter_match_quota();
    fill_quota(&sf_ddb_repo, user_id, filter_id, free_quota).await;

    let shop_id = ShopId::new();
    let shops_product_id = ShopsProductId::new();
    index_product(
        &product_os_repo,
        mk_product_document(shop_id, &shops_product_id),
    )
    .await;

    // Default mock — panics if create_notification is called
    let notification_service = MockNotificationService::default();
    let service = PeriodicMatcherServiceImpl::new(
        &sf_service,
        &query_product_service,
        &embedding_service,
        &enhanced_service,
        &notification_service,
        &user_service,
    );

    let result = service.match_active_filters().await.unwrap();

    assert_eq!(result.filters_processed, 1);
    assert_eq!(
        result.matches_created, 1,
        "Match record must be created even when quota is exhausted"
    );
    assert_eq!(
        result.notifications_created, 0,
        "No notification when quota is zero"
    );
    assert_eq!(result.filters_failed, 0);

    // Match record IS persisted despite quota being exhausted
    let persisted_match = sf_service
        .find_search_filter_product_match(&user_id, &filter_id, &shop_id, &shops_product_id)
        .await
        .unwrap();
    assert!(
        persisted_match.is_some(),
        "Match record must be created even when quota is exhausted"
    );
}

/// Two active filters for different users share the same product: user A is
/// below quota (gets match + notification), user B has an exhausted quota
/// (gets match but no notification).
#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_create_matches_for_both_users_but_notification_only_for_quota_eligible_user() {
    let ddb = get_dynamodb_client().await;
    let os = get_opensearch_client().await;

    let sf_ddb_repo = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_os_repo = UserSearchFilterOpenSearchRepositoryImpl::new(os);
    let product_os_repo = ProductOpenSearchRepositoryImpl::new(os);
    let user_repo = UserDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_service = UserServiceImpl::new(&user_repo);
    let sf_service =
        UserSearchFilterServiceImpl::with_opensearch(&sf_ddb_repo, &user_service, &sf_os_repo);
    let query_product_service = QueryProductServiceImpl::new(&product_os_repo);
    let embedding_service = MockMultimodalEmbeddingService::default();
    let enhanced_service = MockEnhancedSearchMatchService::default();

    // User A: below quota — will get a notification
    let user_a = create_user(&user_service, "user-a-quota@test.com").await;
    let filter_a = UserSearchFilterId::new();
    seed_filter(
        &sf_ddb_repo,
        &sf_os_repo,
        mk_search_filter_record(user_a, filter_a),
    )
    .await;

    // User B: quota exhausted — gets a match but no notification
    let user_b = create_user(&user_service, "user-b-quota@test.com").await;
    let filter_b = UserSearchFilterId::new();
    seed_filter(
        &sf_ddb_repo,
        &sf_os_repo,
        mk_search_filter_record(user_b, filter_b),
    )
    .await;
    let free_quota = UserTier::Free.search_filter_match_quota();
    fill_quota(&sf_ddb_repo, user_b, filter_b, free_quota).await;

    // Shared product
    let shop_id = ShopId::new();
    let shops_product_id = ShopsProductId::new();
    index_product(
        &product_os_repo,
        mk_product_document(shop_id, &shops_product_id),
    )
    .await;

    let (notification_service, notification_count) = mock_notification_service_counting_calls();
    let service = PeriodicMatcherServiceImpl::new(
        &sf_service,
        &query_product_service,
        &embedding_service,
        &enhanced_service,
        &notification_service,
        &user_service,
    );

    let result = service.match_active_filters().await.unwrap();

    assert_eq!(
        result.filters_processed, 2,
        "Both filters must be processed"
    );
    assert_eq!(result.matches_created, 2, "Both users get a match record");
    assert_eq!(
        result.notifications_created, 1,
        "Only quota-eligible user gets a notification"
    );
    assert_eq!(result.filters_failed, 0);

    // Both match records must be persisted
    let match_a = sf_service
        .find_search_filter_product_match(&user_a, &filter_a, &shop_id, &shops_product_id)
        .await
        .unwrap();
    assert!(match_a.is_some(), "User A match must be persisted");

    let match_b = sf_service
        .find_search_filter_product_match(&user_b, &filter_b, &shop_id, &shops_product_id)
        .await
        .unwrap();
    assert!(match_b.is_some(), "User B match must be persisted");

    assert_eq!(
        notification_count.load(Ordering::SeqCst),
        1,
        "Exactly one notification — for the quota-eligible user"
    );
}

/// A product whose `updated` timestamp is strictly before
/// `filter.last_hybrid_search_matched` must not be returned by OpenSearch
/// because the service adds 1ns to the watermark as the `updated_query.min`.
/// The filter is still processed and its watermark updated.
#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_not_match_product_updated_before_last_hybrid_search_matched() {
    let ddb = get_dynamodb_client().await;
    let os = get_opensearch_client().await;

    let sf_ddb_repo = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_os_repo = UserSearchFilterOpenSearchRepositoryImpl::new(os);
    let product_os_repo = ProductOpenSearchRepositoryImpl::new(os);
    let user_repo = UserDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_service = UserServiceImpl::new(&user_repo);
    let sf_service =
        UserSearchFilterServiceImpl::with_opensearch(&sf_ddb_repo, &user_service, &sf_os_repo);
    let query_product_service = QueryProductServiceImpl::new(&product_os_repo);
    let embedding_service = MockMultimodalEmbeddingService::default();
    let enhanced_service = MockEnhancedSearchMatchService::default();

    let user_id = create_user(&user_service, "old-product@test.com").await;
    let filter_id = UserSearchFilterId::new();
    // watermark = 2024-01-02; service will query updated > 2024-01-02 + 1ns
    seed_filter(
        &sf_ddb_repo,
        &sf_os_repo,
        mk_search_filter_record(user_id, filter_id),
    )
    .await;

    // Product updated on 2024-01-01 — before the watermark
    let shop_id = ShopId::new();
    let shops_product_id = ShopsProductId::new();
    let mut old_doc = mk_product_document(shop_id, &shops_product_id);
    old_doc.updated = datetime!(2024-01-01 00:00:00 UTC);
    index_product(&product_os_repo, old_doc).await;

    // Default mock — panics if create_notification is called
    let notification_service = MockNotificationService::default();
    let service = PeriodicMatcherServiceImpl::new(
        &sf_service,
        &query_product_service,
        &embedding_service,
        &enhanced_service,
        &notification_service,
        &user_service,
    );

    let result = service.match_active_filters().await.unwrap();

    assert_eq!(result.filters_processed, 1);
    assert_eq!(
        result.matches_created, 0,
        "Product before watermark must not match"
    );
    assert_eq!(result.notifications_created, 0);
    assert_eq!(result.filters_failed, 0);

    // No match record for the stale product
    let persisted_match = sf_service
        .find_search_filter_product_match(&user_id, &filter_id, &shop_id, &shops_product_id)
        .await
        .unwrap();
    assert!(
        persisted_match.is_none(),
        "Product updated before watermark must not produce a match"
    );

    // Watermark should still be bumped
    let updated_filter = sf_service
        .find_user_search_filter(&user_id, &filter_id)
        .await
        .unwrap();
    assert!(
        updated_filter.last_hybrid_search_matched > datetime!(2024-01-02 00:00:00 UTC),
        "Watermark must be updated even when no products matched"
    );
}

/// When there are no Active filters in OpenSearch the service must return
/// a zero-result `PeriodicMatcherResult` immediately without error.
#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_return_zero_results_when_no_active_filters_exist() {
    let ddb = get_dynamodb_client().await;
    let os = get_opensearch_client().await;

    let sf_ddb_repo = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_os_repo = UserSearchFilterOpenSearchRepositoryImpl::new(os);
    let product_os_repo = ProductOpenSearchRepositoryImpl::new(os);
    let user_repo = UserDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_service = UserServiceImpl::new(&user_repo);
    let sf_service =
        UserSearchFilterServiceImpl::with_opensearch(&sf_ddb_repo, &user_service, &sf_os_repo);
    let query_product_service = QueryProductServiceImpl::new(&product_os_repo);
    let embedding_service = MockMultimodalEmbeddingService::default();
    let enhanced_service = MockEnhancedSearchMatchService::default();

    // No filters and no products — nothing to do
    let notification_service = MockNotificationService::default();
    let service = PeriodicMatcherServiceImpl::new(
        &sf_service,
        &query_product_service,
        &embedding_service,
        &enhanced_service,
        &notification_service,
        &user_service,
    );

    let result = service.match_active_filters().await.unwrap();

    assert_eq!(result.filters_processed, 0);
    assert_eq!(result.matches_created, 0);
    assert_eq!(result.notifications_created, 0);
    assert_eq!(result.filters_failed, 0);
}

/// One active filter paired with two distinct Listed products — both updated
/// after the watermark — must produce two match records and two notifications.
#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_create_matches_for_multiple_products_matching_same_filter() {
    let ddb = get_dynamodb_client().await;
    let os = get_opensearch_client().await;

    let sf_ddb_repo = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_os_repo = UserSearchFilterOpenSearchRepositoryImpl::new(os);
    let product_os_repo = ProductOpenSearchRepositoryImpl::new(os);
    let user_repo = UserDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_service = UserServiceImpl::new(&user_repo);
    let sf_service =
        UserSearchFilterServiceImpl::with_opensearch(&sf_ddb_repo, &user_service, &sf_os_repo);
    let query_product_service = QueryProductServiceImpl::new(&product_os_repo);
    let embedding_service = MockMultimodalEmbeddingService::default();
    let enhanced_service = MockEnhancedSearchMatchService::default();

    let user_id = create_user(&user_service, "multi-product@test.com").await;
    let filter_id = UserSearchFilterId::new();
    seed_filter(
        &sf_ddb_repo,
        &sf_os_repo,
        mk_search_filter_record(user_id, filter_id),
    )
    .await;

    // Two distinct products, both updated after the watermark
    let shop_id = ShopId::new();

    let shops_product_id_1 = ShopsProductId::new();
    let doc1 = mk_product_document(shop_id, &shops_product_id_1);

    let shops_product_id_2 = ShopsProductId::new();
    let doc2 = mk_product_document(shop_id, &shops_product_id_2);

    // Index both products; refresh once after both are written
    product_os_repo
        .create_product_documents(vec![doc1, doc2])
        .await
        .unwrap();
    refresh_index("products").await;

    let (notification_service, notification_count) = mock_notification_service_counting_calls();
    let service = PeriodicMatcherServiceImpl::new(
        &sf_service,
        &query_product_service,
        &embedding_service,
        &enhanced_service,
        &notification_service,
        &user_service,
    );

    let result = service.match_active_filters().await.unwrap();

    assert_eq!(result.filters_processed, 1);
    assert_eq!(
        result.matches_created, 2,
        "Both products must produce a match"
    );
    assert_eq!(
        result.notifications_created, 2,
        "Both products must produce a notification"
    );
    assert_eq!(result.filters_failed, 0);

    let match1 = sf_service
        .find_search_filter_product_match(&user_id, &filter_id, &shop_id, &shops_product_id_1)
        .await
        .unwrap();
    assert!(match1.is_some(), "Match record for product 1 must exist");

    let match2 = sf_service
        .find_search_filter_product_match(&user_id, &filter_id, &shop_id, &shops_product_id_2)
        .await
        .unwrap();
    assert!(match2.is_some(), "Match record for product 2 must exist");

    assert_eq!(
        notification_count.load(Ordering::SeqCst),
        2,
        "Two notifications — one per product"
    );
}
