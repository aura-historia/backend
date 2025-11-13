use aws_config::{BehaviorVersion, SdkConfig};
use opensearch::http::{
    Url,
    transport::{SingleNodeConnectionPool, TransportBuilder},
};
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
    },
};
use std::sync::Arc;
use tokio::sync::OnceCell;
use tracing::info;

static AWS_CONFIG: OnceCell<SdkConfig> = OnceCell::const_new();
pub async fn get_aws_config() -> &'static SdkConfig {
    AWS_CONFIG
        .get_or_init(|| async {
            info!("Loading AWS-Config...");
            aws_config::defaults(BehaviorVersion::v2025_08_07())
                .load()
                .await
        })
        .await
}

static DYNAMODB_CLIENT: OnceCell<aws_sdk_dynamodb::Client> = OnceCell::const_new();
pub async fn get_dynamodb_client() -> &'static aws_sdk_dynamodb::Client {
    DYNAMODB_CLIENT
        .get_or_init(|| async {
            info!("Loading DynamoDB-Client...");
            aws_sdk_dynamodb::Client::new(get_aws_config().await)
        })
        .await
}

static SQS_CLIENT: OnceCell<aws_sdk_sqs::Client> = OnceCell::const_new();
pub async fn get_sqs_client() -> &'static aws_sdk_sqs::Client {
    SQS_CLIENT
        .get_or_init(|| async {
            info!("Loading SQS-Client...");
            aws_sdk_sqs::Client::new(get_aws_config().await)
        })
        .await
}

static OPENSEARCH_CLIENT: OnceCell<opensearch::OpenSearch> = OnceCell::const_new();
pub async fn get_opensearch_client() -> &'static opensearch::OpenSearch {
    OPENSEARCH_CLIENT
        .get_or_init(|| async {
            info!("Loading OpenSearch-Client...");
            let domain_endpoint = std::env::var("OPENSEARCH_DOMAIN_ENDPOINT_URL")
                .expect("shouldn't fail reading env-var 'OPENSEARCH_DOMAIN_ENDPOINT_URL'");
            info!(
                url = domain_endpoint,
                "Loaded OpenSearch Domain-Endpoint-Url."
            );
            let domain_endpoint_url = Url::parse(&domain_endpoint).expect(
                "shouldn't fail parsing value for env-var 'OPENSEARCH_DOMAIN_ENDPOINT_URL' as url",
            );
            let transport =
                TransportBuilder::new(SingleNodeConnectionPool::new(domain_endpoint_url))
                    .auth(
                        get_aws_config()
                            .await
                            .clone()
                            .try_into()
                            .expect("shouldn't fail extracting AWS-Config for OpenSearch"),
                    )
                    .service_name("es")
                    .build()
                    .expect("shouldn't fail building OpenSearch-Transport");
            opensearch::OpenSearch::new(transport)
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
    info!(tableName = table_name, "Loaded DynamoDb table-name.");
    let product_dynamodb_repository =
        ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &table_name);
    info!(type = %std::any::type_name::<ProductDynamoDbRepositoryImpl>(), "Loaded component.");

    let enrichment_queue_url = std::env::var("ENRICHMENT_QUEUE_URL")
        .expect("shouldn't fail reading env-var 'ENRICHMENT_QUEUE_URL'");
    info!(
        queueUrl = enrichment_queue_url,
        "Loaded EnrichmentQueue-Url."
    );
    let sqs_client_arc = Arc::new(get_sqs_client().await.clone());

    let product_opensearch_repository =
        ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    info!(type = %std::any::type_name::<ProductOpenSearchRepositoryImpl>(), "Loaded component.");

    let faucet =
        EnrichmentPipeFaucetImpl::new(sqs_client_arc.clone(), enrichment_queue_url.clone());
    info!(type = %std::any::type_name::<EnrichmentPipeFaucetImpl>(), "Loaded component.");

    let embedding_delegate = EmbeddingDelegateImpl::new().unwrap();
    info!(type = %std::any::type_name::<EmbeddingDelegateImpl>(), "Loaded component.");

    let embedding_pipe = EmbeddingEnrichmentPipeImpl::new(Arc::new(embedding_delegate));
    info!(type = %std::any::type_name::<EmbeddingEnrichmentPipeImpl>(), "Loaded component.");
    let pipes: Vec<Box<dyn EnrichmentPipe + Send + Sync>> = vec![Box::new(embedding_pipe)];

    let sink = EnrichmentPipeSinkImpl::new(
        Arc::new(product_dynamodb_repository),
        Arc::new(product_opensearch_repository),
    );
    info!(type = %std::any::type_name::<EnrichmentPipeSinkImpl>(), "Loaded component.");

    let plumbing = EnrichmentPlumbingImpl::new(
        Arc::new(faucet),
        pipes,
        Arc::new(sink),
        sqs_client_arc,
        enrichment_queue_url,
    );
    info!(type = %std::any::type_name::<EnrichmentPlumbingImpl>(), "Loaded component.");

    info!("Initialization complete.");
    loop {
        let res = plumbing.plumb(1000).await;
        let total = res.failures + res.successes;

        if total == 0 {
            break;
        }
    }
}
