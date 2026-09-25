use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use large_language_model::{VertexAiConfig, VertexAiGemini};
use platform_lambda_bootstrap::{
    LambdaPostgresConfig, VersionedCompositionCache, VersionedCompositionLease,
    google_application_default_credentials, log_cold_start, log_invocation_start,
    logging_config_from_env, materialize_google_application_credentials_from_env,
    required_config_from_env,
};
use platform_lambda_sqs::handle_sqs_invocation;
use platform_observability::init;
use platform_postgres_secretsmanager::postgres_credentials_provider_from_env;
use product_listing_service::use_cases::TranslateProductListingEventUseCase;
use product_translation_lambda::{
    compose_product_translation_use_case, handler_with_invocation_budget, invocation_budget,
};
use std::{future::Future, sync::Arc, time::Instant};

const COMPONENT: &str = "product-translation-lambda";
const VERTEX_AI_LOCATION_ENV: &str = "VERTEX_AI_LOCATION";
const VERTEX_AI_MODEL_ENV: &str = "VERTEX_AI_MODEL";
const VERTEX_AI_PROJECT_ID_ENV: &str = "VERTEX_AI_PROJECT_ID";
fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    materialize_google_application_credentials_from_env()?;
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
    let vertex = VertexConfig::from_env()?;
    let use_cases = Arc::new(VersionedCompositionCache::new());

    log_cold_start(COMPONENT, initialization_started_at);
    run(service_fn(
        move |event: LambdaEvent<aws_lambda_events::sqs::SqsEvent>| {
            let postgres = postgres.clone();
            let credentials = Arc::clone(&credentials);
            let vertex = vertex.clone();
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
                            let model = vertex.large_language_model()?;
                            Ok::<_, Error>(compose_product_translation_use_case(pool, model))
                        })
                        .await
                })
                .await
            }
        },
    ))
    .await
}

type ProductTranslationUseCase = Arc<dyn TranslateProductListingEventUseCase>;
type ProductTranslationUseCaseLease = VersionedCompositionLease<ProductTranslationUseCase>;

async fn handle_invocation<F, Fut>(
    event: LambdaEvent<aws_lambda_events::sqs::SqsEvent>,
    setup: F,
) -> Result<aws_lambda_events::sqs::SqsBatchResponse, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<ProductTranslationUseCaseLease, Error>>,
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
struct VertexConfig {
    project_id: String,
    location: String,
    model: String,
}

impl VertexConfig {
    fn from_env() -> Result<Self, Error> {
        Ok(Self {
            project_id: required_config_from_env(VERTEX_AI_PROJECT_ID_ENV)?,
            location: required_config_from_env(VERTEX_AI_LOCATION_ENV)?,
            model: required_config_from_env(VERTEX_AI_MODEL_ENV)?,
        })
    }

    fn large_language_model(&self) -> Result<VertexAiGemini, Error> {
        let credentials = google_application_default_credentials()?;
        VertexAiGemini::new(
            VertexAiConfig::new(
                self.project_id.clone(),
                self.location.clone(),
                self.model.clone(),
            ),
            credentials,
        )
        .map_err(|_| Error::from("failed to configure Vertex AI client"))
    }
}
