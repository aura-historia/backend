use aws_config::BehaviorVersion;
use classifier_model::{
    CloudflareClassifierConfig, CloudflareClassifierModel, CloudflareModel, Probability,
};
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use opensearch::{
    OpenSearch,
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, VersionedCompositionLease, log_cold_start,
    log_invocation_start, logging_config_from_env, required_config_from_env,
};
use platform_lambda_sqs::handle_sqs_invocation;
use platform_observability::init;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use search_filter_percolator_lambda::{
    compose_percolator_use_case, handler_with_invocation_budget, invocation_budget,
};
use search_filter_service::use_cases::MatchProductListingEventUseCase;
use std::{future::Future, str::FromStr, sync::Arc, time::Instant};
use url::Url;

const COMPONENT: &str = "search-filter-percolator-lambda";
const OPENSEARCH_ENDPOINT_URL_ENV: &str = "OPENSEARCH_ENDPOINT_URL";
const OPENSEARCH_PASSWORD_ENV: &str = "OPENSEARCH_PASSWORD";
const OPENSEARCH_USERNAME_ENV: &str = "OPENSEARCH_USERNAME";
const WORKER_STAGE_ENV: &str = "STAGE";
const DEFAULT_CLASSIFIER_PROVIDER: &str = "cloudflare";
const DEFAULT_CLASSIFIER_MODEL: &str = "clef-flash";
const DEFAULT_SHOULD_SHOW_THRESHOLD_BPS: u16 = 5_000;
fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| Error::from("failed to build Lambda async runtime"))?
        .block_on(async_main(initialization_started_at))
}

async fn async_main(initialization_started_at: Instant) -> Result<(), Error> {
    let postgres = LambdaPostgresConfig::from_env()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
    let config = PercolatorConfig::from_env().await?;
    let open_search = open_search_client(&config)?;
    let use_cases = Arc::new(VersionedCompositionCache::new());

    log_cold_start(COMPONENT, initialization_started_at);
    run(service_fn(
        move |event: LambdaEvent<aws_lambda_events::sqs::SqsEvent>| {
            let postgres = postgres.clone();
            let credentials = Arc::clone(&credentials);
            let open_search = open_search.clone();
            let config = config.clone();
            let use_cases = Arc::clone(&use_cases);
            async move {
                log_invocation_start(COMPONENT, &event.context);
                handle_invocation(event, move || async move {
                    let credentials = credentials
                        .current()
                        .await
                        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
                    use_cases
                        .get_or_try_build(credentials.version_id(), || async {
                            let pool = postgres
                                .pool_config(credentials.credentials())
                                .map_err(|_| Error::from("invalid PostgreSQL configuration"))?
                                .connect()
                                .await
                                .map_err(|_| Error::from("failed to create PostgreSQL pool"))?;
                            let evaluator = cloudflare_classifier(&config)?;
                            Ok::<_, Error>(compose_percolator_use_case(
                                pool,
                                open_search,
                                evaluator,
                                config.should_show_threshold,
                            ))
                        })
                        .await
                })
                .await
            }
        },
    ))
    .await
}

type PercolatorUseCase = Arc<dyn MatchProductListingEventUseCase>;
type PercolatorUseCaseLease = VersionedCompositionLease<PercolatorUseCase>;

async fn handle_invocation<F, Fut>(
    event: LambdaEvent<aws_lambda_events::sqs::SqsEvent>,
    setup: F,
) -> Result<aws_lambda_events::sqs::SqsBatchResponse, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<PercolatorUseCaseLease, Error>>,
{
    handle_sqs_invocation(
        event,
        COMPONENT,
        invocation_budget,
        setup,
        |event, use_case, budget| async move {
            handler_with_invocation_budget(event, use_case.value().as_ref(), &budget).await
        },
    )
    .await
}

#[derive(Clone)]
struct PercolatorConfig {
    endpoint: Url,
    basic_auth: Option<(String, String)>,
    classifier_provider: String,
    classifier_model: CloudflareModel,
    cloudflare_account_id: String,
    cloudflare_api_token: String,
    should_show_threshold: Probability,
}

impl PercolatorConfig {
    async fn from_env() -> Result<Self, Error> {
        let stage = required_config_from_env(WORKER_STAGE_ENV)?;
        let endpoint = required_config_from_env(OPENSEARCH_ENDPOINT_URL_ENV)?;
        let endpoint =
            Url::parse(&endpoint).map_err(|_| Error::from("invalid OpenSearch endpoint"))?;
        let basic_auth = if stage == "ephemeral" {
            None
        } else {
            Some((
                required_config_from_env(OPENSEARCH_USERNAME_ENV)?,
                required_config_from_env(OPENSEARCH_PASSWORD_ENV)?,
            ))
        };
        let classifier_provider = std::env::var("CLASSIFIER_MODEL_PROVIDER")
            .unwrap_or_else(|_| DEFAULT_CLASSIFIER_PROVIDER.to_owned());
        if classifier_provider != "cloudflare" {
            return Err(Error::from("invalid classifier model provider"));
        }
        let classifier_model = CloudflareModel::from_str(
            &std::env::var("CLASSIFIER_MODEL")
                .unwrap_or_else(|_| DEFAULT_CLASSIFIER_MODEL.to_owned()),
        )
        .map_err(|_| Error::from("invalid classifier model"))?;
        let threshold_bps = std::env::var("SEARCH_FILTER_MATCH_SHOULD_SHOW_THRESHOLD_BPS")
            .ok()
            .map(|value| value.parse::<u16>())
            .transpose()
            .map_err(|_| Error::from("invalid saved-search match threshold"))?
            .unwrap_or(DEFAULT_SHOULD_SHOW_THRESHOLD_BPS);
        if threshold_bps > 10_000 {
            return Err(Error::from("invalid saved-search match threshold"));
        }
        let cloudflare_account_id = required_config_from_env("CLOUDFLARE_ACCOUNT_ID")?;
        let cloudflare_api_token = match std::env::var("CLOUDFLARE_API_TOKEN") {
            Ok(token) if !token.trim().is_empty() => token,
            Ok(_) => return Err(Error::from("invalid classifier credential configuration")),
            Err(_) => {
                let parameter_name =
                    required_config_from_env("CLOUDFLARE_API_TOKEN_SSM_PARAMETER")?;
                let config = aws_config::defaults(BehaviorVersion::latest()).load().await;
                aws_sdk_ssm::Client::new(&config)
                    .get_parameter()
                    .name(parameter_name)
                    .with_decryption(true)
                    .send()
                    .await
                    .map_err(|_| Error::from("classifier credentials unavailable"))?
                    .parameter()
                    .and_then(|parameter| parameter.value())
                    .filter(|token| !token.trim().is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| Error::from("classifier credentials unavailable"))?
            }
        };
        Ok(Self {
            endpoint,
            basic_auth,
            classifier_provider,
            classifier_model,
            cloudflare_account_id,
            cloudflare_api_token,
            should_show_threshold: Probability::new(f64::from(threshold_bps) / 10_000.0)
                .map_err(|_| Error::from("invalid saved-search match threshold"))?,
        })
    }
}

fn open_search_client(config: &PercolatorConfig) -> Result<OpenSearch, Error> {
    let pool = SingleNodeConnectionPool::new(config.endpoint.clone());
    let builder = TransportBuilder::new(pool);
    let transport = match &config.basic_auth {
        Some((username, password)) => {
            builder.auth(Credentials::Basic(username.clone(), password.clone()))
        }
        None => builder,
    }
    .build()
    .map_err(|_| Error::from("failed to configure OpenSearch client"))?;
    Ok(OpenSearch::new(transport))
}

fn cloudflare_classifier(config: &PercolatorConfig) -> Result<CloudflareClassifierModel, Error> {
    if config.classifier_provider != "cloudflare" {
        return Err(Error::from("invalid classifier model provider"));
    }
    let classifier_config = CloudflareClassifierConfig::new(
        config.cloudflare_account_id.clone(),
        config.cloudflare_api_token.clone(),
        config.classifier_model,
    )
    .map_err(|_| Error::from("failed to configure classifier client"))?;
    CloudflareClassifierModel::new(classifier_config)
        .map_err(|_| Error::from("failed to configure classifier client"))
}
