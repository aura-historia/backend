use super::{PeriodicMatchConfig, opensearch_client};
use std::{collections::BTreeMap, env::VarError};

// Reuse only fixture source, not worker runtime/library or its private implementation.
#[path = "../../../aura-historia-worker/src/tls_test_fixture.rs"]
mod fixture;
use fixture::{Fixture, IDENTITY, Outcome, REQUEST_BOUND, TestResult};

async fn exercise(trusted: bool, hostname: bool) -> TestResult {
    let fixture = Fixture::new()?;
    for stage in ["dev", "prod"] {
        fixture.trust(trusted)?;
        let server = fixture.listen("HEAD", IDENTITY)?;
        let mut values: BTreeMap<&str, String> = [
            ("STAGE", stage),
            ("POSTGRES_SSL_MODE", "verify-full"),
            ("POSTGRES_HOST", "127.0.0.1"),
            ("POSTGRES_PORT", "1"),
            ("POSTGRES_DATABASE", "database_canary"),
            ("POSTGRES_USERNAME", "username_canary"),
            ("POSTGRES_PASSWORD", "password_canary"),
            (
                "POSTGRES_SSL_ROOT_CERT",
                concat!(env!("CARGO_MANIFEST_DIR"), "/src/postgres-test-ca.crt"),
            ),
            ("OPENSEARCH_USERNAME", "username_canary"),
            ("OPENSEARCH_PASSWORD", "password_canary"),
            ("VERTEX_AI_PROJECT_ID", "project_canary"),
            ("VERTEX_AI_LOCATION", "location_canary"),
            ("VERTEX_AI_MODEL", "model_canary"),
        ]
        .map(|(key, value)| (key, value.to_owned()))
        .into();
        values.insert("OPENSEARCH_ENDPOINT_URL", server.endpoint(hostname));
        values.insert(
            "OPENSEARCH_SSL_ROOT_CERT",
            fixture
                .ca_path
                .to_str()
                .ok_or("non-Unicode fixture path")?
                .into(),
        );
        let mut ca_reads = 0;
        let config = PeriodicMatchConfig::from_lookup(&mut |key| {
            if key == "OPENSEARCH_SSL_ROOT_CERT" {
                ca_reads += 1;
            }
            values.get(key).cloned().ok_or(VarError::NotPresent)
        })
        .map_err(|_| "cron config parsing failed")?;
        assert_eq!(ca_reads, 1);
        fixture.trust(false)?;
        // Never build jobs/ADC/pools: only the production search constructor.
        let client = opensearch_client(&config).map_err(|_| "cron SDK construction failed")?;
        let result = tokio::time::timeout(REQUEST_BOUND, client.ping().send()).await;
        let outcome = server.finish()?;
        let result = result.map_err(|_| "cron SDK request timed out")?;
        if trusted && hostname {
            assert!(
                result
                    .map_err(|_| "cron SDK request failed")?
                    .status_code()
                    .is_success()
            );
            assert_eq!(outcome, Outcome::Responded);
        } else {
            assert!(result.is_err(), "cron SDK accepted invalid TLS peer");
            assert_eq!(outcome, Outcome::CertificateRejected);
        }
    }
    fixture.finish()
}

#[tokio::test]
#[ignore = "secured OpenSearch image witness"]
async fn should_ping_secured_opensearch_from_witness_environment() -> TestResult {
    let config = PeriodicMatchConfig::from_env()?;
    let client = opensearch_client(&config)?;
    let response = tokio::time::timeout(REQUEST_BOUND, client.ping().send()).await??;
    assert!(response.status_code().is_success());
    Ok(())
}

#[tokio::test]
async fn should_use_frozen_ca_in_actual_cron_sdk_for_dev_and_prod() -> TestResult {
    exercise(true, true).await
}

#[tokio::test]
async fn should_reject_wrong_ca_in_actual_cron_sdk_for_dev_and_prod() -> TestResult {
    exercise(false, true).await
}

#[tokio::test]
async fn should_reject_wrong_hostname_in_actual_cron_sdk_for_dev_and_prod() -> TestResult {
    exercise(true, false).await
}
