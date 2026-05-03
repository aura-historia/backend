use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
use common::resource_state::record::ResourceStateRecord;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use fake::{Fake, Faker};
use lambda_runtime::{Context, LambdaEvent};
use notification::service::notification_service::{
    CreateNotificationsResult, MockNotificationService,
};
use product::dynamodb::product_record::{
    ProductRecord, mk_pk as product_mk_pk, mk_sk as product_mk_sk,
};
use product::dynamodb::product_state_record::ProductStateRecord;
use product::dynamodb::repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl};
use product::service::get_service::GetProductServiceImpl;
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
use search_filter_lambda_percolate_product::handler;
use search_filter_lambda_percolate_product::service::ProductMatcherServiceImpl;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use test_api::*;
use time::OffsetDateTime;
use time::macros::datetime;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::user_service::{UserService, UserServiceImpl};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Builds a `UserSearchFilterRecord` whose percolation query matches
/// products with state `Listed` (via `state_query`). All other filter
/// fields are empty/permissive so the filter matches any product that
/// is in the Listed state — allowing tests to control matching through
/// the product record's state alone.
fn mk_search_filter_record(
    user_id: UserId,
    filter_id: UserSearchFilterId,
) -> UserSearchFilterRecord {
    UserSearchFilterRecord {
        pk: format!("user#{user_id}"),
        sk: format!("search_filter#{filter_id}"),
        user_id,
        user_search_filter_id: filter_id,
        name: "Integration Test Filter".into(),
        enhanced_search_description: None,
        notifications: true,
        state: ResourceStateRecord::Active,
        product_query: None,
        category_id: HashSet::new(),
        period_id: HashSet::new(),
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
        origin_year_query: None,
        authenticity_query: HashSet::new(),
        condition_query: HashSet::new(),
        provenance_query: HashSet::new(),
        restoration_query: HashSet::new(),
        language: common::language::record::LanguageRecord::En,
        currency: common::currency::record::CurrencyRecord::Eur,
        created: datetime!(2024-01-01 00:00:00 UTC),
        updated: datetime!(2024-01-02 00:00:00 UTC),
    }
}

/// Build a `ProductRecord` for a Listed product in a known shop.
fn mk_product_record(shop_id: ShopId, shops_product_id: &ShopsProductId) -> ProductRecord {
    let mut record: ProductRecord = Faker.fake();
    record.shop_id = shop_id;
    record.seller_id = shop_id;
    record.shops_product_id = shops_product_id.clone();
    record.pk = product_mk_pk(&shop_id, shops_product_id);
    record.sk = product_mk_sk().to_string();
    record.state = ProductStateRecord::Listed;
    record
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

/// Wrap a `ProductDomainEventRecord` into the SQS → EventBridge → DynamoDB
/// Streams envelope that the handler expects.
fn mk_event_bridge_body(
    record: &product::dynamodb::product_event_record::domain::ProductDomainEventRecord,
) -> String {
    use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
    use aws_lambda_events::eventbridge::EventBridgeEvent;

    let new_image = serde_dynamo::to_item(record).unwrap();

    let mut stream_record = StreamRecord::default();
    stream_record.new_image = new_image;

    let mut event_record = EventRecord::default();
    event_record.event_name = "INSERT".to_string();
    event_record.change = stream_record;

    let mut event = EventBridgeEvent::<EventRecord>::default();
    event.detail_type = "DynamoDBStreamRecord".to_string();
    event.source = "test-source".to_string();
    event.detail = event_record;

    serde_json::to_string(&event).unwrap()
}

/// Wrap a `ProductEnrichmentEventRecord` into the SQS → EventBridge → DynamoDB
/// Streams envelope that the handler expects.
fn mk_event_bridge_body_enrichment(
    record: &product::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord,
) -> String {
    use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
    use aws_lambda_events::eventbridge::EventBridgeEvent;

    let new_image = serde_dynamo::to_item(record).unwrap();

    let mut stream_record = StreamRecord::default();
    stream_record.new_image = new_image;

    let mut event_record = EventRecord::default();
    event_record.event_name = "INSERT".to_string();
    event_record.change = stream_record;

    let mut event = EventBridgeEvent::<EventRecord>::default();
    event.detail_type = "DynamoDBStreamRecord".to_string();
    event.source = "test-source".to_string();
    event.detail = event_record;

    serde_json::to_string(&event).unwrap()
}

fn mk_sqs_event(messages: Vec<SqsMessage>) -> LambdaEvent<SqsEvent> {
    let mut sqs_event = SqsEvent::default();
    sqs_event.records = messages;
    LambdaEvent::new(sqs_event, Context::default())
}

fn mk_sqs_message(body: &str) -> SqsMessage {
    let mut msg = SqsMessage::default();
    msg.message_id = Some(Uuid::new_v4().to_string());
    msg.body = Some(body.to_string());
    msg
}

/// Build a `ProductDomainEventRecord` for a state-change event matching
/// the given product record.
fn mk_state_change_event_record(
    product_record: &ProductRecord,
) -> product::dynamodb::product_event_record::domain::ProductDomainEventRecord {
    use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
    use product::dynamodb::product_event_type_record::domain::ProductDomainEventTypeRecord;

    let mut record: ProductDomainEventRecord = Faker.fake();
    record.product_id = product_record.product_id;
    record.shop_id = product_record.shop_id;
    record.seller_id = product_record.seller_id;
    record.shops_product_id = product_record.shops_product_id.clone();
    record.event_type = ProductDomainEventTypeRecord::DomainStateChanged;
    record.event_type_schema_version = 1;
    record.old_state = Some(ProductStateRecord::Available);
    record.new_state = Some(ProductStateRecord::Listed);
    record
}

/// Create a user via the service and return its UserId.
async fn create_user(user_service: &impl UserService, email: &str) -> UserId {
    let user_id = UserId::new();
    user_service
        .create_user(user::service::command::CreateUserCommand {
            id: user_id,
            email: email.parse().unwrap(),
        })
        .await
        .unwrap();
    user_id
}

/// Fill quota with existing match records for the given user.
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

fn mock_notification_service_counting_calls() -> (MockNotificationService, Arc<AtomicUsize>) {
    let call_count = Arc::new(AtomicUsize::new(0));
    let counter = call_count.clone();
    let mut svc = MockNotificationService::default();
    svc.expect_create_notifications().returning(
        move |_, cmds: Vec<notification::service::command::CreateNotificationCommand>| {
            counter.fetch_add(cmds.len(), Ordering::SeqCst);
            Box::pin(async {
                CreateNotificationsResult {
                    processed: Vec::new(),
                    unprocessed: Vec::new(),
                }
            })
        },
    );
    (svc, call_count)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Happy path: a search-filter matches a product event → match record
/// persisted AND notification created.
#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_create_match_and_notification_when_filter_matches_product() {
    let ddb = get_dynamodb_client().await;
    let os = get_opensearch_client().await;

    let product_repo = ProductDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_ddb_repo = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_os_repo = UserSearchFilterOpenSearchRepositoryImpl::new(os);
    let user_repo = UserDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_service = UserServiceImpl::new(&user_repo);
    let get_product_service = GetProductServiceImpl::new(&product_repo);
    let enhanced_service = MockEnhancedSearchMatchService::default();

    let sf_service =
        UserSearchFilterServiceImpl::with_opensearch(&sf_ddb_repo, &user_service, &sf_os_repo);

    // Set up user + search filter
    let user_id = create_user(&user_service, "happy@test.com").await;
    let filter_id = UserSearchFilterId::new();
    let record = mk_search_filter_record(user_id, filter_id);
    index_filter(&sf_os_repo, record).await;

    // Set up product
    let shop_id = ShopId::new();
    let shops_product_id = ShopsProductId::new();
    let product_record = mk_product_record(shop_id, &shops_product_id);
    product_repo
        .put_product_records([product_record.clone()].into())
        .await
        .unwrap();

    let (notification_service, notification_count) = mock_notification_service_counting_calls();

    let matcher = ProductMatcherServiceImpl::new(
        &sf_service,
        &get_product_service,
        &enhanced_service,
        &user_service,
    );

    let event_record = mk_state_change_event_record(&product_record);
    let body = mk_event_bridge_body(&event_record);
    let event = mk_sqs_event(vec![mk_sqs_message(&body)]);

    let response = handler(&matcher, &notification_service, &sf_service, event)
        .await
        .unwrap();

    assert!(
        response.batch_item_failures.is_empty(),
        "Expected no batch failures"
    );

    // Verify match record was persisted in DynamoDB
    let persisted_match = sf_service
        .find_search_filter_product_match(&user_id, &filter_id, &shop_id, &shops_product_id)
        .await
        .unwrap();
    assert!(
        persisted_match.is_some(),
        "Expected match record to be persisted"
    );

    // Verify notification was created
    assert_eq!(
        notification_count.load(Ordering::SeqCst),
        1,
        "Expected one notification to be created"
    );
}

/// No percolation match → no match records and no notifications.
#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_not_create_match_or_notification_when_no_filter_matches() {
    let ddb = get_dynamodb_client().await;
    let os = get_opensearch_client().await;

    let product_repo = ProductDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_ddb_repo = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_os_repo = UserSearchFilterOpenSearchRepositoryImpl::new(os);
    let user_repo = UserDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_service = UserServiceImpl::new(&user_repo);
    let get_product_service = GetProductServiceImpl::new(&product_repo);
    let enhanced_service = MockEnhancedSearchMatchService::default();

    let sf_service =
        UserSearchFilterServiceImpl::with_opensearch(&sf_ddb_repo, &user_service, &sf_os_repo);

    // No search filter indexed → no percolation match

    // Set up product
    let shop_id = ShopId::new();
    let shops_product_id = ShopsProductId::new();
    let product_record = mk_product_record(shop_id, &shops_product_id);
    product_repo
        .put_product_records([product_record.clone()].into())
        .await
        .unwrap();

    // NotificationService should NOT be called
    let notification_service = MockNotificationService::default();

    let matcher = ProductMatcherServiceImpl::new(
        &sf_service,
        &get_product_service,
        &enhanced_service,
        &user_service,
    );

    let event_record = mk_state_change_event_record(&product_record);
    let body = mk_event_bridge_body(&event_record);
    let event = mk_sqs_event(vec![mk_sqs_message(&body)]);

    let response = handler(&matcher, &notification_service, &sf_service, event)
        .await
        .unwrap();

    assert!(response.batch_item_failures.is_empty());
}

/// Already-matched filter is skipped (dedup) → no duplicate match or notification.
#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_skip_already_matched_filter_for_same_product() {
    let ddb = get_dynamodb_client().await;
    let os = get_opensearch_client().await;

    let product_repo = ProductDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_ddb_repo = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_os_repo = UserSearchFilterOpenSearchRepositoryImpl::new(os);
    let user_repo = UserDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_service = UserServiceImpl::new(&user_repo);
    let get_product_service = GetProductServiceImpl::new(&product_repo);
    let enhanced_service = MockEnhancedSearchMatchService::default();

    let sf_service =
        UserSearchFilterServiceImpl::with_opensearch(&sf_ddb_repo, &user_service, &sf_os_repo);

    let user_id = create_user(&user_service, "dedup@test.com").await;
    let filter_id = UserSearchFilterId::new();
    let filter_record = mk_search_filter_record(user_id, filter_id);
    index_filter(&sf_os_repo, filter_record).await;

    let shop_id = ShopId::new();
    let shops_product_id = ShopsProductId::new();
    let product_record = mk_product_record(shop_id, &shops_product_id);
    product_repo
        .put_product_records([product_record.clone()].into())
        .await
        .unwrap();

    // Pre-insert a match record for this filter+product
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

    // Notification should NOT be called because no new matches
    let notification_service = MockNotificationService::default();

    let matcher = ProductMatcherServiceImpl::new(
        &sf_service,
        &get_product_service,
        &enhanced_service,
        &user_service,
    );

    let event_record = mk_state_change_event_record(&product_record);
    let body = mk_event_bridge_body(&event_record);
    let event = mk_sqs_event(vec![mk_sqs_message(&body)]);

    let response = handler(&matcher, &notification_service, &sf_service, event)
        .await
        .unwrap();

    assert!(response.batch_item_failures.is_empty());
}

/// Quota exceeded → match record IS created but notification is NOT.
#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_create_match_but_not_notification_when_quota_exceeded() {
    use search_filter::core::quota::SearchFilterQuota;
    use user::core::tier::UserTier;

    let ddb = get_dynamodb_client().await;
    let os = get_opensearch_client().await;

    let product_repo = ProductDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_ddb_repo = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_os_repo = UserSearchFilterOpenSearchRepositoryImpl::new(os);
    let user_repo = UserDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_service = UserServiceImpl::new(&user_repo);
    let get_product_service = GetProductServiceImpl::new(&product_repo);
    let enhanced_service = MockEnhancedSearchMatchService::default();

    let sf_service =
        UserSearchFilterServiceImpl::with_opensearch(&sf_ddb_repo, &user_service, &sf_os_repo);

    let user_id = create_user(&user_service, "quota@test.com").await;
    let filter_id = UserSearchFilterId::new();
    let filter_record = mk_search_filter_record(user_id, filter_id);
    index_filter(&sf_os_repo, filter_record).await;

    let free_quota = UserTier::Free.search_filter_match_quota();
    fill_quota(&sf_ddb_repo, user_id, filter_id, free_quota).await;

    let shop_id = ShopId::new();
    let shops_product_id = ShopsProductId::new();
    let product_record = mk_product_record(shop_id, &shops_product_id);
    product_repo
        .put_product_records([product_record.clone()].into())
        .await
        .unwrap();

    // Notification should NOT be called
    let notification_service = MockNotificationService::default();

    let matcher = ProductMatcherServiceImpl::new(
        &sf_service,
        &get_product_service,
        &enhanced_service,
        &user_service,
    );

    let event_record = mk_state_change_event_record(&product_record);
    let body = mk_event_bridge_body(&event_record);
    let event = mk_sqs_event(vec![mk_sqs_message(&body)]);

    let response = handler(&matcher, &notification_service, &sf_service, event)
        .await
        .unwrap();

    assert!(
        response.batch_item_failures.is_empty(),
        "Expected no batch failures"
    );

    // Match record IS persisted
    let persisted_match = sf_service
        .find_search_filter_product_match(&user_id, &filter_id, &shop_id, &shops_product_id)
        .await
        .unwrap();
    assert!(
        persisted_match.is_some(),
        "Expected match record to be persisted even when quota is exceeded"
    );
}

/// Two filters for different users: one below quota, one above quota.
/// Both get match records, only the below-quota user gets a notification.
#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_create_matches_for_both_and_notification_only_for_quota_eligible_user() {
    use search_filter::core::quota::SearchFilterQuota;
    use user::core::tier::UserTier;

    let ddb = get_dynamodb_client().await;
    let os = get_opensearch_client().await;

    let product_repo = ProductDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_ddb_repo = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_os_repo = UserSearchFilterOpenSearchRepositoryImpl::new(os);
    let user_repo = UserDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_service = UserServiceImpl::new(&user_repo);
    let get_product_service = GetProductServiceImpl::new(&product_repo);
    let enhanced_service = MockEnhancedSearchMatchService::default();

    let sf_service =
        UserSearchFilterServiceImpl::with_opensearch(&sf_ddb_repo, &user_service, &sf_os_repo);

    // User A: below quota
    let user_a = create_user(&user_service, "user-a@test.com").await;
    let filter_a = UserSearchFilterId::new();
    index_filter(&sf_os_repo, mk_search_filter_record(user_a, filter_a)).await;

    // User B: at quota limit
    let user_b = create_user(&user_service, "user-b@test.com").await;
    let filter_b = UserSearchFilterId::new();
    index_filter(&sf_os_repo, mk_search_filter_record(user_b, filter_b)).await;
    let free_quota = UserTier::Free.search_filter_match_quota();
    fill_quota(&sf_ddb_repo, user_b, filter_b, free_quota).await;

    // Shared product
    let shop_id = ShopId::new();
    let shops_product_id = ShopsProductId::new();
    let product_record = mk_product_record(shop_id, &shops_product_id);
    product_repo
        .put_product_records([product_record.clone()].into())
        .await
        .unwrap();

    let (notification_service, notification_count) = mock_notification_service_counting_calls();

    let matcher = ProductMatcherServiceImpl::new(
        &sf_service,
        &get_product_service,
        &enhanced_service,
        &user_service,
    );

    let event_record = mk_state_change_event_record(&product_record);
    let body = mk_event_bridge_body(&event_record);
    let event = mk_sqs_event(vec![mk_sqs_message(&body)]);

    let response = handler(&matcher, &notification_service, &sf_service, event)
        .await
        .unwrap();

    assert!(response.batch_item_failures.is_empty());

    // Both match records persisted
    let match_a = sf_service
        .find_search_filter_product_match(&user_a, &filter_a, &shop_id, &shops_product_id)
        .await
        .unwrap();
    assert!(match_a.is_some(), "User A match should be persisted");

    let match_b = sf_service
        .find_search_filter_product_match(&user_b, &filter_b, &shop_id, &shops_product_id)
        .await
        .unwrap();
    assert!(match_b.is_some(), "User B match should be persisted");

    // Only user A (below quota) gets a notification
    assert_eq!(
        notification_count.load(Ordering::SeqCst),
        1,
        "Only one notification expected (quota-eligible user)"
    );
}

/// Invalid SQS message body → batch item failure, no match or notification.
#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_return_batch_failure_when_sqs_message_body_invalid() {
    let ddb = get_dynamodb_client().await;
    let os = get_opensearch_client().await;

    let sf_ddb_repo = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_os_repo = UserSearchFilterOpenSearchRepositoryImpl::new(os);
    let product_repo = ProductDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_repo = UserDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_service = UserServiceImpl::new(&user_repo);
    let get_product_service = GetProductServiceImpl::new(&product_repo);
    let enhanced_service = MockEnhancedSearchMatchService::default();

    let sf_service =
        UserSearchFilterServiceImpl::with_opensearch(&sf_ddb_repo, &user_service, &sf_os_repo);

    let notification_service = MockNotificationService::default();

    let matcher = ProductMatcherServiceImpl::new(
        &sf_service,
        &get_product_service,
        &enhanced_service,
        &user_service,
    );

    let event = mk_sqs_event(vec![mk_sqs_message("{\"not\":\"a valid event\"}")]);

    let response = handler(&matcher, &notification_service, &sf_service, event)
        .await
        .unwrap();

    assert_eq!(
        response.batch_item_failures.len(),
        1,
        "Expected one batch failure for invalid message"
    );
}

/// Empty SQS batch → no failures, no processing.
#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_succeed_when_sqs_batch_is_empty() {
    let ddb = get_dynamodb_client().await;
    let os = get_opensearch_client().await;

    let sf_ddb_repo = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_os_repo = UserSearchFilterOpenSearchRepositoryImpl::new(os);
    let product_repo = ProductDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_repo = UserDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_service = UserServiceImpl::new(&user_repo);
    let get_product_service = GetProductServiceImpl::new(&product_repo);
    let enhanced_service = MockEnhancedSearchMatchService::default();

    let sf_service =
        UserSearchFilterServiceImpl::with_opensearch(&sf_ddb_repo, &user_service, &sf_os_repo);

    let notification_service = MockNotificationService::default();

    let matcher = ProductMatcherServiceImpl::new(
        &sf_service,
        &get_product_service,
        &enhanced_service,
        &user_service,
    );

    let event = mk_sqs_event(vec![]);

    let response = handler(&matcher, &notification_service, &sf_service, event)
        .await
        .unwrap();

    assert!(response.batch_item_failures.is_empty());
}

/// Enrichment event (e.g. ENRICHMENT_EMBEDDED) in DynamoDB stream → processed without failure.
/// No search filters → no matches, no notifications.
#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_process_enrichment_event_without_failure_when_in_stream() {
    let ddb = get_dynamodb_client().await;
    let os = get_opensearch_client().await;

    let sf_ddb_repo = UserSearchFilterDynamoDbRepositoryImpl::new(ddb, "table_1");
    let sf_os_repo = UserSearchFilterOpenSearchRepositoryImpl::new(os);
    let product_repo = ProductDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_repo = UserDynamoDbRepositoryImpl::new(ddb, "table_1");
    let user_service = UserServiceImpl::new(&user_repo);
    let get_product_service = GetProductServiceImpl::new(&product_repo);
    let enhanced_service = MockEnhancedSearchMatchService::default();

    let sf_service =
        UserSearchFilterServiceImpl::with_opensearch(&sf_ddb_repo, &user_service, &sf_os_repo);

    // Set up the product that the enrichment event references
    let shop_id = ShopId::new();
    let shops_product_id = ShopsProductId::new();
    let product_record = mk_product_record(shop_id, &shops_product_id);
    product_repo
        .put_product_records([product_record.clone()].into())
        .await
        .unwrap();

    // Notification should NOT be called (no matching filters)
    let notification_service = MockNotificationService::default();

    let matcher = ProductMatcherServiceImpl::new(
        &sf_service,
        &get_product_service,
        &enhanced_service,
        &user_service,
    );

    let mut enrichment_record: product::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord = Faker.fake();
    enrichment_record.product_id = product_record.product_id;
    enrichment_record.shop_id = shop_id;
    enrichment_record.seller_id = shop_id;
    enrichment_record.shops_product_id = shops_product_id;
    let body = mk_event_bridge_body_enrichment(&enrichment_record);
    let event = mk_sqs_event(vec![mk_sqs_message(&body)]);

    let response = handler(&matcher, &notification_service, &sf_service, event)
        .await
        .unwrap();

    assert!(
        response.batch_item_failures.is_empty(),
        "Enrichment events must be processed without failure"
    );
}
