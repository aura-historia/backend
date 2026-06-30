use common::currency::domain::Currency;
use common::enhanced_match_reason::EnhancedMatchReason;
use common::language::document::{LanguageDocument, TextDocument};
use common::language::domain::Language;
use common::resource_state::domain::ResourceState;
use fake::{Fake, Faker};
use notification::dynamodb::repository::{
    NotificationDynamoDbRepository, NotificationDynamoDbRepositoryImpl,
};
use notification::service::noop_adapters::{NoopS3Adapter, NoopSesAdapter};
use notification::service::notification_service::NotificationServiceImpl;
use product::core::product_search::{EnhancedSearchDescription, ProductSearch};
use product::opensearch::product_document::ProductDocument;
use product::opensearch::product_state_document::ProductStateDocument;
use product::opensearch::repository::{
    ProductOpenSearchRepository, ProductOpenSearchRepositoryImpl,
};
use product::service::query_service::QueryProductServiceImpl;
use search_filter::core::user_search_filter::UserSearchFilter;
use search_filter::core::user_search_filter_name::UserSearchFilterName;
use search_filter::dynamodb::repository::{
    UserSearchFilterDynamoDbRepository, UserSearchFilterDynamoDbRepositoryImpl,
};
use search_filter::dynamodb::user_search_filter_record::UserSearchFilterRecord;
use search_filter::opensearch::repository::{
    UserSearchFilterOpenSearchRepository, UserSearchFilterOpenSearchRepositoryImpl,
};
use search_filter::opensearch::user_search_filter_document::UserSearchFilterDocument;
use search_filter::service::enhanced_search_match_service::{
    EnhancedSearchMatchResult, MockEnhancedSearchMatchService,
};
use search_filter::service::user_search_filter_service::{
    UserSearchFilterService, UserSearchFilterServiceImpl,
};
use search_filter_periodic_match::{
    DEFAULT_LLM_CONCURRENCY, PeriodicMatcherResult, PeriodicMatcherService,
    PeriodicMatcherServiceImpl,
};
use test_api::*;
use time::OffsetDateTime;
use time::macros::datetime;
use user::core::tier::UserTier;
use user::core::user::User;
use user::dynamodb::repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl};
use user::service::user_service::UserServiceImpl;

fn one_hot_embedding(slot: usize) -> Vec<f32> {
    let mut embedding = vec![0.0_f32; 768];
    embedding[slot] = 1.0;
    embedding
}

fn set_titles(doc: &mut ProductDocument, title: &str) {
    doc.title_en = Some(title.to_string());
    doc.title_native = TextDocument {
        text: title.to_string(),
        language: LanguageDocument::En,
    };
}

fn make_product_doc(title: &str, embedding: Vec<f32>, updated: OffsetDateTime) -> ProductDocument {
    let mut doc: ProductDocument = Faker.fake();
    set_titles(&mut doc, title);
    doc.embedding = Some(embedding);
    doc.state = ProductStateDocument::Available;
    doc.created = updated;
    doc.updated = updated;
    doc
}

fn make_search_filter(user: &User, embedding: Vec<f32>) -> UserSearchFilter {
    let last_matched = datetime!(2026-05-10 12:00 UTC);
    let mut filter: UserSearchFilter = Faker.fake();
    filter.user_id = user.user_id;
    filter.name = UserSearchFilterName::from("Periodic porcelain alerts");
    filter.state = ResourceState::Active;
    filter.search = ProductSearch::new(Language::En, Currency::Eur)
        .with_product_query("rare porcelain vase".try_into().unwrap())
        .with_enhanced_search_description(EnhancedSearchDescription::from(
            "blue floral porcelain vase",
        ));
    filter.created = last_matched;
    filter.updated = last_matched;
    filter.last_hybrid_search_matched = last_matched;
    filter.embedding = Some(embedding);
    filter
}

#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_persist_match_and_notification_when_periodic_matcher_uses_stored_embedding() {
    let user_repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let search_filter_dynamodb_repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let search_filter_opensearch_repository =
        UserSearchFilterOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let search_filter_service = UserSearchFilterServiceImpl::with_opensearch(
        &search_filter_dynamodb_repository,
        &user_service,
        &search_filter_opensearch_repository,
    );
    let product_opensearch_repository =
        ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_product_service = QueryProductServiceImpl::new(&product_opensearch_repository);
    let notification_repository =
        NotificationDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let noop_ses_adapter = NoopSesAdapter;
    let noop_s3_adapter = NoopS3Adapter;
    let notification_service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &noop_ses_adapter,
        &noop_s3_adapter,
        "test-bucket",
        "test-stage",
        "test-sha",
    );

    let mut user: User = Faker.fake();
    user.tier = UserTier::Ultimate;
    user.language = Some(Language::En);
    user.currency = Some(Currency::Eur);
    user_repository
        .put_user_record(user.clone().into())
        .await
        .unwrap();

    let query_embedding = one_hot_embedding(42);
    let filter = make_search_filter(&user, query_embedding.clone());
    let filter_record = UserSearchFilterRecord::from(filter.clone());
    search_filter_dynamodb_repository
        .put_user_search_filter_record(filter_record.clone())
        .await
        .unwrap();

    let mut filter_document: UserSearchFilterDocument = filter_record.try_into().unwrap();
    filter_document.embedding = Some(query_embedding.clone());
    search_filter_opensearch_repository
        .index_document(filter_document)
        .await
        .unwrap();
    refresh_index("user_search_filters").await;

    let product = make_product_doc(
        "Rare porcelain vase with blue floral decoration",
        query_embedding,
        filter.last_hybrid_search_matched + time::Duration::days(1),
    );
    product_opensearch_repository
        .create_product_documents(vec![product.clone()])
        .await
        .unwrap();
    refresh_index("products").await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let expected_reason = EnhancedMatchReason::from("It is the requested blue porcelain vase.");
    let mut enhanced_search_match_service = MockEnhancedSearchMatchService::default();
    let expected_reason_for_llm = expected_reason.clone();
    enhanced_search_match_service
        .expect_evaluate()
        .times(1)
        .return_once(move |description, _, _, language, _| {
            assert_eq!(description.as_ref(), "blue floral porcelain vase");
            assert_eq!(language, Language::En);
            Box::pin(async move {
                Ok(EnhancedSearchMatchResult {
                    matches: true,
                    reason: Some(expected_reason_for_llm),
                })
            })
        });

    let matcher = PeriodicMatcherServiceImpl::new(
        &search_filter_service,
        &query_product_service,
        &enhanced_search_match_service,
        &notification_service,
        &user_service,
        DEFAULT_LLM_CONCURRENCY,
    );

    let result = matcher.match_active_filters().await.unwrap();

    assert_eq!(
        result,
        PeriodicMatcherResult {
            filters_processed: 1,
            matches_created: 1,
            notifications_created: 1,
            filters_failed: 0,
        }
    );

    let persisted_match = search_filter_dynamodb_repository
        .get_user_search_filter_match_record(
            &filter.user_id,
            &filter.user_search_filter_id,
            &product.shop_id,
            &product.shops_product_id,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(persisted_match.product_id, product.product_id);
    assert_eq!(
        persisted_match.enhanced_match_reason.as_deref(),
        Some(expected_reason.as_ref())
    );

    let persisted_notification = notification_repository
        .get_notification_record(&filter.user_id, &product.event_id)
        .await
        .unwrap();
    assert!(persisted_notification.is_some());

    let updated_filter = search_filter_service
        .find_user_search_filter(&filter.user_id, &filter.user_search_filter_id)
        .await
        .unwrap();
    assert!(updated_filter.last_hybrid_search_matched > filter.last_hybrid_search_matched);
}

#[localstack_test(services = [DynamoDB(), OpenSearch()])]
async fn should_not_create_match_when_periodic_matcher_filter_excludes_product_shop_name() {
    let user_repository = UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let search_filter_dynamodb_repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let search_filter_opensearch_repository =
        UserSearchFilterOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let search_filter_service = UserSearchFilterServiceImpl::with_opensearch(
        &search_filter_dynamodb_repository,
        &user_service,
        &search_filter_opensearch_repository,
    );
    let product_opensearch_repository =
        ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let query_product_service = QueryProductServiceImpl::new(&product_opensearch_repository);
    let notification_repository =
        NotificationDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let noop_ses_adapter = NoopSesAdapter;
    let noop_s3_adapter = NoopS3Adapter;
    let notification_service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &noop_ses_adapter,
        &noop_s3_adapter,
        "test-bucket",
        "test-stage",
        "test-sha",
    );

    let mut user: User = Faker.fake();
    user.tier = UserTier::Ultimate;
    user.language = Some(Language::En);
    user.currency = Some(Currency::Eur);
    user_repository
        .put_user_record(user.clone().into())
        .await
        .unwrap();

    let query_embedding = one_hot_embedding(84);
    let product = make_product_doc(
        "Rare porcelain vase with blue floral decoration",
        query_embedding.clone(),
        datetime!(2026-05-11 12:00 UTC),
    );
    product_opensearch_repository
        .create_product_documents(vec![product.clone()])
        .await
        .unwrap();
    refresh_index("products").await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let mut filter = make_search_filter(&user, query_embedding.clone());
    filter.search = filter.search.with_exclude_shop_name_query(
        std::collections::HashSet::from_iter([product.shop_name.clone().into()]).into(),
    );
    let filter_record = UserSearchFilterRecord::from(filter.clone());
    search_filter_dynamodb_repository
        .put_user_search_filter_record(filter_record.clone())
        .await
        .unwrap();

    let mut filter_document: UserSearchFilterDocument = filter_record.try_into().unwrap();
    filter_document.embedding = Some(query_embedding);
    search_filter_opensearch_repository
        .index_document(filter_document)
        .await
        .unwrap();
    refresh_index("user_search_filters").await;

    let enhanced_search_match_service = MockEnhancedSearchMatchService::default();

    let matcher = PeriodicMatcherServiceImpl::new(
        &search_filter_service,
        &query_product_service,
        &enhanced_search_match_service,
        &notification_service,
        &user_service,
        DEFAULT_LLM_CONCURRENCY,
    );

    let result = matcher.match_active_filters().await.unwrap();

    assert_eq!(
        result,
        PeriodicMatcherResult {
            filters_processed: 1,
            matches_created: 0,
            notifications_created: 0,
            filters_failed: 0,
        }
    );

    let persisted_match = search_filter_dynamodb_repository
        .get_user_search_filter_match_record(
            &filter.user_id,
            &filter.user_search_filter_id,
            &product.shop_id,
            &product.shops_product_id,
        )
        .await
        .unwrap();
    assert!(persisted_match.is_none());

    let persisted_notification = notification_repository
        .get_notification_record(&filter.user_id, &product.event_id)
        .await
        .unwrap();
    assert!(persisted_notification.is_none());
}
