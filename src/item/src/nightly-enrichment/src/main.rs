use aws_config::{BehaviorVersion, SdkConfig};
use item_dynamodb::repository::ItemDynamoDbRepositoryImpl;
use item_opensearch::repository::ItemOpenSearchRepositoryImpl;
use nightly_enrichment::{
    embed::EmbeddingDelegateImpl,
    pipeline::{
        embed::EmbeddingEnrichmentPipeImpl,
        faucet::EnrichmentPipeFaucetImpl,
        pipe::EnrichmentPipe,
        plumbing::{EnrichmentPlumbing, EnrichmentPlumbingImpl},
        sink::EnrichmentPipeSinkImpl,
    },
};
use nightly_enrichment_asg_scale_down::scale_down;
use opensearch::http::{
    Url,
    transport::{SingleNodeConnectionPool, TransportBuilder},
};
use std::sync::Arc;
use tokio::sync::OnceCell;
use tracing::info;

static AWS_CONFIG: OnceCell<SdkConfig> = OnceCell::const_new();
pub async fn get_aws_config() -> &'static SdkConfig {
    info!("Loading AWS-Config...");
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
    info!("Loading DynamoDB-Client...");
    DYNAMODB_CLIENT
        .get_or_init(|| async { aws_sdk_dynamodb::Client::new(get_aws_config().await) })
        .await
}

static SQS_CLIENT: OnceCell<aws_sdk_sqs::Client> = OnceCell::const_new();
pub async fn get_sqs_client() -> &'static aws_sdk_sqs::Client {
    info!("Loading SQS-Client...");
    SQS_CLIENT
        .get_or_init(|| async { aws_sdk_sqs::Client::new(get_aws_config().await) })
        .await
}

static OPENSEARCH_CLIENT: OnceCell<opensearch::OpenSearch> = OnceCell::const_new();
pub async fn get_opensearch_client() -> &'static opensearch::OpenSearch {
    info!("Loading OpenSearch-Client...");
    OPENSEARCH_CLIENT
        .get_or_init(|| async {
            let domain_endpoint = std::env::var("OPENSEARCH_DOMAIN_ENDPOINT_URL")
                .expect("shoudln't fail reading env-var 'OPENSEARCH_DOMAIN_ENDPOINT_URL'");
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
        .expect("shoudln't fail reading env-var 'DYNAMODB_TABLE_NAME'");
    let item_dynamodb_repository =
        ItemDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &table_name);

    let enrichment_queue_url = std::env::var("ENRICHMENT_QUEUE_URL")
        .expect("shoudln't fail reading env-var 'ENRICHMENT_QUEUE_URL'");
    let sqs_client_arc = Arc::new(get_sqs_client().await.clone());

    let item_opensearch_repository =
        ItemOpenSearchRepositoryImpl::new(get_opensearch_client().await);

    let faucet =
        EnrichmentPipeFaucetImpl::new(sqs_client_arc.clone(), enrichment_queue_url.clone());
    let embedding_delegate = EmbeddingDelegateImpl::new().unwrap();
    let embedding_pipe = EmbeddingEnrichmentPipeImpl::new(Arc::new(embedding_delegate));
    let pipes: Vec<Box<dyn EnrichmentPipe + Send + Sync>> = vec![Box::new(embedding_pipe)];
    let sink = EnrichmentPipeSinkImpl::new(
        Arc::new(item_dynamodb_repository),
        Arc::new(item_opensearch_repository),
    );

    let plumbing = EnrichmentPlumbingImpl::new(
        Arc::new(faucet),
        pipes,
        Arc::new(sink),
        sqs_client_arc,
        enrichment_queue_url,
    );

    loop {
        let res = plumbing.plumb(1000).await;
        let total = res.failures + res.successes;

        if total == 0 {
            break;
        }
    }

    let autoscaling_client = aws_sdk_autoscaling::Client::new(get_aws_config().await);
    let asg_name = std::env::var("NIGHTLY_ENRICHMENT_ASG_NAME")
        .expect("shoudln't fail reading env-var 'NIGHTLY_ENRICHMENT_ASG_NAME'");
    scale_down(
        &autoscaling_client,
        get_opensearch_client().await,
        &asg_name,
    )
    .await
    .unwrap();
}
