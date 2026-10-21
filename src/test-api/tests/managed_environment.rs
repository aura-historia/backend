#![cfg(feature = "supervisor")]

use test_api::{IntegrationTestService, Postgres};

#[test]
fn should_manage_job_lifecycle_across_test_processes() {
    let result = std::process::Command::new("python3")
        .arg(concat!(env!("CARGO_WORKSPACE_DIR"), "ci/test-env-smoke.py"))
        .args(["--binary", env!("CARGO_BIN_EXE_aura-test-env")])
        .arg("--fixture-binary")
        .arg(std::env::current_exe().unwrap())
        .output()
        .expect("smoke driver should execute");
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    println!("{}", String::from_utf8_lossy(&result.stdout));
}

#[tokio::test(flavor = "current_thread")]
async fn managed_fixture_child() {
    if std::env::var_os("AURA_TEST_ENV_MANAGED").is_none() {
        return;
    }
    let postgres = Postgres::new("migrations");
    postgres.set_up().await;
    let pool = test_api::get_postgres_client().await;
    let extensions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_extension WHERE extname IN ('pg_trgm','unaccent')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(2, extensions);
    let encrypted: bool =
        sqlx::query_scalar("SELECT ssl FROM pg_stat_ssl WHERE pid=pg_backend_pid()")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(encrypted);
    let heartbeat: bool = sqlx::query_scalar(
        "SELECT has_schema_privilege('aura_replication','dms_heartbeat','CREATE')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(heartbeat);
    pool.close().await;
    postgres.tear_down().await;
}
