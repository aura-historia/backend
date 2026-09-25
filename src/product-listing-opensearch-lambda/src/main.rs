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
use product_listing_opensearch_lambda::{
    compose_projection_use_case, handler_with_invocation_budget, invocation_budget,
};
use product_listing_service::use_cases::ProjectProductListingUseCase;
use std::{future::Future, sync::Arc, time::Instant};
use url::Url;

const OPENSEARCH_ENDPOINT_URL_ENV: &str = "OPENSEARCH_ENDPOINT_URL";
const OPENSEARCH_PASSWORD_ENV: &str = "OPENSEARCH_PASSWORD";
const OPENSEARCH_USERNAME_ENV: &str = "OPENSEARCH_USERNAME";
const WORKER_STAGE_ENV: &str = "STAGE";

#[tokio::main]
async fn main() -> Result<(), Error> {
    let initialization_started_at = Instant::now();
    init(logging_config_from_env());

    let postgres = LambdaPostgresConfig::from_env()?;
    let credentials = postgres_credentials_provider_from_env()
        .await
        .map_err(|_| Error::from("PostgreSQL credential provider unavailable"))?;
    let open_search = open_search_client(OpenSearchConfig::from_env()?)?;
    let use_cases = Arc::new(VersionedCompositionCache::new());

    log_cold_start(
        "product-listing-opensearch-lambda",
        initialization_started_at,
    );
    run(service_fn(
        move |event: LambdaEvent<aws_lambda_events::sqs::SqsEvent>| {
            let postgres = postgres.clone();
            let credentials = Arc::clone(&credentials);
            let open_search = open_search.clone();
            let use_cases = Arc::clone(&use_cases);
            async move {
                log_invocation_start("product-listing-opensearch-lambda", &event.context);
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
                            Ok::<_, Error>(compose_projection_use_case(pool, open_search))
                        })
                        .await
                })
                .await
            }
        },
    ))
    .await
}

type ProjectionUseCase = Arc<dyn ProjectProductListingUseCase>;
type ProjectionUseCaseLease = VersionedCompositionLease<ProjectionUseCase>;

/// Keeps setup and record work inside the one Lambda invocation budget.
async fn handle_invocation<F, Fut>(
    event: LambdaEvent<aws_lambda_events::sqs::SqsEvent>,
    setup: F,
) -> Result<aws_lambda_events::sqs::SqsBatchResponse, Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<ProjectionUseCaseLease, Error>>,
{
    handle_sqs_invocation(
        event,
        "product-listing-opensearch-lambda",
        invocation_budget,
        setup,
        |event, use_case, budget| async move {
            handler_with_invocation_budget(event, use_case.value().as_ref(), &budget).await
        },
    )
    .await
}

struct OpenSearchConfig {
    endpoint: Url,
    basic_auth: Option<(String, String)>,
}

impl OpenSearchConfig {
    fn from_env() -> Result<Self, Error> {
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
        Ok(Self {
            endpoint,
            basic_auth,
        })
    }
}

fn open_search_client(config: OpenSearchConfig) -> Result<OpenSearch, Error> {
    let pool = SingleNodeConnectionPool::new(config.endpoint);
    let builder = TransportBuilder::new(pool);
    let transport = match config.basic_auth {
        Some((username, password)) => builder.auth(Credentials::Basic(username, password)),
        None => builder,
    }
    .build()
    .map_err(|_| Error::from("failed to configure OpenSearch client"))?;
    Ok(OpenSearch::new(transport))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use lambda_runtime::Context;
    use platform_lambda_sqs::{SetupOutcome, run_setup_with_budget};
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, SystemTime, UNIX_EPOCH},
    };
    use tokio::sync::Notify;

    #[tokio::test]
    async fn should_not_poll_credential_refresh_when_no_invocation_budget_remains() {
        let budget = expired_budget();
        let polls = Arc::new(AtomicUsize::new(0));
        let operation_polls = Arc::clone(&polls);

        let result = run_setup_with_budget(&budget, || async move {
            operation_polls.fetch_add(1, Ordering::AcqRel);
            Ok::<_, ()>(())
        })
        .await;

        assert!(matches!(result, SetupOutcome::TimedOut));
        assert_eq!(0, polls.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn should_retain_every_record_without_polling_setup_when_the_actual_invocation_budget_is_exhausted()
     {
        let polls = Arc::new(AtomicUsize::new(0));
        let setup_polls = Arc::clone(&polls);
        let response = handle_invocation(event_with_records(Duration::ZERO), move || async move {
            setup_polls.fetch_add(1, Ordering::AcqRel);
            std::future::pending::<Result<ProjectionUseCaseLease, Error>>().await
        })
        .await;

        let response = match response {
            Ok(response) => response,
            Err(error) => panic!("invocation failed: {error}"),
        };
        assert_eq!(failure_ids(response), ["first", "second"]);
        assert_eq!(0, polls.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn should_retain_every_record_when_the_actual_invocation_waits_on_a_composition_build() {
        let cache = Arc::new(VersionedCompositionCache::<ProjectionUseCase>::new());
        let started = Arc::new(Notify::new());
        let held_cache = Arc::clone(&cache);
        let held_started = Arc::clone(&started);
        let holder = tokio::spawn(async move {
            held_cache
                .get_or_try_build("version-1", move || async move {
                    held_started.notify_one();
                    std::future::pending::<Result<ProjectionUseCase, Error>>().await
                })
                .await
        });
        started.notified().await;

        let waiting_cache = Arc::clone(&cache);
        let response = handle_invocation(
            event_with_records(Duration::from_millis(100)),
            move || async move {
                waiting_cache
                    .get_or_try_build("version-1", || async {
                        std::future::pending::<Result<ProjectionUseCase, Error>>().await
                    })
                    .await
            },
        )
        .await;
        holder.abort();
        let holder_result = holder.await;

        let response = match response {
            Ok(response) => response,
            Err(error) => panic!("invocation failed: {error}"),
        };
        assert_eq!(failure_ids(response), ["first", "second"]);
        assert!(holder_result.is_err());
    }

    #[tokio::test]
    async fn should_stop_a_blocked_credential_refresh_at_the_usable_budget() {
        let budget = budget_with_usable_time(Duration::from_millis(100));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let operation_started = Arc::clone(&started);
        let operation_release = Arc::clone(&release);

        let refresh = async move {
            operation_started.notify_one();
            operation_release.notified().await;
            Ok::<_, ()>(())
        };
        let (result, ()) = tokio::join!(run_setup_with_budget(&budget, || refresh), async {
            started.notified().await
        });

        assert!(matches!(result, SetupOutcome::TimedOut));
        release.notify_waiters();
    }

    #[tokio::test]
    async fn should_stop_when_a_versioned_composition_waits_on_an_active_build() {
        let cache = Arc::new(VersionedCompositionCache::<usize>::new());
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let held_cache = Arc::clone(&cache);
        let held_started = Arc::clone(&started);
        let held_release = Arc::clone(&release);
        let holder = tokio::spawn(async move {
            held_cache
                .get_or_try_build("version-1", move || async move {
                    held_started.notify_one();
                    held_release.notified().await;
                    Ok::<_, ()>(1)
                })
                .await
        });
        started.notified().await;

        let budget = budget_with_usable_time(Duration::from_millis(100));
        let result = run_setup_with_budget(&budget, || {
            cache.get_or_try_build("version-1", || async { Ok::<_, ()>(2) })
        })
        .await;

        assert!(matches!(result, SetupOutcome::TimedOut));
        release.notify_waiters();
        let completed = holder.await;
        assert!(matches!(completed, Ok(Ok(_))));
    }

    fn expired_budget() -> platform_lambda_bootstrap::LambdaInvocationBudget {
        let mut context = Context::default();
        context.deadline = epoch_millis();
        invocation_budget(&context)
    }

    fn budget_with_usable_time(
        usable: Duration,
    ) -> platform_lambda_bootstrap::LambdaInvocationBudget {
        let mut context = Context::default();
        context.deadline = epoch_millis().saturating_add(
            (Duration::from_secs(5) + usable)
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
        );
        invocation_budget(&context)
    }

    fn event_with_records(usable_budget: Duration) -> LambdaEvent<SqsEvent> {
        let mut event = SqsEvent::default();
        event.records = ["first", "second"]
            .into_iter()
            .map(|message_id| {
                let mut record = SqsMessage::default();
                record.message_id = Some(message_id.to_owned());
                record.body = Some("{}".to_owned());
                record
            })
            .collect();
        let mut context = Context::default();
        context.deadline = epoch_millis().saturating_add(
            (Duration::from_secs(5) + usable_budget)
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
        );
        LambdaEvent::new(event, context)
    }

    fn failure_ids(response: aws_lambda_events::sqs::SqsBatchResponse) -> Vec<String> {
        response
            .batch_item_failures
            .into_iter()
            .map(|failure| failure.item_identifier)
            .collect()
    }

    fn epoch_millis() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or_default()
    }
}
