use std::process::Stdio;
use test_api::aws::{get_aws_emulator, get_endpoint_url};
use test_api::{IntegrationTestService, OpenSearch};
use tokio::process::Command;

#[tokio::test]
#[serial_test::serial]
async fn should_expose_test_host_and_port() {
    get_aws_emulator(&["opensearch"], &[]).await;
    let endpoint = url::Url::parse(get_endpoint_url()).unwrap();
    assert_eq!(endpoint.host_str(), Some("localhost"));
    assert!(endpoint.port().unwrap() > 0);
    aws_sdk_opensearch::Client::new(test_api::aws::get_aws_config().await)
        .list_domain_names()
        .send()
        .await
        .expect("published Floci endpoint must accept real SDK requests");
}

#[tokio::test]
#[serial_test::serial]
async fn should_reap_child_resources_without_removing_another_process_fixture() {
    test_api::aws::get_aws_emulator(&["opensearch"], &[]).await;
    OpenSearch().set_up().await;

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "aws_fixture_child", "--test-threads=1"])
        .env("AURA_AWS_FIXTURE_CHILD", "1")
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let namespace = format!(
        "aura-historia-aws-backend-floci-test-{}",
        child.id().expect("child must have an id before waiting")
    );
    assert!(child.wait().await.unwrap().success());
    let emulator = Command::new("docker")
        .args(["inspect", &namespace])
        .output()
        .await
        .unwrap();
    assert!(!emulator.status.success(), "child emulator must be removed");
    for args in [
        vec!["ps", "-aq", "--filter"],
        vec!["volume", "ls", "-q", "--filter"],
    ] {
        let output = Command::new("docker")
            .args(args)
            .arg(format!("label=floci_namespace={namespace}"))
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        assert!(output.stdout.is_empty(), "child resources must be removed");
    }
    assert!(
        test_api::get_opensearch_client()
            .await
            .ping()
            .send()
            .await
            .unwrap()
            .status_code()
            .is_success(),
        "the parent process's OpenSearch must survive child cleanup"
    );
}

#[tokio::test]
async fn aws_fixture_child() {
    if std::env::var_os("AURA_AWS_FIXTURE_CHILD").is_none() {
        return;
    }
    test_api::aws::get_aws_emulator(&["opensearch"], &[]).await;
    OpenSearch().set_up().await;
}
