use super::check_opensearch;
use aura_historia_worker::WorkerStartupConfig;
use std::process::Command;

#[path = "tls_test_fixture.rs"]
mod fixture;
use fixture::{Fixture, IDENTITY, Outcome, OwnedChild, REQUEST_BOUND, TestResult};

const CHILD: &str = "preflight::opensearch_tls_tests::tls_child";
const CA: &str = "OPENSEARCH_SSL_ROOT_CERT";

fn exercise(
    stage: &str,
    sdk: bool,
    trusted: bool,
    hostname: bool,
    identity: &'static str,
) -> TestResult {
    let fixture = Fixture::new()?;
    fixture.trust(trusted)?;
    let server = fixture.listen(if sdk { "HEAD" } else { "GET" }, identity)?;
    let success = trusted && hostname && (sdk || identity == IDENTITY);
    let child = OwnedChild::run(
        Command::new(std::env::current_exe()?)
            .args(["--exact", CHILD, "--ignored", "--test-threads=1"])
            .env_clear()
            .env("STAGE", stage)
            .env("COMMIT_SHA", "14fa8e7d80841d34ce69dd51e77a98e51a213c67")
            .env("AURA_HISTORIA_WORKER_SCOPE", "search-filter-projection")
            .env("AWS_REGION", "eu-central-1")
            .env("AURA_HISTORIA_WORKER_QUEUE_URL", format!("https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-search-filter-projection-{stage}"))
            .env("POSTGRES_SSL_MODE", "verify-full")
            .env("POSTGRES_HOST", "127.0.0.1")
            .env("POSTGRES_PORT", "1")
            .env("POSTGRES_DATABASE", "database_canary")
            .env("POSTGRES_USERNAME", "username_canary")
            .env("POSTGRES_PASSWORD", "password_canary")
            .env("POSTGRES_SSL_ROOT_CERT", concat!(env!("CARGO_MANIFEST_DIR"), "/src/postgres-test-ca.crt"))
            .env("OPENSEARCH_ENDPOINT_URL", server.endpoint(hostname))
            .env("OPENSEARCH_USERNAME", "username_canary")
            .env("OPENSEARCH_PASSWORD", "password_canary")
            .env(CA, &fixture.ca_path)
            .env("TLS_TEST_SDK", if sdk { "1" } else { "0" })
            .env("TLS_TEST_SUCCESS", if success { "1" } else { "0" }),
    );
    // Join even when the child fails; no detached listener or raw child output.
    let outcome = server.finish();
    child?;
    assert_eq!(
        outcome?,
        if trusted && hostname {
            Outcome::Responded
        } else {
            Outcome::CertificateRejected
        }
    );
    fixture.finish()
}

#[rstest::rstest]
#[case::frozen_ca(true, true)]
#[case::wrong_ca(false, true)]
#[case::wrong_hostname(true, false)]
fn should_verify_tls_through_actual_worker_constructors(
    #[values("dev", "prod")] stage: &str,
    #[values(false, true)] sdk: bool,
    #[case] trusted: bool,
    #[case] hostname: bool,
) -> TestResult {
    exercise(stage, sdk, trusted, hostname, IDENTITY)
}

#[rstest::rstest]
fn should_reject_wrong_identity_after_successful_preflight_tls(
    #[values("dev", "prod")] stage: &str,
) -> TestResult {
    exercise(
        stage,
        false,
        true,
        true,
        r#"{"version":{"distribution":"opensearch","number":"4.0.0"}}"#,
    )
}

#[tokio::test]
#[ignore = "parent-owned loopback TLS child; never run directly"]
async fn tls_child() -> TestResult {
    // Only parse config and invoke these two search-only functions. Never call
    // preflight::check/run/composition, which would initialize PG, SQS or domain work.
    let startup = WorkerStartupConfig::from_env().map_err(|_| "worker config parsing failed")?;
    let search = startup.opensearch().ok_or("worker search config missing")?;
    let ca_path = std::env::var(CA)?;
    std::fs::write(ca_path, include_bytes!("postgres-test-ca.crt"))?;
    let success = std::env::var("TLS_TEST_SUCCESS")? == "1";
    if std::env::var("TLS_TEST_SDK")? == "1" {
        // Construction happens after replacement: success proves frozen bytes,
        // not a transport built earlier or a second read of the original CA file.
        let client =
            crate::opensearch_client(search).map_err(|_| "worker SDK construction failed")?;
        let result = tokio::time::timeout(REQUEST_BOUND, client.ping().send())
            .await
            .map_err(|_| "worker SDK request timed out")?;
        if success {
            assert!(
                result
                    .map_err(|_| "worker SDK request failed")?
                    .status_code()
                    .is_success()
            );
        } else {
            assert!(result.is_err(), "worker SDK accepted invalid TLS peer");
        }
    } else {
        let result = tokio::time::timeout(REQUEST_BOUND, check_opensearch(search))
            .await
            .map_err(|_| "worker preflight request timed out")?;
        if success {
            assert!(result.is_ok(), "worker preflight rejected OpenSearch 3.1.0");
        } else {
            assert!(matches!(
                result,
                Err(crate::MainError::OpenSearchCompatibility)
            ));
        }
    }
    Ok(())
}
