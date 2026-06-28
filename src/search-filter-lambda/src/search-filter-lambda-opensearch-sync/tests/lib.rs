use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
use aws_lambda_events::eventbridge::EventBridgeEvent;
use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
use common::actor::record::ActorRecord;
use common::currency::record::CurrencyRecord;
use common::language::record::LanguageRecord;
use common::resource_state::record::ResourceStateRecord;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use lambda_runtime::{Context, LambdaEvent};
use search_filter::dynamodb::user_search_filter_record::UserSearchFilterRecord;
use search_filter::opensearch::repository::{
    UserSearchFilterOpenSearchRepository, UserSearchFilterOpenSearchRepositoryImpl,
};
use search_filter::opensearch::user_search_filter_document::UserSearchFilterDocument;
use search_filter_lambda_opensearch_sync::handler;
use std::collections::HashSet;
use test_api::*;
use time::macros::datetime;

fn embedding(slot: usize) -> Vec<f32> {
    let mut embedding = vec![0.0_f32; 768];
    embedding[slot] = 1.0;
    embedding
}

fn mk_sqs_event(body: String) -> LambdaEvent<SqsEvent> {
    let mut msg = SqsMessage::default();
    msg.message_id = Some("test-message-id".to_string());
    msg.body = Some(body);
    LambdaEvent {
        payload: {
            let mut event = SqsEvent::default();
            event.records = vec![msg];
            event
        },
        context: Context::default(),
    }
}

fn mk_event_bridge_body(record: &UserSearchFilterRecord, event_name: &str) -> String {
    let new_image = serde_dynamo::to_item(record.clone()).unwrap();

    let mut stream_record = StreamRecord::default();
    stream_record.new_image = new_image;

    let mut event_record = EventRecord::default();
    event_record.event_name = event_name.to_string();
    event_record.change = stream_record;

    let mut event = EventBridgeEvent::<EventRecord>::default();
    event.detail_type = "DynamoDBStreamRecord".to_string();
    event.source = "table_1".to_string();
    event.detail = event_record;

    serde_json::to_string(&event).unwrap()
}

fn base_record() -> UserSearchFilterRecord {
    let user_id = UserId::new();
    let user_search_filter_id = UserSearchFilterId::new();

    UserSearchFilterRecord {
        pk: format!("user#{user_id}"),
        sk: format!("search_filter#{user_search_filter_id}"),
        user_id,
        user_search_filter_id,
        name: "integration filter".into(),
        enhanced_search_description: None,
        embedding: None,
        notifications: true,
        state: ResourceStateRecord::Active,
        product_query: vec!["antique lamp".try_into().unwrap()],
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
        state_query: HashSet::new(),
        created_query: None,
        updated_query: None,
        auction_start_query: None,
        auction_end_query: None,
        language: LanguageRecord::En,
        currency: CurrencyRecord::Eur,
        created_by: ActorRecord::System,
        updated_by: ActorRecord::System,
        created: datetime!(2024-01-01 00:00:00 UTC),
        updated: datetime!(2024-01-02 00:00:00 UTC),
        last_hybrid_search_matched: datetime!(2024-01-02 00:00:00 UTC),
    }
}

fn mk_record(query: &str, enhanced_description: Option<&str>) -> UserSearchFilterRecord {
    let mut record = base_record();
    record.product_query = vec![query.try_into().unwrap()];
    record.enhanced_search_description = enhanced_description.map(str::to_string);
    record
}

#[localstack_test(services = [OpenSearch()])]
async fn should_embed_query_and_enhanced_description_when_syncing_insert_for_opensearch() {
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let mut record = mk_record(
        "antique vase",
        Some("blue ceramic vase with floral pattern"),
    );
    record.embedding = Some(embedding(42));
    let event = mk_sqs_event(mk_event_bridge_body(&record, "INSERT"));

    handler(&repository, event).await.unwrap();
    refresh_index("user_search_filters").await;

    let actual: UserSearchFilterDocument =
        read_by_id("user_search_filters", record.user_search_filter_id).await;

    assert_eq!(actual.embedding, Some(embedding(42)));
}

#[localstack_test(services = [OpenSearch()])]
async fn should_sync_persisted_embedding_when_query_text_is_unchanged_for_opensearch_sync() {
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let mut record = mk_record("antique lamp", Some("brass table lamp"));
    let mut existing_document: UserSearchFilterDocument = record.clone().try_into().unwrap();
    existing_document.embedding = Some(embedding(24));
    repository.index_document(existing_document).await.unwrap();
    refresh_index("user_search_filters").await;

    record.name = "updated name".into();
    record.embedding = Some(embedding(24));
    let event = mk_sqs_event(mk_event_bridge_body(&record, "MODIFY"));

    handler(&repository, event).await.unwrap();
    refresh_index("user_search_filters").await;

    let actual: UserSearchFilterDocument =
        read_by_id("user_search_filters", record.user_search_filter_id).await;

    assert_eq!(actual.name, record.name);
    assert_eq!(actual.embedding, Some(embedding(24)));
}

#[localstack_test(services = [OpenSearch()])]
async fn should_update_embedding_when_query_text_changes_for_opensearch_sync() {
    let repository = UserSearchFilterOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let existing_record = mk_record("antique lamp", Some("brass table lamp"));
    let mut existing_document: UserSearchFilterDocument =
        existing_record.clone().try_into().unwrap();
    existing_document.embedding = Some(embedding(24));
    repository.index_document(existing_document).await.unwrap();
    refresh_index("user_search_filters").await;

    let mut changed_record = existing_record;
    changed_record.product_query = vec!["antique chandelier".try_into().unwrap()];
    changed_record.enhanced_search_description = Some("crystal ceiling light".to_string());
    changed_record.embedding = Some(embedding(91));
    let event = mk_sqs_event(mk_event_bridge_body(&changed_record, "MODIFY"));

    handler(&repository, event).await.unwrap();
    refresh_index("user_search_filters").await;

    let actual: UserSearchFilterDocument =
        read_by_id("user_search_filters", changed_record.user_search_filter_id).await;

    assert_eq!(actual.embedding, Some(embedding(91)));
}
