#![cfg(all(
    feature = "supervisor",
    feature = "s3",
    feature = "ses",
    feature = "aura-historia-api"
))]

use sqlx::Executor;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use test_api::{IntegrationTestService, Isolation, TestContext};

#[test]
fn should_verify_managed_context_isolation_and_recovery() {
    assert!(
        test_api::environment::managed_environment().is_none(),
        "context acceptance owns its supervisor and must run in the unmanaged control phase"
    );
    let directory =
        std::env::temp_dir().join(format!("aura-context-acceptance-{}", uuid::Uuid::new_v4()));
    let result = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_WORKSPACE_DIR"),
            "ci/test-context-smoke.py"
        ))
        .args(["--binary", env!("CARGO_BIN_EXE_aura-test-env")])
        .arg("--fixture-binary")
        .arg(std::env::current_exe().unwrap())
        .args(["--slots", "2", "--output"])
        .arg(&directory)
        .output()
        .expect("context acceptance driver should execute");
    assert!(
        result.status.success(),
        "{}\n{}\ncontext acceptance evidence: {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
        directory.display()
    );
    println!("{}", String::from_utf8_lossy(&result.stdout));
    std::fs::remove_dir_all(directory).unwrap();
}

struct ApiState {
    marker: String,
    starts: std::sync::atomic::AtomicUsize,
    factory_pointer: std::sync::atomic::AtomicUsize,
}
static API: test_api::AuraHistoriaApi = test_api::AuraHistoriaApi::new(app);
fn app() -> Pin<Box<dyn Future<Output = axum::Router> + Send>> {
    Box::pin(async {
        use std::sync::atomic::Ordering;
        let context = TestContext::current().unwrap();
        let state = context.api_state::<std::sync::Arc<ApiState>>().unwrap();
        state.starts.fetch_add(1, Ordering::SeqCst);
        state
            .factory_pointer
            .store(std::sync::Arc::as_ptr(&state) as usize, Ordering::SeqCst);
        let pool = test_api::get_postgres_client().await;
        axum::Router::new()
            .route(
                "/marker",
                axum::routing::get(
                    |axum::extract::State((pool, state)): axum::extract::State<(
                        sqlx::PgPool,
                        std::sync::Arc<ApiState>,
                    )>| async move {
                        let marker = sqlx::query_scalar::<_, String>(
                            "SELECT name FROM test_items WHERE value=1",
                        )
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                        assert_eq!(state.marker, marker);
                        marker
                    },
                ),
            )
            .with_state((pool, state))
    })
}

async fn send_email(source: &str, marker: &str) {
    use aws_sdk_sesv2::types::{Body, Content, Destination, EmailContent, Message};
    test_api::get_ses_client()
        .await
        .send_email()
        .from_email_address(source)
        .destination(
            Destination::builder()
                .to_addresses("recipient@example.test")
                .build(),
        )
        .content(
            EmailContent::builder()
                .simple(
                    Message::builder()
                        .subject(Content::builder().data(marker).build().unwrap())
                        .body(
                            Body::builder()
                                .text(Content::builder().data(marker).build().unwrap())
                                .build(),
                        )
                        .build(),
                )
                .build(),
        )
        .send()
        .await
        .unwrap();
}

#[tokio::test]
async fn context_fixture_child() {
    let Ok(action) = std::env::var("AURA_CONTEXT_ACTION") else {
        return;
    };
    assert!(test_api::environment::managed_environment().is_some());
    test_api::aws::get_aws_emulator(&["sqs", "s3", "sesv2"], &[]).await;
    let session = if std::env::var_os("AURA_CONTEXT_EXPLICIT").is_some() {
        TestContext::acquire_explicit("explicit process isolation probe", Isolation::Isolated)
            .await
            .unwrap()
    } else {
        TestContext::acquire("process isolation probe", Isolation::Isolated)
            .await
            .unwrap()
    };
    if let Ok(previous) = std::env::var("AURA_CONTEXT_PREVIOUS")
        && tokio::fs::try_exists(&previous).await.unwrap()
    {
        assert_recovered(
            &serde_json::from_slice::<serde_json::Value>(&tokio::fs::read(previous).await.unwrap())
                .unwrap(),
        )
        .await;
    }
    let context = session.context().clone();
    context.scope(exercise_context(&context, &action)).await;
    session.finish().await.unwrap();
}

async fn exercise_context(context: &TestContext, action: &str) {
    (if action == "seed" {
        test_api::Postgres::with_setup_script(
            "src/test-api/tests/fixtures/rds_migrations",
            "src/test-api/tests/fixtures/postgres_setup.sql",
        )
    } else {
        test_api::Postgres::new("src/test-api/tests/fixtures/rds_migrations")
    })
    .set_up()
    .await;
    test_api::OpenSearch().set_up().await;
    test_api::Ses().set_up().await;
    let marker = context.test_id();
    let pool = context.postgres_pool().await.unwrap();
    sqlx::query("INSERT INTO test_items(name,value) VALUES ($1,1)")
        .bind(&marker)
        .execute(&pool)
        .await
        .unwrap();
    pool.execute("CREATE TABLE dirty_ddl (id integer)")
        .await
        .unwrap();
    let tls: bool = sqlx::query_scalar("SELECT ssl FROM pg_stat_ssl WHERE pid=pg_backend_pid()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(tls);
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    let index = context.product_listings_index().unwrap();
    test_api::get_opensearch_client()
        .await
        .index(opensearch::IndexParts::IndexId(&index, &marker))
        .body(serde_json::json!({"productListingId":marker}))
        .send()
        .await
        .unwrap()
        .error_for_status_code()
        .unwrap();
    test_api::refresh_index(&index).await;
    let queues = test_api::WorkerSqs::new("marketing-consent-sync", 330).queues();
    queues.set_up().await;
    test_api::get_sqs_client()
        .await
        .send_message()
        .queue_url(queues.queue_url())
        .message_body(&marker)
        .message_group_id("scope")
        .message_deduplication_id(&marker)
        .send()
        .await
        .unwrap();
    let bucket = context.bucket_name("objects").await.unwrap();
    let s3 = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::config::Builder::from(test_api::aws::get_aws_config().await)
            .force_path_style(true)
            .build(),
    );
    s3.create_bucket()
        .bucket(&bucket)
        .create_bucket_configuration(
            aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(aws_sdk_s3::types::BucketLocationConstraint::EuCentral1)
                .build(),
        )
        .send()
        .await
        .unwrap();
    s3.put_object()
        .bucket(&bucket)
        .key("marker")
        .body(aws_sdk_s3::primitives::ByteStream::from(
            marker.clone().into_bytes(),
        ))
        .send()
        .await
        .unwrap();
    let source = context.sender_email().unwrap();
    let configuration_set = context.configuration_set_name("mail").await.unwrap();
    test_api::get_ses_client()
        .await
        .create_configuration_set()
        .configuration_set_name(&configuration_set)
        .send()
        .await
        .unwrap();
    send_email(&source, &marker).await;
    let secondary_source = context.register_sender("secondary").await.unwrap();
    let config_path = PathBuf::from(std::env::var_os("AURA_TEST_ENV_CONFIG").unwrap());
    let manifest_path = config_path
        .parent()
        .unwrap()
        .join(format!("slot-{}.json", context.slot()));
    let manifest: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(manifest_path).await.unwrap()).unwrap();
    assert!(
        manifest["resources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|resource| resource["kind"] == "SENDER" && resource["name"] == secondary_source),
        "labeled sender ownership must be durable before identity creation"
    );
    test_api::verify_sender_identity(&secondary_source).await;
    send_email(&secondary_source, "secondary marker").await;
    let secondary_messages = test_api::get_sent_emails_from(&secondary_source).await;
    assert_eq!(1, secondary_messages.len());
    assert_eq!("secondary marker", secondary_messages[0].subject);
    use std::sync::atomic::Ordering;
    assert!(context.api_state::<std::sync::Arc<ApiState>>().is_err());
    let state = std::sync::Arc::new(ApiState {
        marker: marker.clone(),
        starts: 0.into(),
        factory_pointer: 0.into(),
    });
    context.insert_api_state(state.clone()).unwrap();
    assert!(
        context.insert_api_state(state.clone()).is_err(),
        "duplicate state must not replace the factory's Arc"
    );
    let api = &API;
    tokio::join!(api.set_up(), api.set_up());
    assert_eq!(
        1,
        state.starts.load(Ordering::SeqCst),
        "concurrent setup must construct one server"
    );
    assert_eq!(
        std::sync::Arc::as_ptr(&state) as usize,
        state.factory_pointer.load(Ordering::SeqCst),
        "factory on the OS thread must receive the identical Arc"
    );
    let response = reqwest::get(format!("{}/marker", API.base_url()))
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(
        marker, response,
        "server factory on another OS thread must use the same context"
    );
    let output = PathBuf::from(std::env::var_os("AURA_CONTEXT_OUTPUT").unwrap());
    let report = serde_json::json!({"run":context.run_id(),"slot":context.slot(),"test":marker,"database":database,
            "index":index,"queue":queues.queue_url(),"bucket":bucket,"sender":source,"secondary_sender":secondary_source,"configuration_set":configuration_set});
    tokio::fs::write(&output, serde_json::to_vec(&report).unwrap())
        .await
        .unwrap();
    if std::env::var("AURA_CONTEXT_FAIL_FIRST_ATTEMPT")
        .ok()
        .as_deref()
        == Some("1")
        && std::env::var("NEXTEST_ATTEMPT").ok().as_deref() == Some("1")
    {
        panic!("intentional first-attempt recovery probe");
    }
    if action == "dirty" {
        std::future::pending::<()>().await;
    }
    let release = PathBuf::from(std::env::var_os("AURA_CONTEXT_RELEASE").unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    while !tokio::fs::try_exists(&release).await.unwrap() {
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM test_items")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        if action == "seed" { 2 } else { 1 },
        count,
        "seeds must be restored exactly once and other slots must not contribute rows"
    );
    let parameters =
        std::collections::HashMap::from([("search_pipeline", "hybrid-search-pipeline")]);
    let response=test_api::get_opensearch_client().await.send(opensearch::http::Method::Post,&format!("{index}/_search"),
            opensearch::http::headers::HeaderMap::new(),Some(&parameters),Some(opensearch::http::request::JsonBody::new(
                serde_json::json!({"query":{"hybrid":{"queries":[{"match_all":{}},{"match_all":{}}]}}}))),None).await.unwrap()
            .error_for_status_code().unwrap().json::<serde_json::Value>().await.unwrap();
    assert_eq!(1, response["hits"]["total"]["value"]);
    assert_eq!(marker, response["hits"]["hits"][0]["_id"]);
    let messages = test_api::get_sqs_client()
        .await
        .receive_message()
        .queue_url(queues.queue_url())
        .wait_time_seconds(0)
        .send()
        .await
        .unwrap();
    assert_eq!(1, messages.messages().len());
    assert_eq!(Some(marker.as_str()), messages.messages()[0].body());
    assert_eq!(
        marker.as_bytes(),
        s3.get_object()
            .bucket(&bucket)
            .key("marker")
            .send()
            .await
            .unwrap()
            .body
            .collect()
            .await
            .unwrap()
            .into_bytes()
            .as_ref()
    );
    let emails = test_api::get_sent_emails().await;
    assert_eq!(1, emails.len());
    assert_eq!(marker, emails[0].subject);
}

async fn assert_recovered(previous: &serde_json::Value) {
    let pool = test_api::get_postgres_client().await;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname=$1)")
            .bind(previous["database"].as_str().unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    pool.close().await;
    assert!(!exists, "previous database generation survives recovery");
    let index = previous["index"].as_str().unwrap();
    let response = test_api::get_opensearch_client()
        .await
        .indices()
        .exists(opensearch::indices::IndicesExistsParts::Index(&[index]))
        .send()
        .await
        .unwrap();
    assert_eq!(404, response.status_code().as_u16());
    assert!(
        test_api::get_sqs_client()
            .await
            .get_queue_attributes()
            .queue_url(previous["queue"].as_str().unwrap())
            .send()
            .await
            .is_err()
    );
    assert!(
        test_api::get_sent_emails_from(previous["sender"].as_str().unwrap())
            .await
            .is_empty()
    );
    assert!(
        test_api::get_sent_emails_from(previous["secondary_sender"].as_str().unwrap())
            .await
            .is_empty()
    );
    for sender in ["sender", "secondary_sender"] {
        assert!(
            test_api::get_ses_client()
                .await
                .get_email_identity()
                .email_identity(previous[sender].as_str().unwrap())
                .send()
                .await
                .is_err(),
            "owned sender identity must be removed during recovery"
        );
    }
    let s3 = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::config::Builder::from(test_api::aws::get_aws_config().await)
            .force_path_style(true)
            .build(),
    );
    assert!(
        s3.head_bucket()
            .bucket(previous["bucket"].as_str().unwrap())
            .send()
            .await
            .is_err()
    );
}

#[tokio::test]
async fn mail_sentinel_child() {
    let Ok(action) = std::env::var("AURA_CONTEXT_SENTINEL") else {
        return;
    };
    let config = test_api::environment::managed_environment().unwrap();
    test_api::aws::get_aws_emulator(&["sesv2"], &[]).await;
    let sender = format!("a{}-sentinel@example.test", config.run_id());
    if action == "create" {
        test_api::verify_sender_identity(&sender).await;
        send_email(&sender, "other test's sentinel").await;
    } else {
        let messages = test_api::get_sent_emails_from(&sender).await;
        assert_eq!(
            1,
            messages.len(),
            "scoped cleanup deleted another owner's capture"
        );
        let url = format!("{}/_aws/ses", test_api::aws::get_endpoint_url());
        reqwest::Client::new()
            .delete(url)
            .query(&[("id", &messages[0].id)])
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        test_api::get_ses_client()
            .await
            .delete_email_identity()
            .email_identity(sender)
            .send()
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn reset_benchmark_child() {
    if std::env::var_os("AURA_CONTEXT_BENCHMARK").is_none() {
        return;
    }
    let fixture = test_api::Postgres::new("migrations");
    let mut session = TestContext::acquire_explicit("reset benchmark", Isolation::Exclusive)
        .await
        .unwrap();
    session.context().scope(fixture.set_up()).await;
    let pool = session.context().postgres_pool().await.unwrap();
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename FROM pg_tables WHERE schemaname='public' ORDER BY tablename",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let table_list = tables
        .iter()
        .map(|table| format!("\"{}\"", table.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(",");
    let mut truncate = vec![];
    for _ in 0..3 {
        let started = std::time::Instant::now();
        pool.execute(sqlx::AssertSqlSafe(format!(
            "TRUNCATE TABLE {table_list} RESTART IDENTITY CASCADE"
        )))
        .await
        .unwrap();
        truncate.push(started.elapsed().as_secs_f64());
    }
    pool.close().await;
    let mut recreate = vec![];
    for _ in 0..3 {
        let started = std::time::Instant::now();
        session.finish().await.unwrap();
        session = TestContext::acquire_explicit("reset benchmark reuse", Isolation::Exclusive)
            .await
            .unwrap();
        session.context().scope(fixture.set_up()).await;
        recreate.push(started.elapsed().as_secs_f64());
    }
    session.finish().await.unwrap();
    println!(
        "AURA_CONTEXT_RESET_BENCHMARK {}",
        serde_json::json!({"public_tables":tables.len(),"truncate_seconds":truncate,"recreate_seconds":recreate,
        "choice":"RECREATE","reason":"restores DDL and seeds and fences old writers"})
    );
}

#[test_api::aura_integration_test(services=[],isolated=true)]
async fn should_bind_context_to_macro_body(context: TestContext) {
    assert_eq!(context.test_id(), TestContext::current().unwrap().test_id());
}

struct ReadyService;
struct FailedSetup;
#[async_trait::async_trait]
impl IntegrationTestService for ReadyService {
    fn service_names(&self) -> &'static [&'static str] {
        &[]
    }
    async fn set_up(&self) {}
    async fn tear_down(&self) {
        panic!("cleanup panic");
    }
}
#[async_trait::async_trait]
impl IntegrationTestService for FailedSetup {
    fn service_names(&self) -> &'static [&'static str] {
        &[]
    }
    async fn set_up(&self) {
        panic!("setup canary");
    }
}
#[test_api::aura_integration_test(services=[ReadyService,FailedSetup],isolated=true)]
#[should_panic(expected = "setup canary")]
async fn should_preserve_setup_panic_when_service_cleanup_also_panics() {}

#[test_api::aura_integration_test(services=[ReadyService],isolated=true)]
#[should_panic(expected = "body canary")]
async fn should_preserve_body_panic_when_service_cleanup_also_panics() {
    panic!("body canary");
}

#[tokio::test]
async fn unmanaged_context_child() {
    if std::env::var_os("AURA_CONTEXT_UNMANAGED").is_none() {
        return;
    }
    assert!(
        test_api::environment::managed_environment().is_none(),
        "unmanaged compatibility phase must have its own fixtures"
    );
    test_api::aws::get_aws_emulator(&["opensearch", "sqs", "s3", "sesv2"], &[]).await;
    for _ in 0..2 {
        let session = TestContext::acquire_explicit("unmanaged context reuse", Isolation::Isolated)
            .await
            .unwrap();
        session
            .context()
            .scope(async {
                test_api::Postgres::new("src/test-api/tests/fixtures/rds_migrations")
                    .set_up()
                    .await;
                test_api::OpenSearch().set_up().await;
                test_api::Ses().set_up().await;
                let pool = test_api::get_postgres_client().await;
                assert_eq!(
                    0,
                    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM test_items")
                        .fetch_one(&pool)
                        .await
                        .unwrap()
                );
                pool.execute("CREATE TABLE cancelled_test_ddl(id integer)")
                    .await
                    .unwrap();
                let context = TestContext::current().unwrap();
                let index = context.product_listings_index().unwrap();
                test_api::get_opensearch_client()
                    .await
                    .index(opensearch::IndexParts::IndexId(&index, "own"))
                    .body(serde_json::json!({"productListingId":context.test_id()}))
                    .send()
                    .await
                    .unwrap()
                    .error_for_status_code()
                    .unwrap();
                let queues = test_api::WorkerSqs::new("marketing-consent-sync", 330).queues();
                queues.set_up().await;
                send_email(&context.sender_email().unwrap(), &context.test_id()).await;
                assert_eq!(1, test_api::get_sent_emails().await.len());
            })
            .await;
        session.finish().await.unwrap();
    }
}

#[tokio::test]
async fn cleanup_failure_child() {
    if std::env::var_os("AURA_CONTEXT_CLEANUP_FAILURE").is_none() {
        return;
    }
    let session = TestContext::acquire_explicit("resource cleanup failure", Isolation::Isolated)
        .await
        .unwrap();
    let context = session.context().clone();
    test_api::aws::get_aws_emulator(&["sqs"], &[]).await;
    let queue = context.queue_name("survivor", false).await.unwrap();
    let client = test_api::get_sqs_client().await;
    let queue_url = client
        .create_queue()
        .queue_name(&queue)
        .send()
        .await
        .unwrap()
        .queue_url
        .unwrap();
    context
        .scope(test_api::Postgres::new("src/test-api/tests/fixtures/rds_migrations").set_up())
        .await;
    let pool = context.postgres_pool().await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    pool.close().await;
    let administrator = test_api::get_postgres_client().await;
    administrator
        .execute(sqlx::AssertSqlSafe(format!(
            "ALTER DATABASE \"{database}\" WITH IS_TEMPLATE true"
        )))
        .await
        .unwrap();
    let slot = context.slot();
    assert!(
        session.finish().await.is_err(),
        "injected template protection must fail database removal"
    );
    let queue_result = client
        .get_queue_attributes()
        .queue_url(queue_url)
        .send()
        .await;
    administrator
        .execute(sqlx::AssertSqlSafe(format!(
            "ALTER DATABASE \"{database}\" WITH IS_TEMPLATE false"
        )))
        .await
        .unwrap();
    let next = TestContext::acquire_explicit("resource cleanup recovery", Isolation::Isolated)
        .await
        .unwrap();
    assert_eq!(slot, next.context().slot());
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname=$1)")
            .bind(&database)
            .fetch_one(&administrator)
            .await
            .unwrap();
    next.finish().await.unwrap();
    administrator.close().await;
    assert!(
        !exists,
        "the next holder must recover the retained database manifest"
    );
    assert!(
        queue_result.is_err(),
        "one failed database removal must not skip deleting the owned queue"
    );
}
