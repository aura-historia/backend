use aws_config::BehaviorVersion;
use aws_sdk_sqs::Client;
use product::dynamodb::product_event_record::ProductEventRecord;
use product_pipeline_common::{
    flow_out::PipeFlowOutImpl,
    pipe::{Pipe, PipeImpl},
    types::InitialPipeProduct,
};
use product_pipeline_init::{
    flow_in::EventBridgeSqsDynamoDbStreamProductEventRecordPipeFlowInImpl,
    process::InitPipeProcessorImpl,
};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_current_span(true)
        .with_ansi(false)
        .without_time()
        .init();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;
    let sqs = Client::new(&aws_config);
    let source_queue_url = std::env::var("SOURCE_QUEUE_URL")
        .expect("shouldn't fail reading env-var 'SOURCE_QUEUE_URL'");
    let target_queue_url = std::env::var("TARGET_QUEUE_URL")
        .expect("shouldn't fail reading env-var 'TARGET_QUEUE_URL'");

    let init_flow_in =
        EventBridgeSqsDynamoDbStreamProductEventRecordPipeFlowInImpl::new(&sqs, &source_queue_url);
    let init_processor = InitPipeProcessorImpl();
    let init_flow_out = PipeFlowOutImpl::new(&sqs, &target_queue_url);
    let init_pipe: PipeImpl<
        '_,
        ProductEventRecord,
        ProductEventRecord,
        InitialPipeProduct,
        InitialPipeProduct,
    > = PipeImpl::new(
        &sqs,
        source_queue_url,
        256,
        300,
        &init_flow_in,
        &init_processor,
        &init_flow_out,
    );
    init_pipe.pipe().await;
}
