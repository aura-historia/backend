use aws_config::BehaviorVersion;
use aws_sdk_sqs::Client;
use product_pipeline_common::{
    flow_in::PipeFlowInImpl,
    flow_out::PipeFlowOutImpl,
    pipe::{Pipe, PipeImpl},
    types::{AttributeExtractedPipeProduct, TextEmbeddedPipeProduct},
};
use product_pipeline_extract_attribute::{
    adapter::ExtractionAdapterImpl, process::AttributeExtractionPipeProcesserImpl,
};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_current_span(true)
        .with_ansi(false)
        .without_time()
        .init();

    let aws_config = aws_config::defaults(BehaviorVersion::v2025_08_07())
        .load()
        .await;
    let sqs = Client::new(&aws_config);
    let source_queue_url = std::env::var("SOURCE_QUEUE_URL")
        .expect("shouldn't fail reading env-var 'SOURCE_QUEUE_URL'");
    let target_queue_url = std::env::var("TARGET_QUEUE_URL")
        .expect("shouldn't fail reading env-var 'TARGET_QUEUE_URL'");

    let extract_attribute_flow_in = PipeFlowInImpl::new(&sqs, &source_queue_url);
    let extract_attribute_processor =
        AttributeExtractionPipeProcesserImpl::new(Arc::new(ExtractionAdapterImpl::new().unwrap()));
    let extract_attribute_flow_out = PipeFlowOutImpl::new(&sqs, &target_queue_url);
    let extract_attribute_pipe: PipeImpl<
        '_,
        TextEmbeddedPipeProduct,
        TextEmbeddedPipeProduct,
        AttributeExtractedPipeProduct,
        AttributeExtractedPipeProduct,
    > = PipeImpl::new(
        &sqs,
        source_queue_url,
        32,
        900,
        &extract_attribute_flow_in,
        &extract_attribute_processor,
        &extract_attribute_flow_out,
    );
    extract_attribute_pipe.pipe().await;
}
