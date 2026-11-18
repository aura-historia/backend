use std::process::Stdio;
use test_api::aws::{get_aws_config, get_aws_emulator, get_endpoint_url};
use tokio::process::Command;

async fn list_buckets(expectation: &str) {
    aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::config::Builder::from(get_aws_config().await)
            .force_path_style(true)
            .build(),
    )
    .list_buckets()
    .send()
    .await
    .expect(expectation);
}

#[tokio::test]
#[ignore = "fixture self-test; run explicitly with --ignored"]
#[serial_test::serial]
async fn should_expose_test_host_and_port() {
    require_unmanaged_phase();
    get_aws_emulator(&["s3"], &[]).await;
    let endpoint = url::Url::parse(get_endpoint_url()).unwrap();
    assert_eq!(endpoint.host_str(), Some("localhost"));
    assert!(endpoint.port().unwrap() > 0);
    list_buckets("published Floci endpoint must accept real SDK requests").await;
}

#[tokio::test]
#[ignore = "fixture self-test; run explicitly with --ignored"]
#[serial_test::serial]
async fn should_remove_a_child_process_fixture_without_removing_its_parent() {
    require_unmanaged_phase();
    get_aws_emulator(&["s3"], &[]).await;

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "aws_fixture_child",
            "--test-threads=1",
        ])
        .env("AURA_AWS_FIXTURE_CHILD", "1")
        .env_remove("AURA_TEST_ENV_MANAGED")
        .env_remove("AURA_TEST_ENV_CONFIG")
        .env_remove("NEXTEST")
        .env_remove("NEXTEST_RUN_ID")
        .env_remove("NEXTEST_TEST_GLOBAL_SLOT")
        .env_remove("NEXTEST_EXECUTION_MODE")
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let child_emulator = format!(
        "aura-historia-aws-backend-floci-test-{}",
        child.id().expect("child must have an id before waiting")
    );
    assert!(child.wait().await.unwrap().success());
    let emulator = Command::new("docker")
        .args(["inspect", &child_emulator])
        .output()
        .await
        .unwrap();
    assert!(!emulator.status.success(), "child emulator must be removed");
    list_buckets("the parent process's emulator must survive child cleanup").await;
}

#[tokio::test]
#[ignore = "fixture self-test; run explicitly with --ignored"]
async fn aws_fixture_child() {
    if std::env::var_os("AURA_AWS_FIXTURE_CHILD").is_none() {
        return;
    }
    require_unmanaged_phase();
    get_aws_emulator(&["s3"], &[]).await;
    list_buckets("child emulator must accept requests").await;
}

fn require_unmanaged_phase() {
    assert!(
        test_api::environment::managed_environment().is_none(),
        "AWS fixture lifecycle diagnostics require an unmanaged local run"
    );
}
