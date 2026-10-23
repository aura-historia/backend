use application::{
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, UnitOfWork},
};
use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
use geo::AddressText;
use geo_postgres::SqlxResolutionRepositoryFactory;
use geo_service::{geocoding::*, resolution::*};
use lambda_runtime::{Context, LambdaEvent};
use platform_postgres::SqlxUnitOfWork;
use sqlx::PgPool;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use uuid::Uuid;

const SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");
const PROFILE: &str = "WORKER_TEST_V1";

struct Provider {
    pool: PgPool,
    calls: AtomicUsize,
    fail: bool,
}
#[async_trait::async_trait]
impl GeocodingProvider for Provider {
    async fn geocode(&self, _: &GeocodingRequest) -> Result<GeocodingResult, GeocodingError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // The composition shares this one-connection pool. Provider I/O must release it.
        tokio::time::timeout(
            Duration::from_secs(1),
            sqlx::query("SELECT 1").execute(&self.pool),
        )
        .await
        .unwrap()
        .unwrap();
        if self.fail {
            return Err(GeocodingError {
                kind: GeocodingFailure::Unavailable,
                source: Box::new(std::io::Error::other("isolated provider failure")),
            });
        }
        Ok(GeocodingResult {
            outcome: GeocodingOutcome::NoMatch,
            retention: ResultRetention::Storage,
        })
    }
}
async fn setup(fail: bool) -> (PgPool, Arc<Provider>, ResolutionBinding, ResolutionWork) {
    let fixture = get_postgres_client().await;
    sqlx::query("UPDATE geo_resolution_provider_controls SET is_default=false")
        .execute(&fixture)
        .await
        .unwrap();
    sqlx::query("INSERT INTO geo_resolution_provider_controls(profile,is_default,enabled,storage_permitted) VALUES($1,true,true,true)")
        .bind(PROFILE).execute(&fixture).await.unwrap();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(fixture.connect_options().as_ref().clone())
        .await
        .unwrap();
    test_api::TestContext::current()
        .unwrap()
        .register_postgres_pool(pool.clone())
        .unwrap();
    let provider = Arc::new(Provider {
        pool: pool.clone(),
        calls: AtomicUsize::new(0),
        fail,
    });
    let binding = ResolutionBinding {
        subject: ResolutionSubject::new(RetentionPurpose::UserLocation, Uuid::now_v7()).unwrap(),
        input_revision: 1,
        guard: 1,
    };
    let input = ResolutionInput::new(
        GeocodingRequest::new(AddressText::new("isolated private input").unwrap()),
        ResolutionScope::new(ScopeKind::User, Some(Uuid::now_v7())).unwrap(),
        RetentionPurpose::UserLocation,
    )
    .unwrap();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let id = SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .bind(&input, binding)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    (
        pool,
        provider,
        binding,
        ResolutionWork { id, generation: 1 },
    )
}
fn compose(pool: &PgPool, provider: &Arc<Provider>) -> impl ResolveUseCase {
    ResolveHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxResolutionRepositoryFactory,
        Arc::clone(provider),
        PROFILE.into(),
    )
}
fn job(work: ResolutionWork) -> String {
    // Exact compact wire format used by CDC and DLQ redrive, without address/evidence.
    let id = work.id.to_string();
    serde_json::json!({
        "schema_version":2, "scope":"geo-resolution", "job_type":"GEO_RESOLUTION",
        "idempotency_key":format!("geo-resolution:{id}:{}", work.generation),
        "ordering_key":format!("geo-resolution:{id}"),
        "payload": { "resolution_id":id, "generation":work.generation }
    })
    .to_string()
}
fn event<const N: usize>(records: [(&str, String); N]) -> LambdaEvent<SqsEvent> {
    let mut payload = SqsEvent::default();
    payload.records = records
        .into_iter()
        .map(|(id, body)| {
            let mut message = SqsMessage::default();
            message.message_id = Some(id.into());
            message.body = Some(body);
            message
        })
        .collect();
    let mut context = Context::default();
    context.deadline = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 60_000;
    LambdaEvent::new(payload, context)
}
async fn process<const N: usize>(
    records: [(&str, String); N],
    use_case: &dyn ResolveUseCase,
) -> Vec<String> {
    geo_resolution_lambda::handler(event(records), use_case, Duration::from_secs(40))
        .await
        .unwrap()
        .batch_item_failures
        .into_iter()
        .map(|item| item.item_identifier)
        .collect()
}
async fn view(pool: &PgPool, binding: ResolutionBinding) -> ResolutionView {
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let view = SqlxResolutionRepositoryFactory
        .resolution_reader(&mut tx)
        .inspect(binding.subject)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    view
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn real_worker_replay_partial_failure_and_scheduled_recovery_use_database_authority() {
    let (pool, provider, binding, work) = setup(false).await;
    let use_case = compose(&pool, &provider);
    assert_eq!(
        vec!["malformed"],
        process(
            [("first", job(work)), ("malformed", "invalid".into())],
            &use_case
        )
        .await
    );
    assert!(
        process([("redrive", job(work))], &use_case)
            .await
            .is_empty()
    );
    assert_eq!(1, provider.calls.load(Ordering::SeqCst));
    assert_eq!(ResolutionStatus::NoMatch, view(&pool, binding).await.status);

    // Completed cache can be acknowledged while the provider is paused.
    sqlx::query("UPDATE geo_resolution_provider_controls SET enabled=false")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        process([("cached-disabled", job(work))], &use_case)
            .await
            .is_empty()
    );
    sqlx::query("UPDATE geo_resolution_provider_controls SET enabled=true")
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .requeue(binding, work, true)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let current = view(&pool, binding).await.work;
    assert!(
        process([("stale-redrive", job(work))], &use_case)
            .await
            .is_empty()
    );
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    assert!(matches!(
        SqlxResolutionRepositoryFactory
            .resolutions(&mut tx)
            .claim(current, PROFILE)
            .await
            .unwrap(),
        ClaimOutcome::Claimed(_)
    ));
    tx.commit().await.unwrap();
    assert_eq!(
        vec!["active"],
        process([("active", job(current))], &use_case).await
    );
    sqlx::query("UPDATE geo_resolutions SET lease_until=clock_timestamp()-interval '1 second' WHERE resolution_id=$1")
        .bind(work.id.into_uuid()).execute(&pool).await.unwrap();
    let maintenance = MaintainResolutionsHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxResolutionRepositoryFactory,
    );
    let context = OperationContext {
        principal: Principal::Service("backend-cleanup-worker".into()),
        request_id: "test".into(),
        correlation_id: "test".into(),
    };
    assert_eq!(
        ResolutionMaintenanceCounts {
            expired: 0,
            requeued: 1
        },
        maintenance.execute(&context, 100).await.unwrap()
    );
    let recovered = view(&pool, binding).await.work;
    assert_eq!(current.generation + 1, recovered.generation);
    assert_eq!(1, provider.calls.load(Ordering::SeqCst));
    assert!(
        process([("recovered-redrive", job(current))], &use_case)
            .await
            .is_empty()
    );
    assert!(
        process([("recovered-wakeup", job(recovered))], &use_case)
            .await
            .is_empty()
    );
    assert_eq!(2, provider.calls.load(Ordering::SeqCst));

    sqlx::query("UPDATE geo_resolutions SET valid_until=clock_timestamp()-interval '1 second' WHERE resolution_id=$1")
        .bind(work.id.into_uuid()).execute(&pool).await.unwrap();
    assert!(view(&pool, binding).await.evidence.is_none());
    sqlx::query("UPDATE geo_resolution_provider_controls SET enabled=false")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        ResolutionMaintenanceCounts {
            expired: 1,
            requeued: 0
        },
        maintenance.execute(&context, 100).await.unwrap()
    );
    let expired = view(&pool, binding).await;
    assert_eq!(recovered.generation + 1, expired.work.generation);
    assert!(expired.evidence.is_none());
    assert!(
        process([("expired-stale-redrive", job(current))], &use_case)
            .await
            .is_empty()
    );
    assert_eq!(
        vec!["disabled-pending"],
        process([("disabled-pending", job(expired.work))], &use_case).await
    );
    assert_eq!(2, provider.calls.load(Ordering::SeqCst));
}

#[aura_integration_test(services=[SCHEMA], isolated = true)]
async fn provider_failure_preserves_its_cause_and_incomplete_queue_custody() {
    let (pool, provider, binding, work) = setup(true).await;
    let use_case = compose(&pool, &provider);
    let context = OperationContext {
        principal: Principal::Service("worker-test".into()),
        request_id: "test".into(),
        correlation_id: "test".into(),
    };
    let error = use_case.execute(&context, work).await.unwrap_err();
    let ResolutionError::Provider(error) = error else {
        panic!("provider source must survive");
    };
    assert_eq!(GeocodingFailure::Unavailable, error.kind);
    assert_eq!(
        "isolated provider failure",
        error
            .source
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .to_string()
    );
    let current = view(&pool, binding).await;
    assert_eq!(ResolutionStatus::Pending, current.status);
    assert_eq!(Some("UNAVAILABLE"), current.last_failure.as_deref());
    assert!(current.evidence.is_none());
    assert_eq!(
        vec!["backoff"],
        process([("backoff", job(work))], &use_case).await
    );
    assert_eq!(1, provider.calls.load(Ordering::SeqCst));
}
