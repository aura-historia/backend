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
    let endpoint_url_str = std::env::var("OPENSEARCH_ENDPOINT_URL")?;

    // Inside a LocalStack Lambda Docker container `localhost` in the endpoint URL
    // resolves to the Lambda container's own loopback, not to the LocalStack container.
    //
    // LocalStack Pro sets `LOCALSTACK_HOSTNAME` in every Lambda container it spawns,
    // so we use its presence as a signal that we are running inside such a container.
    //
    // Two further rewrites are required:
    //  1. `localhost` → `host.docker.internal`
    //     The Lambda container has `host.docker.internal` mapped to the Docker host
    //     gateway via `--add-host=host.docker.internal:host-gateway` (passed through
    //     `LAMBDA_DOCKER_FLAGS` when LocalStack is started). This lets the Lambda reach
    //     services bound on the host machine.
    //  2. `:4566` → `:{LOCALSTACK_MAPPED_PORT}`
    //     LocalStack listens on a randomly chosen free port on the host (not on the
    //     fixed internal container port 4566). `LOCALSTACK_MAPPED_PORT` carries that
    //     port and is injected into every Lambda via the CloudFormation parameter
    //     `LocalStackMappedPort`.
    //  3. `https://` → `http://`
    //     The CloudFormation `!Sub "https://…"` produces an HTTPS URL, but LocalStack's
    //     OpenSearch proxy is plain HTTP.
    let endpoint_url_str = match std::env::var("LOCALSTACK_HOSTNAME") {
        Ok(_) => {
            let mapped_port = std::env::var("LOCALSTACK_MAPPED_PORT")
                .unwrap_or_else(|_| "4566".to_owned());
            endpoint_url_str
                .replace("https://", "http://")
                .replace("localhost", "host.docker.internal")
                .replace(":4566", &format!(":{mapped_port}"))
        }
        Err(_) => endpoint_url_str,
    };

    let endpoint_url = Url::parse(&endpoint_url_str)?;
    let stage = std::env::var("STAGE").unwrap_or_else(|_| "prod".into());

    let transport = match stage.as_str() {
        "ephemeral" => {
            let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
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
