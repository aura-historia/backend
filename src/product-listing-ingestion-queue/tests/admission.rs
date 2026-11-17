//! Admission through the PostgreSQL job queue: FIFO group order, deduplicated replays, the
//! database function's own validation, and the crawler role that may only call it.

use platform_postgres_queue::{ClaimedJob, Disposition, JobQueue};
use platform_worker_queue_contract::job_queue::product_listing_ingestion_job_scope;
use product_listing_ingestion_queue::{QueueProductListingIngestionPublisher, codec};
use product_listing_service::{
    ports::ProductListingIngestionPublisher,
    use_cases::{ProductListingIngestionMessage, ProductListingIngestionOutcome},
};
use serde_json::Value;
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const DATABASE: Postgres = Postgres::new("migrations");
const FIXTURES: &str = include_str!("fixtures/ingestion_v2.json");
const CRAWLER_ROLE: &str = "aura_crawler_ingestion";

/// Fixtures 0 to 3 change one listing (one FIFO group); fixture 4 captures raw evidence in
/// its own group.
fn commands(indices: &[usize]) -> Vec<ProductListingIngestionMessage> {
    let fixtures: Vec<Value> = serde_json::from_str(FIXTURES).unwrap();
    indices
        .iter()
        .map(|&index| {
            codec::decode(&fixtures[index].to_string())
                .unwrap()
                .into_service_envelope()
                .unwrap()
                .message
        })
        .collect()
}

/// Sessions that act as the crawler's role, which the fixture creates without login.
async fn crawler_pool(pool: &PgPool) -> PgPool {
    let options: PgConnectOptions = (*pool.connect_options()).clone();
    PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query(sqlx::AssertSqlSafe(format!("SET ROLE {CRAWLER_ROLE}")))
                    .execute(connection)
                    .await
                    .map(|_| ())
            })
        })
        .connect_with(options)
        .await
        .unwrap()
}

async fn admit(pool: &PgPool, commands: Vec<ProductListingIngestionMessage>) {
    let outcomes = QueueProductListingIngestionPublisher::postgres(pool.clone())
        .publish(commands)
        .await
        .unwrap();
    for item in outcomes {
        assert_eq!(ProductListingIngestionOutcome::Accepted, item.outcome);
    }
}

fn index(job: &ClaimedJob) -> u64 {
    serde_json::from_str::<Value>(job.payload()).unwrap()["index"]
        .as_u64()
        .unwrap()
}

async fn claim_indices(queue: &JobQueue) -> (Vec<ClaimedJob>, Vec<u64>) {
    let jobs = queue.claim(10).await.unwrap();
    let mut indices: Vec<_> = jobs.iter().map(index).collect();
    indices.sort_unstable();
    (jobs, indices)
}

async fn queued_jobs(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM job_queue_jobs WHERE scope = 'product-listing-ingestion'",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_deliver_a_listing_group_in_admission_order_beside_independent_groups() {
    let pool = get_postgres_client().await;
    admit(&crawler_pool(&pool).await, commands(&[0, 1, 2, 3, 4])).await;
    let queue = JobQueue::new(pool, product_listing_ingestion_job_scope());

    let (heads, indices) = claim_indices(&queue).await;
    assert_eq!(vec![0, 4], indices, "one head per group");
    let mut delivered = Vec::new();
    let mut leased = heads;
    while !leased.is_empty() {
        for job in &leased {
            delivered.push(index(job));
            queue
                .settle(job.lease(), Disposition::Complete)
                .await
                .unwrap();
        }
        leased = queue.claim(10).await.unwrap();
        assert!(leased.len() <= 1, "the raw group has no successor");
    }
    delivered.retain(|&index| index != 4);
    assert_eq!(vec![0, 1, 2, 3], delivered);
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_deduplicate_an_identical_replay_while_the_first_job_waits() {
    let pool = get_postgres_client().await;
    let first = commands(&[0]);
    let mut replay = first.clone();
    replay[0].metadata.request_id = "replayed-request".into();
    replay[0].metadata.correlation_id = "replayed-correlation".into();

    admit(&pool, first).await;
    admit(&pool, replay).await;

    assert_eq!(1, queued_jobs(&pool).await);
}

/// Reusing a command ID for different content gets a different dedup key, so admission never
/// absorbs it as a replay; execution then refuses it against the first command's receipt.
#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_never_absorb_conflicting_content_under_one_command_id_as_a_replay() {
    let pool = get_postgres_client().await;
    let first = commands(&[3]);
    let mut conflicting = commands(&[1]);
    conflicting[0].metadata = first[0].metadata.clone();
    conflicting[0].metadata.operation = conflicting[0].intent.operation();

    admit(&pool, first).await;
    admit(&pool, conflicting).await;

    assert_eq!(2, queued_jobs(&pool).await);
    let queue = JobQueue::new(pool, product_listing_ingestion_job_scope());
    let (jobs, _) = claim_indices(&queue).await;
    assert_eq!(1, jobs.len(), "both commands share their listing's group");
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_refuse_invalid_keys_and_payloads_in_the_enqueue_function() {
    let pool = get_postgres_client().await;
    let oversized = "x".repeat(1_048_577);
    for (group, dedup, payload) in [
        ("", "plid1_a", "{}"),
        ("pli1_a", "with space", "{}"),
        ("pli1_a", "plid1_a", ""),
        ("pli1_a", "plid1_a", oversized.as_str()),
    ] {
        let error = sqlx::query("SELECT job_queue_enqueue_product_listing_ingestion($1, $2, $3)")
            .bind(group)
            .bind(dedup)
            .bind(payload)
            .execute(&pool)
            .await
            .unwrap_err();
        let code = error.as_database_error().and_then(|error| error.code());
        assert_eq!(Some("22023"), code.as_deref(), "{group:?} {dedup:?}");
    }
    assert_eq!(0, queued_jobs(&pool).await);
}

#[aura_integration_test(services = [DATABASE], isolated = true)]
async fn should_give_the_crawler_role_no_table_privilege_and_one_executable_definer_function() {
    let pool = get_postgres_client().await;

    let tables_with_privilege: Vec<String> = sqlx::query_scalar(
        "SELECT format('%I.%I', n.nspname, c.relname) FROM pg_class c \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f', 'S') \
           AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
           AND n.nspname NOT LIKE 'pg_toast%' \
           AND (CASE WHEN c.relkind = 'S' \
                THEN has_sequence_privilege($1, c.oid, 'USAGE, SELECT, UPDATE') \
                ELSE has_table_privilege($1, c.oid, \
                    'SELECT, INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER') END \
             OR has_any_column_privilege($1, c.oid, 'SELECT, INSERT, UPDATE, REFERENCES'))",
    )
    .bind(CRAWLER_ROLE)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(Vec::<String>::new(), tables_with_privilege);

    let definer_functions: Vec<String> = sqlx::query_scalar(
        "SELECT p.oid::regprocedure::text FROM pg_proc p \
         JOIN pg_namespace n ON n.oid = p.pronamespace \
         WHERE p.prosecdef AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
           AND has_function_privilege($1, p.oid, 'EXECUTE')",
    )
    .bind(CRAWLER_ROLE)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        vec!["job_queue_enqueue_product_listing_ingestion(text,text,text)".to_owned()],
        definer_functions
    );
    let replication_may_enqueue: bool = sqlx::query_scalar(
        "SELECT has_function_privilege('aura_replication', \
         'job_queue_enqueue_product_listing_ingestion(text, text, text)', 'EXECUTE')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!replication_may_enqueue, "PUBLIC must not execute it");

    let crawler = crawler_pool(&pool).await;
    for statement in [
        "SELECT count(*) FROM job_queue_jobs",
        "INSERT INTO job_queue_jobs (scope, group_key, payload) VALUES ('product-listing-ingestion', 'g', 'p')",
        "DELETE FROM job_queue_failures",
        "SELECT count(*) FROM product_listings",
    ] {
        let error = sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&crawler)
            .await
            .unwrap_err();
        let code = error.as_database_error().and_then(|error| error.code());
        assert_eq!(Some("42501"), code.as_deref(), "{statement}");
    }
    admit(&crawler, commands(&[4])).await;
    assert_eq!(1, queued_jobs(&pool).await);
}
