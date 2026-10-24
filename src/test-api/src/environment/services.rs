use super::contract::{ManagedEnvironment, PostgresEndpoint, Profile};
use super::docker::{docker, labels, published_port};
use super::{Result, context, invalid};
use sqlx::postgres::{PgConnectOptions, PgSslMode};
use sqlx::{ConnectOptions, Connection};
use std::path::Path;
use std::time::{Duration, Instant};

// Release tags aid review; immutable digests are the actual managed image policy.
pub(super) const POSTGRES_IMAGE: &str =
    "postgres:16-bookworm@sha256:efedf3595f1d6f415c08568ba171029bf54052e754cc9f030e3f2412b21f3d67";
pub(super) const FLOCI_IMAGE: &str =
    "floci/floci:2.3.0@sha256:e0b292df4dc9559b7b1bf4f51bdda4bb8e269276236438cd03cae661260ea8a9";
pub(super) const OPENSEARCH_IMAGE: &str = "opensearchproject/opensearch:3.9.0@sha256:adfa61f85025d06b4aeb562e7e74fde7e31c437039c93c3862c17e9acebd6c7c";

pub(super) async fn bootstrap(
    run: &str,
    profile: Profile,
    slots: u16,
    dir: &Path,
) -> Result<ManagedEnvironment> {
    let network = format!("aura-test-{run}");
    let mut args = vec!["network".into(), "create".into()];
    args.extend(labels(run));
    args.push(network.clone());
    docker(args).await?;
    let mut config = ManagedEnvironment {
        schema_version: 1,
        run_id: run.into(),
        profile,
        slots,
        postgres: None,
        aws_endpoint: None,
        opensearch_endpoint: None,
        aws_services: profile.services(),
        directory: None,
    };
    if profile.postgres() {
        config.postgres = Some(start_postgres(run, &network, dir).await?);
    }
    if profile.aws() {
        config.aws_endpoint = Some(start_floci(run, &network).await?);
    }
    if profile.opensearch() {
        config.opensearch_endpoint = Some(start_opensearch(run, &network).await?);
    }
    config.validate()?;
    Ok(config)
}

async fn start(
    run: &str,
    network: &str,
    service: &str,
    image: &str,
    port: u16,
    options: Vec<String>,
    command: Vec<String>,
) -> Result<(String, u16)> {
    let started = Instant::now();
    let name = format!("aura-test-{run}-{service}");
    let mut args = vec![
        "create".into(),
        "--name".into(),
        name.clone(),
        "--network".into(),
        network.into(),
        "--restart=no".into(),
        "--publish".into(),
        format!("127.0.0.1::{port}"),
    ];
    args.extend(labels(run));
    args.extend(options);
    args.push(image.into());
    args.extend(command);
    docker(args).await?;
    docker(["start", &name]).await?;
    let port = published_port(&name, port).await?;
    eprintln!(
        "AURA_TEST_ENV {}",
        serde_json::json!({"event":"SERVICE_STARTED","run_id":run,
        "service":service,"count":1,"elapsed_ms":started.elapsed().as_millis()})
    );
    Ok((name, port))
}

async fn volume(run: &str, service: &str) -> Result<String> {
    let name = format!("aura-test-{run}-{service}");
    let mut args = vec!["volume".into(), "create".into()];
    args.extend(labels(run));
    args.push(name.clone());
    docker(args).await?;
    Ok(name)
}

async fn start_postgres(run: &str, network: &str, dir: &Path) -> Result<PostgresEndpoint> {
    let volume = volume(run, "postgres").await?;
    let options = vec![
        "--entrypoint".into(),
        "/bin/sh".into(),
        "--env".into(),
        "POSTGRES_PASSWORD=postgres".into(),
        "--mount".into(),
        format!("type=volume,src={volume},dst=/var/lib/postgresql/data"),
    ];
    let (name, port) = start(
        run,
        network,
        "postgres",
        POSTGRES_IMAGE,
        5432,
        options,
        vec![
            "-ec".into(),
            crate::postgres::tls::POSTGRES_TLS_ENTRYPOINT.into(),
        ],
    )
    .await?;
    let root_certificate = dir.join("postgres-ca.crt");
    let endpoint = PostgresEndpoint {
        port,
        root_certificate,
    };
    let source = format!(
        "{name}:{}",
        crate::postgres::tls::POSTGRES_TLS_ROOT_CERTIFICATE
    );
    let path = endpoint
        .root_certificate
        .to_str()
        .ok_or_else(|| invalid("CA path must be UTF-8"))?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut poll = tokio::time::interval(Duration::from_millis(200));
    loop {
        poll.tick().await;
        let ready = match docker(["cp", &source, path]).await {
            Ok(_) => check_postgres(&endpoint).await,
            Err(error) => Err(error),
        };
        match ready {
            Ok(()) => return Ok(endpoint),
            Err(error) if Instant::now() >= deadline => {
                return Err(context(
                    "Postgres did not become ready with verified TLS",
                    error,
                ));
            }
            Err(_) => {}
        }
    }
}

pub(super) async fn check_postgres(endpoint: &PostgresEndpoint) -> Result<()> {
    let options = PgConnectOptions::new()
        .host("127.0.0.1")
        .port(endpoint.port)
        .username("postgres")
        .password("postgres")
        .database("postgres")
        .ssl_mode(PgSslMode::VerifyFull)
        .ssl_root_cert(&endpoint.root_certificate);
    let mut connection = tokio::time::timeout(Duration::from_secs(5), options.connect())
        .await
        .map_err(|e| context("Postgres readiness timed out", e))??;
    let (version,wal,ssl):(String,String,bool)=sqlx::query_as("SELECT current_setting('server_version_num'), current_setting('wal_level'), (SELECT ssl FROM pg_stat_ssl WHERE pid=pg_backend_pid())")
        .fetch_one(&mut connection).await?;
    connection.close().await?;
    if version.parse::<u32>()? / 10000 != 16 || wal != "logical" || !ssl {
        return Err(invalid(
            "test Postgres requires version 16, logical WAL and verified TLS",
        ));
    }
    Ok(())
}

async fn start_floci(run: &str, network: &str) -> Result<String> {
    let options = vec![
        "--env".into(),
        "FLOCI_STORAGE_MODE=memory".into(),
        "--env".into(),
        "FLOCI_DEFAULT_REGION=eu-central-1".into(),
        "--env".into(),
        "FLOCI_SERVICES_OPENSEARCH_MOCK=false".into(),
        "--env".into(),
        format!("FLOCI_DOCKER_RESOURCE_NAMESPACE=aura-test-{run}"),
    ];
    let (name, port) = start(run, network, "floci", FLOCI_IMAGE, 4566, options, vec![]).await?;
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut poll = tokio::time::interval(Duration::from_millis(250));
    loop {
        poll.tick().await;
        let output = docker(["inspect", "--format", "{{.State.Health.Status}}", &name]).await?;
        if output.trim() == "healthy" {
            return Ok(format!("http://127.0.0.1:{port}"));
        }
        if Instant::now() >= deadline {
            return Err(invalid("Floci did not become healthy"));
        }
    }
}

async fn start_opensearch(run: &str, network: &str) -> Result<String> {
    let volume = volume(run, "opensearch").await?;
    let options = vec![
        "--env".into(),
        "discovery.type=single-node".into(),
        "--env".into(),
        "DISABLE_SECURITY_PLUGIN=true".into(),
        "--env".into(),
        "DISABLE_INSTALL_DEMO_CONFIG=true".into(),
        "--env".into(),
        "OPENSEARCH_JAVA_OPTS=-Xms512m -Xmx512m".into(),
        "--mount".into(),
        format!("type=volume,src={volume},dst=/usr/share/opensearch/data"),
    ];
    let (_, port) = start(
        run,
        network,
        "opensearch",
        OPENSEARCH_IMAGE,
        9200,
        options,
        vec![],
    )
    .await?;
    let endpoint = format!("http://127.0.0.1:{port}");
    let client = search_client(&endpoint)?;
    crate::opensearch::bootstrap_indices(&client).await?;
    let response = client
        .info()
        .send()
        .await?
        .error_for_status_code()?
        .json::<serde_json::Value>()
        .await?;
    if response["version"]["number"] != "3.9.0" {
        return Err(invalid("managed OpenSearch must be the real 3.9.0 engine"));
    }
    Ok(endpoint)
}

pub(super) fn search_client(endpoint: &str) -> Result<opensearch::OpenSearch> {
    use opensearch::http::transport::{SingleNodeConnectionPool, TransportBuilder};
    Ok(opensearch::OpenSearch::new(
        TransportBuilder::new(SingleNodeConnectionPool::new(endpoint.parse()?))
            .disable_proxy()
            .timeout(Duration::from_secs(5))
            .build()?,
    ))
}

pub(super) async fn probe(config: &ManagedEnvironment) -> Result<()> {
    if let Some(endpoint) = &config.postgres {
        check_postgres(endpoint).await?;
    }
    if config.postgres.is_some() {
        let pool = crate::get_postgres_client().await;
        sqlx::query("SELECT 1").execute(&pool).await?;
        pool.close().await;
    }
    if config.aws_endpoint.is_some() {
        crate::aws::get_aws_emulator(
            &config
                .aws_services
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            &[],
        )
        .await;
        crate::get_sqs_client().await.list_queues().send().await?;
    }
    if config.opensearch_endpoint.is_some() {
        let client = crate::get_opensearch_client().await;
        let query =
            serde_json::json!({"query":{"hybrid":{"queries":[{"match_all":{}},{"match_all":{}}]}}});
        let parameters =
            std::collections::HashMap::from([("search_pipeline", "hybrid-search-pipeline")]);
        let response = client
            .send(
                opensearch::http::Method::Post,
                "product-listings/_search",
                opensearch::http::headers::HeaderMap::new(),
                Some(&parameters),
                Some(opensearch::http::request::JsonBody::new(query)),
                None,
            )
            .await?
            .error_for_status_code()?;
        let body: serde_json::Value = response.json().await?;
        if body["_shards"]["failed"] != 0 {
            return Err(invalid("native hybrid search probe failed"));
        }
    }
    Ok(())
}
