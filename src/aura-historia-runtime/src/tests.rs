use super::*;
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use listing_source_core::ListingSourceSlugId;
use listing_source_service::use_cases::queries::{
    get_public_listing_source_by_slug::{
        GetPublicListingSourceBySlugError, GetPublicListingSourceBySlugRequest,
    },
    search_public_listing_sources::{
        PublicListingSourceSearchQuery, SearchPublicListingSourcesError,
        SearchPublicListingSourcesRequest,
    },
};
use platform_postgres::{
    PostgresCredentialRefreshError, PostgresCredentialsProvider, VersionedPostgresCredentials,
};
use std::{
    error::Error,
    process::Command,
    sync::{Arc, Mutex},
};

type TestError = Box<dyn Error + Send + Sync>;

fn context() -> OperationContext {
    OperationContext {
        principal: Principal::Anonymous,
        request_id: RequestId::new("runtime-test"),
        correlation_id: CorrelationId::new("runtime-test"),
    }
}

fn search(query: Option<&str>) -> Result<SearchPublicListingSourcesRequest, TestError> {
    Ok(SearchPublicListingSourcesRequest::new(
        PublicListingSourceSearchQuery::new(query.map(str::to_owned))?,
        21,
        None,
    )?)
}

fn run_probe(name: &str) -> Result<(), TestError> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["--exact", name, "--ignored", "--nocapture"])
        .env_clear()
        .envs([
            ("POSTGRES_HOST", "localhost"),
            ("POSTGRES_PORT", "9"),
            ("POSTGRES_DATABASE", "fixture"),
            ("POSTGRES_USERNAME", "fixture"),
            ("POSTGRES_PASSWORD", "fixture-password"),
            ("POSTGRES_TLS_ROOT_CERT", "/fixture/ca.pem"),
        ]);
    // Keep subprocess coverage in CI's per-process output directory.
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

#[test]
fn should_start_anonymous_runtime_without_unrelated_provider_configuration() -> Result<(), TestError>
{
    run_probe("tests::anonymous_bootstrap_probe")
}

#[tokio::test]
#[ignore = "invoked by the anonymous startup test in a clean subprocess"]
async fn anonymous_bootstrap_probe() -> Result<(), TestError> {
    let native = Runtime::from_env().await?;
    let lambda = RotatingRuntime::from_env().await?;
    let lease = lambda.current().await?;
    for runtime in [&native, lease.value()] {
        let family = runtime.public_listing_sources();
        let same = runtime.public_listing_sources();
        assert!(Arc::ptr_eq(&family.search, &same.search));
        assert!(Arc::ptr_eq(&family.get_by_slug, &same.get_by_slug));
        let result = family
            .search
            .execute(&context(), search(Some("a"))?)
            .await?;
        assert!(result.items.is_empty());
        assert!(result.continuation.is_none());
        assert_eq!(0, runtime.postgres_pool().size());
        runtime.postgres_pool().close().await;
    }
    Ok(())
}

#[test]
fn should_share_warm_handlers_refresh_versions_and_fail_closed_on_refresh_failure()
-> Result<(), TestError> {
    run_probe("tests::rotation_probe")
}

#[tokio::test]
#[ignore = "invoked by the rotation test in a clean subprocess"]
async fn rotation_probe() -> Result<(), TestError> {
    let credentials = Arc::new(FixtureCredentials(Mutex::new(Some(version("version-1")?))));
    let runtime = RotatingRuntime::new(
        platform_lambda_bootstrap::LambdaPostgresConfig::from_env()?,
        credentials.clone(),
    );
    let old = runtime.current().await?;
    let same = runtime.current().await?;
    assert!(std::ptr::eq(old.value(), same.value()));
    let old_family = old.value().public_listing_sources();
    assert!(Arc::ptr_eq(
        &old_family.search,
        &same.value().public_listing_sources().search
    ));

    *credentials.0.lock().unwrap() = Some(version("version-2")?);
    let (left, right) = tokio::join!(runtime.current(), runtime.current());
    let new = left?;
    let right = right?;
    assert_eq!("version-2", new.version_id());
    assert!(std::ptr::eq(new.value(), right.value()));
    assert!(!Arc::ptr_eq(
        &old_family.search,
        &new.value().public_listing_sources().search
    ));
    assert!(!old.value().postgres_pool().is_closed());
    assert!(!new.value().postgres_pool().is_closed());

    *credentials.0.lock().unwrap() = None;
    let error = match runtime.current().await {
        Err(error) => error,
        Ok(_) => panic!("failed credential refresh must not return a stale composition"),
    };
    assert!(matches!(error, RuntimeError::CredentialsRefresh(_)));
    assert!(error.source().is_some());
    *credentials.0.lock().unwrap() = Some(version("version-2")?);
    assert!(std::ptr::eq(new.value(), runtime.current().await?.value()));
    old.value().postgres_pool().close().await;
    new.value().postgres_pool().close().await;
    Ok(())
}

fn version(version: &str) -> Result<VersionedPostgresCredentials, TestError> {
    Ok(VersionedPostgresCredentials::new(
        version.to_owned(),
        "fixture".to_owned(),
        "fixture-password".to_owned(),
    )?)
}

struct FixtureCredentials(Mutex<Option<VersionedPostgresCredentials>>);

#[async_trait::async_trait]
impl PostgresCredentialsProvider for FixtureCredentials {
    async fn current(
        &self,
    ) -> Result<VersionedPostgresCredentials, PostgresCredentialRefreshError> {
        self.0
            .lock()
            .unwrap()
            .clone()
            .ok_or(PostgresCredentialRefreshError)
    }
}

#[tokio::test]
async fn should_preserve_transaction_failure_causes_for_both_composed_use_cases()
-> Result<(), TestError> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://fixture:fixture@localhost:9/fixture")?;
    let runtime = Runtime::from_pool(pool);
    runtime.postgres_pool().close().await;
    let family = runtime.public_listing_sources();
    let search = family
        .search
        .execute(&context(), search(None)?)
        .await
        .unwrap_err();
    let detail = family
        .get_by_slug
        .execute(
            &context(),
            GetPublicListingSourceBySlugRequest {
                slug_id: ListingSourceSlugId::raw("fixture-source")?,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        search,
        SearchPublicListingSourcesError::BeginTransaction { .. }
    ));
    assert!(matches!(
        detail,
        GetPublicListingSourceBySlugError::BeginTransaction { .. }
    ));
    for error in [&search as &dyn Error, &detail as &dyn Error] {
        assert!(error.source().is_some_and(|cause| {
            cause
                .source()
                .is_some_and(|cause| cause.downcast_ref::<sqlx::Error>().is_some())
        }));
    }
    Ok(())
}
