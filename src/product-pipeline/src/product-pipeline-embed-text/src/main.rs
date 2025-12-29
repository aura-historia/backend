use aws_config::BehaviorVersion;
use aws_sdk_sqs::Client;
use product_pipeline_common::{
    flow_in::PipeFlowInImpl,
    flow_out::PipeFlowOutImpl,
    pipe::{Pipe, PipeImpl},
    types::{TextEmbeddedPipeProduct, TranslatedPipeProduct},
};
use product_pipeline_embed_text::{
    adapter::EmbeddingAdapterImpl, process::TextEmbeddingPipeProcesserImpl,
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

    let embed_text_flow_in = PipeFlowInImpl::new(&sqs, &source_queue_url);
    let embed_text_processor =
        TextEmbeddingPipeProcesserImpl::new(Arc::new(EmbeddingAdapterImpl::new().unwrap()));
    let embed_text_flow_out = PipeFlowOutImpl::new(&sqs, &target_queue_url);
    let embed_text_pipe: PipeImpl<
        '_,
        TranslatedPipeProduct,
        TranslatedPipeProduct,
        TextEmbeddedPipeProduct,
        TextEmbeddedPipeProduct,
    > = PipeImpl::new(
        &sqs,
        source_queue_url,
        256,
        900,
        &embed_text_flow_in,
        &embed_text_processor,
        &embed_text_flow_out,
    );
    embed_text_pipe.pipe().await;
}
