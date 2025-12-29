use aws_config::BehaviorVersion;
use aws_sdk_sqs::Client;
use product_pipeline_common::{
    flow_in::PipeFlowInImpl,
    flow_out::PipeFlowOutImpl,
    pipe::{Pipe, PipeImpl},
    types::{CleansedPipeProduct, TranslatedPipeProduct},
};
use product_pipeline_translate::{
    adapter::TranslationAdapterImpl, process::TranslationPipeProcesserImpl,
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

    let translate_flow_in = PipeFlowInImpl::new(&sqs, &source_queue_url);
    let translate_processor =
        TranslationPipeProcesserImpl::new(Arc::new(TranslationAdapterImpl::new().unwrap()));
    let translate_flow_out = PipeFlowOutImpl::new(&sqs, &target_queue_url);
    let translate_pipe: PipeImpl<
        '_,
        CleansedPipeProduct,
        CleansedPipeProduct,
        TranslatedPipeProduct,
        TranslatedPipeProduct,
    > = PipeImpl::new(
        &sqs,
        source_queue_url,
        256,
        600,
        &translate_flow_in,
        &translate_processor,
        &translate_flow_out,
    );
    translate_pipe.pipe().await;
}
