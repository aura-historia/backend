use aws_config::BehaviorVersion;
use opensearch::{
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, Transport, TransportBuilder},
};
use url::Url;

pub async fn load_client() -> Result<opensearch::OpenSearch, lambda_runtime::Error> {
    Ok(opensearch::OpenSearch::new(load_transport().await?))
}

pub async fn load_transport() -> Result<Transport, lambda_runtime::Error> {
    let endpoint_url = Url::parse(&std::env::var("OPENSEARCH_ENDPOINT_URL")?)?;
    let stage = std::env::var("STAGE").unwrap_or_else(|_| "prod".into());

    let transport = match stage.as_str() {
        "ephemeral" => {
            let aws_config = aws_config::defaults(BehaviorVersion::v2025_08_07())
                .load()
                .await;
            TransportBuilder::new(SingleNodeConnectionPool::new(endpoint_url))
                .auth(aws_config.try_into()?)
                .service_name("es")
                .build()?
        }
        _ => {
            let os_username = std::env::var("OPENSEARCH_USERNAME")?;
            let os_password = std::env::var("OPENSEARCH_PASSWORD")?;
            TransportBuilder::new(SingleNodeConnectionPool::new(endpoint_url))
                .auth(Credentials::Basic(os_username, os_password))
                .build()?
        }
    };

    Ok(transport)
}
