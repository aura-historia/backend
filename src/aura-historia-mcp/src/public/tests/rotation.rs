use super::*;
use crate::{McpStartupError, RotatingMcpRouter};
use aura_historia_runtime::{ProductListingConfig, RotatingRuntime};
use platform_lambda_bootstrap::LambdaPostgresConfig;
use platform_postgres::{
    PostgresCredentialRefreshError, PostgresCredentialsProvider, VersionedPostgresCredentials,
};
use std::{
    error::Error,
    process::Command,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

type TestError = application::error::BoxError;

#[test]
fn should_refresh_credentials_for_each_public_invocation_and_reject_refresh_failure()
-> Result<(), TestError> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args([
            "--exact",
            "public::tests::rotation::rotation_probe",
            "--ignored",
            "--nocapture",
        ])
        .env_clear()
        .envs([
            ("POSTGRES_HOST", "127.0.0.1"),
            ("POSTGRES_PORT", "9"),
            ("POSTGRES_DATABASE", "fixture"),
            ("POSTGRES_TLS_ROOT_CERT", "/fixture/ca.pem"),
            ("STAGE", "ephemeral"),
            ("OPENSEARCH_ENDPOINT_URL", "http://127.0.0.1:9"),
        ]);
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    let output = command.output()?;
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed"));
    Ok(())
}

struct Credentials {
    version: Mutex<Option<&'static str>>,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl PostgresCredentialsProvider for Credentials {
    async fn current(
        &self,
    ) -> Result<VersionedPostgresCredentials, PostgresCredentialRefreshError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let version = self
            .version
            .lock()
            .unwrap()
            .ok_or(PostgresCredentialRefreshError)?;
        Ok(VersionedPostgresCredentials::new(
            version.into(),
            "fixture".into(),
            "fixture-password".into(),
        )
        .unwrap())
    }
}

#[tokio::test]
#[ignore = "run by the clean-environment credential-rotation test"]
async fn rotation_probe() -> Result<(), TestError> {
    let credentials = Arc::new(Credentials {
        version: Mutex::new(Some("version-1")),
        calls: AtomicUsize::new(0),
    });
    let runtime = RotatingRuntime::new(LambdaPostgresConfig::from_env()?, credentials.clone());
    let app = RotatingMcpRouter::new(
        runtime,
        TransportConfig {
            allowed_hosts: vec!["mcp.example.test".into()],
            ..Default::default()
        },
        ProductListingConfig::from_env()?,
    );
    let old = app.current().await?;
    let same = app.current().await?;
    assert_eq!(2, credentials.calls.load(Ordering::SeqCst));
    let (_, old_list, _) = post(old.clone(), "tools/list", json!({})).await;
    let (_, same_list, _) = post(same, "tools/list", json!({})).await;
    assert_eq!(old_list, same_list);
    assert_eq!(12, old_list["result"]["tools"].as_array().unwrap().len());

    *credentials.version.lock().unwrap() = Some("version-2");
    let (left, right) = tokio::join!(app.current(), app.current());
    let (left, right) = (left?, right?);
    assert_eq!(4, credentials.calls.load(Ordering::SeqCst));
    assert_eq!(old_list, post(left, "tools/list", json!({})).await.1);
    assert_eq!(old_list, post(right, "tools/list", json!({})).await.1);

    *credentials.version.lock().unwrap() = None;
    let error = match app.current().await {
        Err(error) => error,
        Ok(_) => panic!("failed refresh returned a cached router"),
    };
    assert!(matches!(
        error,
        McpStartupError::Runtime(aura_historia_runtime::RuntimeError::CredentialsRefresh(_))
    ));
    assert!(error.source().is_some());
    // A request already holding the old router can still complete after replacement.
    assert_eq!(old_list, post(old, "tools/list", json!({})).await.1);
    *credentials.version.lock().unwrap() = Some("version-2");
    assert_eq!(
        old_list,
        post(app.current().await?, "tools/list", json!({})).await.1
    );
    assert_eq!(6, credentials.calls.load(Ordering::SeqCst));
    Ok(())
}
