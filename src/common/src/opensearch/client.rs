use opensearch::{
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, Transport, TransportBuilder},
};
use url::Url;

pub async fn load_client() -> Result<opensearch::OpenSearch, lambda_runtime::Error> {
    Ok(opensearch::OpenSearch::new(load_transport().await?))
}

pub async fn load_transport() -> Result<Transport, lambda_runtime::Error> {
    let endpoint_url_str = std::env::var("OPENSEARCH_ENDPOINT_URL")?;

    // Inside a LocalStack Lambda Docker container `localhost` in the endpoint URL
    // resolves to the Lambda container's own loopback, not to the LocalStack container.
    //
    // In LocalStack Lambda containers, `localhost` points at the Lambda container,
    // not the LocalStack container. Ephemeral Lambdas get `LOCALSTACK_MAPPED_PORT`
    // from CDK; some LocalStack versions also set `LOCALSTACK_HOSTNAME`.
    // Use either signal to rewrite the endpoint to the Docker host.
    let endpoint_url_str = if std::env::var("LOCALSTACK_HOSTNAME").is_ok()
        || std::env::var("LOCALSTACK_MAPPED_PORT").is_ok()
    {
        let mapped_port =
            std::env::var("LOCALSTACK_MAPPED_PORT").unwrap_or_else(|_| "4566".to_owned());
        endpoint_url_str
            .replace("https://", "http://")
            .replace("localhost", "host.docker.internal")
            .replace(":4566", &format!(":{mapped_port}"))
    } else {
        endpoint_url_str
    };

    let endpoint_url = Url::parse(&endpoint_url_str)?;
    let stage = std::env::var("STAGE").unwrap_or_else(|_| "prod".into());

    let transport = match stage.as_str() {
        "ephemeral" => {
            TransportBuilder::new(SingleNodeConnectionPool::new(endpoint_url)).build()?
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
