use aws_lambda_events::{
    dynamodb::{EventRecord, StreamRecord},
    eventbridge::EventBridgeEvent,
};
use common::year::Year;
use fake::{Fake, Faker};
use product::{
    core::product_event::ProductCreatedEventPayload,
    dynamodb::{
        product_event_record::ProductEventRecord, repository::ProductDynamoDbRepositoryImpl,
    },
    opensearch::repository::ProductOpenSearchRepositoryImpl,
};
use product::{
    dynamodb::repository::ProductDynamoDbRepository,
    opensearch::repository::ProductOpenSearchRepository,
};
use product_pipeline_common::{
    flow_in::PipeFlowInImpl,
    flow_out::PipeFlowOutImpl,
    pipe::{Pipe, PipeImpl},
    types::{
        AttributeExtractedPipeProduct, CleansedPipeProduct, CompletedPipeProduct,
        InitialPipeProduct, TextEmbeddedPipeProduct, TranslatedPipeProduct,
    },
};
use product_pipeline_complete::{
    flow_out::PersistDynamoDbOpenSearchPipeFlowOutImpl, process::CompleterPipeProcessorImpl,
};
use product_pipeline_embed_text::{
    adapter::EmbeddingAdapterImpl, process::TextEmbeddingPipeProcesserImpl,
};
use product_pipeline_extract_attribute::{
    adapter::MockExtractionAdapter, process::AttributeExtractionPipeProcesserImpl,
};
use product_pipeline_init::{
    flow_in::EventBridgeSqsDynamoDbStreamProductEventRecordPipeFlowInImpl,
    process::InitPipeProcessorImpl,
};
use product_pipeline_translate::{
    adapter::TranslationAdapterImpl, process::TranslationPipeProcesserImpl,
};
use std::{sync::Arc, time::SystemTime};
use test_api::*;
use uuid::Uuid;

const INIT_Q: Sqs = Sqs { name: "init-queue" };
const TRANSLATE_Q: Sqs = Sqs {
    name: "translate-queue",
};
const EMBED_TEXT_Q: Sqs = Sqs {
    name: "embed-text-queue",
};
const EXTRACT_ATTRIBUTE_Q: Sqs = Sqs {
    name: "extract-attribute-queue",
};
const COMPLETE_Q: Sqs = Sqs {
    name: "complete-queue",
};
use common::{event::Event, event_id::EventId, product_id::ProductId};
use product::core::product_event::ProductEventPayload;
use time::OffsetDateTime;

fn mk_event_bridge_payload(product_event_record: &ProductEventRecord) -> String {
    let mut stream_record = StreamRecord::default();
    stream_record.approximate_creation_date_time = SystemTime::now().into();
    stream_record.new_image = serde_dynamo::to_item(product_event_record).unwrap();
    stream_record.size_bytes = 42;

    let mut event_record = EventRecord::default();
    event_record.aws_region = "eu-central-1".to_string();
    event_record.change = stream_record;
    event_record.event_id = Uuid::new_v4().to_string();
    event_record.event_name = "INSERT".to_string();

    let mut event = EventBridgeEvent::<EventRecord>::default();
    event.detail_type = "foo".to_string();
    event.source = "bar".to_string();
    event.detail = event_record;

    serde_json::to_string(&event).unwrap()
}

#[ignore]
#[rstest::rstest]
#[test_attr(apply(test))]
#[case(1)]
#[case(10)]
#[case(16)]
#[case(21)]
#[trace]
#[localstack_test(services = [INIT_Q, TRANSLATE_Q, EMBED_TEXT_Q, EXTRACT_ATTRIBUTE_Q, COMPLETE_Q, DynamoDB(), OpenSearch()])]
async fn should_flow_through_entire_pipeline(#[case] count: usize) {
    let sqs = get_sqs_client().await;
    let dynamodb = get_dynamodb_client().await;
    let opensearch = get_opensearch_client().await;
    let product_dynamodb_repository = ProductDynamoDbRepositoryImpl::new(dynamodb, "table_1");
    let product_opensearch_repository = ProductOpenSearchRepositoryImpl::new(opensearch);

    // Prepare system-state
    let event_records = fake::vec![ProductCreatedEventPayload; count]
        .into_iter()
        .map(|mut event_payload| {
            event_payload.native_description = Some(Faker.fake());
            event_payload
        })
        .map(|payload| Event {
            aggregate_id: ProductId::new(),
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductEventPayload::Created(payload),
        })
        .map(|event| ProductEventRecord::try_from(event).unwrap());
    for event_record in event_records.clone() {
        let payload = mk_event_bridge_payload(&event_record);
        let _ = sqs
            .send_message()
            .queue_url(INIT_Q.queue_url())
            .message_body(payload)
            .delay_seconds(0)
            .send()
            .await
            .unwrap();

        // Simulate existing materialized views
        let ddb_put_res = product_dynamodb_repository
            .put_product_records([event_record.clone().try_into().unwrap()].into())
            .await
            .unwrap();
        assert!(ddb_put_res.unprocessed_items.unwrap_or_default().is_empty());
        let os_create_res = product_opensearch_repository
            .create_product_documents(vec![event_record.clone().try_into().unwrap()])
            .await
            .unwrap();
        assert!(!os_create_res.errors);
    }
    refresh_index("products").await;

    // Init-Pipe
    let init_flow_in =
        EventBridgeSqsDynamoDbStreamProductEventRecordPipeFlowInImpl::new(sqs, INIT_Q.queue_url());
    let init_processor = InitPipeProcessorImpl();
    let init_flow_out = PipeFlowOutImpl::new(sqs, TRANSLATE_Q.queue_url());
    let init_pipe: PipeImpl<
        '_,
        ProductEventRecord,
        ProductEventRecord,
        InitialPipeProduct,
        InitialPipeProduct,
    > = PipeImpl::new(
        sqs,
        INIT_Q.queue_url(),
        16,
        300,
        &init_flow_in,
        &init_processor,
        &init_flow_out,
    );
    init_pipe.pipe().await;

    // Translate-Pipe
    let translate_flow_in = PipeFlowInImpl::new(sqs, TRANSLATE_Q.queue_url());
    let translate_processor =
        TranslationPipeProcesserImpl::new(Arc::new(TranslationAdapterImpl::new().unwrap()));
    let translate_flow_out = PipeFlowOutImpl::new(sqs, EMBED_TEXT_Q.queue_url());
    let translate_pipe: PipeImpl<
        '_,
        CleansedPipeProduct,
        CleansedPipeProduct,
        TranslatedPipeProduct,
        TranslatedPipeProduct,
    > = PipeImpl::new(
        sqs,
        TRANSLATE_Q.queue_url(),
        16,
        300,
        &translate_flow_in,
        &translate_processor,
        &translate_flow_out,
    );
    translate_pipe.pipe().await;

    // Embed-Text-Pipe
    let embed_text_flow_in = PipeFlowInImpl::new(sqs, EMBED_TEXT_Q.queue_url());
    let embed_text_processor =
        TextEmbeddingPipeProcesserImpl::new(Arc::new(EmbeddingAdapterImpl::new().unwrap()));
    let embed_text_flow_out = PipeFlowOutImpl::new(sqs, EXTRACT_ATTRIBUTE_Q.queue_url());
    let embed_text_pipe: PipeImpl<
        '_,
        TranslatedPipeProduct,
        TranslatedPipeProduct,
        TextEmbeddedPipeProduct,
        TextEmbeddedPipeProduct,
    > = PipeImpl::new(
        sqs,
        EMBED_TEXT_Q.queue_url(),
        16,
        300,
        &embed_text_flow_in,
        &embed_text_processor,
        &embed_text_flow_out,
    );
    embed_text_pipe.pipe().await;

    // Extract-Attribute-Pipe
    let extract_attribute_flow_in = PipeFlowInImpl::new(sqs, EXTRACT_ATTRIBUTE_Q.queue_url());
    let mut extract_attribute_adapter_mock = MockExtractionAdapter::default();
    extract_attribute_adapter_mock
        .expect_extract()
        .returning(|_, _| {
            Ok((0..7)
                .map(|_| r#"{"originYear":1837}"#.to_owned())
                .collect::<Vec<_>>()
                .try_into()
                .unwrap())
        });
    let extract_attribute_processor =
        AttributeExtractionPipeProcesserImpl::new(Arc::new(extract_attribute_adapter_mock));
    let extract_attribute_flow_out = PipeFlowOutImpl::new(sqs, COMPLETE_Q.queue_url());
    let extract_attribute_pipe: PipeImpl<
        '_,
        TextEmbeddedPipeProduct,
        TextEmbeddedPipeProduct,
        AttributeExtractedPipeProduct,
        AttributeExtractedPipeProduct,
    > = PipeImpl::new(
        sqs,
        EMBED_TEXT_Q.queue_url(),
        16,
        300,
        &extract_attribute_flow_in,
        &extract_attribute_processor,
        &extract_attribute_flow_out,
    );
    extract_attribute_pipe.pipe().await;

    // Complete-Pipe
    let complete_flow_in = PipeFlowInImpl::new(sqs, COMPLETE_Q.queue_url());
    let complete_processor = CompleterPipeProcessorImpl();
    let complete_flow_out = PersistDynamoDbOpenSearchPipeFlowOutImpl::new(
        &product_dynamodb_repository,
        &product_opensearch_repository,
    );
    let complete_pipe: PipeImpl<
        '_,
        AttributeExtractedPipeProduct,
        AttributeExtractedPipeProduct,
        CompletedPipeProduct,
        CompletedPipeProduct,
    > = PipeImpl::new(
        sqs,
        COMPLETE_Q.queue_url(),
        16,
        300,
        &complete_flow_in,
        &complete_processor,
        &complete_flow_out,
    );
    complete_pipe.pipe().await;

    for event_record in event_records {
        let materialized_record_dynamodb = product_dynamodb_repository
            .get_product_record(&event_record.shop_id, &event_record.shops_product_id)
            .await
            .unwrap()
            .unwrap();
        assert!(materialized_record_dynamodb.title_de.is_some());
        assert!(materialized_record_dynamodb.title_en.is_some());
        assert!(materialized_record_dynamodb.title_fr.is_some());
        assert!(materialized_record_dynamodb.title_es.is_some());
        if materialized_record_dynamodb.description_native.is_some() {
            assert!(materialized_record_dynamodb.description_de.is_some());
            assert!(materialized_record_dynamodb.description_en.is_some());
            assert!(materialized_record_dynamodb.description_fr.is_some());
            assert!(materialized_record_dynamodb.description_es.is_some());
        }
        assert_eq!(
            Year::from(1837),
            materialized_record_dynamodb.origin_year.unwrap()
        );
    }
}
