use aws_config::{BehaviorVersion, SdkConfig};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::opensearch::repository::ProductOpenSearchRepositoryImpl;
use product_enrichment::{
    embed::EmbeddingDelegateImpl,
    pipeline::{
        embed::EmbeddingEnrichmentPipeImpl,
        faucet::EnrichmentPipeFaucetImpl,
        pipe::EnrichmentPipe,
        plumbing::{EnrichmentPlumbing, EnrichmentPlumbingImpl},
        sink::EnrichmentPipeSinkImpl,
        translate::TranslationEnrichmentPipeImpl,
    },
    translate::TranslationDelegateImpl,
};
use product_enrichment_asg_scale_down::reinstantiate_periodic_opensearch_index_products_refresh;
use std::sync::Arc;
use tokio::sync::OnceCell;
use tracing::{error, info};

static AWS_CONFIG: OnceCell<SdkConfig> = OnceCell::const_new();
pub async fn get_aws_config() -> &'static SdkConfig {
    AWS_CONFIG
        .get_or_init(|| async {
            aws_config::defaults(BehaviorVersion::v2025_08_07())
                .load()
                .await
        })
        .await
}

static DYNAMODB_CLIENT: OnceCell<aws_sdk_dynamodb::Client> = OnceCell::const_new();
pub async fn get_dynamodb_client() -> &'static aws_sdk_dynamodb::Client {
    DYNAMODB_CLIENT
        .get_or_init(|| async { aws_sdk_dynamodb::Client::new(get_aws_config().await) })
        .await
}

static SQS_CLIENT: OnceCell<aws_sdk_sqs::Client> = OnceCell::const_new();
pub async fn get_sqs_client() -> &'static aws_sdk_sqs::Client {
    SQS_CLIENT
        .get_or_init(|| async { aws_sdk_sqs::Client::new(get_aws_config().await) })
        .await
}

static OPENSEARCH_CLIENT: OnceCell<opensearch::OpenSearch> = OnceCell::const_new();
pub async fn get_opensearch_client() -> &'static opensearch::OpenSearch {
    OPENSEARCH_CLIENT
        .get_or_init(|| async {
            common::opensearch::client::load_client()
                .await
                .expect("shouldn't fail loading OpenSearch-Client")
        })
        .await
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_current_span(true)
        .with_ansi(false)
        .init();

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail reading env-var 'DYNAMODB_TABLE_NAME'");
    let product_dynamodb_repository =
        ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &table_name);

    let enrichment_queue_url = std::env::var("ENRICHMENT_QUEUE_URL")
        .expect("shouldn't fail reading env-var 'ENRICHMENT_QUEUE_URL'");
    let sqs_client_arc = Arc::new(get_sqs_client().await.clone());

    let product_opensearch_repository =
        ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let faucet =
        EnrichmentPipeFaucetImpl::new(sqs_client_arc.clone(), enrichment_queue_url.clone());

    let embedding_delegate =
        EmbeddingDelegateImpl::new().expect("shouldn't fail creating 'EmbeddingDelegateImpl'");
    let embedding_pipe = EmbeddingEnrichmentPipeImpl::new(Arc::new(embedding_delegate));

    let translation_delegate =
        TranslationDelegateImpl::new().expect("shouldn't fail creating 'TranslationDelegateImpl'");
    let translation_pipe = TranslationEnrichmentPipeImpl::new(Arc::new(translation_delegate));

    let pipes: Vec<Box<dyn EnrichmentPipe + Send + Sync>> =
        vec![Box::new(embedding_pipe), Box::new(translation_pipe)];

    let sink = EnrichmentPipeSinkImpl::new(
        Arc::new(product_dynamodb_repository),
        Arc::new(product_opensearch_repository),
    );

    let plumbing = EnrichmentPlumbingImpl::new(
        Arc::new(faucet),
        pipes,
        Arc::new(sink),
        sqs_client_arc,
        enrichment_queue_url,
    );

    info!("Initialization complete.");
    loop {
        let res = plumbing.plumb(1000).await;
        let total = res.failures + res.successes;

        if total == 0 {
            break;
        }
    }

    reinstantiate_periodic_opensearch_index_products_refresh(get_opensearch_client().await).await.unwrap_or_else(|err|
        error!(error = ?err, "Failed reinstantiating periodic opensearch refresh-index for index 'products'.")
    );
}
