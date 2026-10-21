use test_api::aws::{get_aws_emulator, get_endpoint_url};
use test_api::{IntegrationTestService, OpenSearch};

#[tokio::test]
#[serial_test::serial]
async fn should_expose_test_host_and_port() {
    let container = get_aws_emulator(&["opensearch"], &[]).await;

    let host_ip = container.get_host().await.unwrap().to_string();
    let host_port = container.get_host_port_ipv4(4566).await.unwrap();

    assert_eq!(host_ip, "localhost");
    assert!(host_port > 0, "a random port should be assigned");

    assert_eq!(format!("http://{host_ip}:{host_port}"), get_endpoint_url());
}

#[tokio::test]
#[serial_test::serial]
async fn should_reap_child_resources_without_removing_another_process_fixture() {
    test_api::aws::get_aws_emulator(&["opensearch"], &[]).await;
    OpenSearch().set_up().await;

    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "aws_fixture_child", "--test-threads=1"])
        .env("AURA_AWS_FIXTURE_CHILD", "1")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let namespace = format!("aura-historia-aws-backend-floci-test-{}", child.id());
    assert!(child.wait().unwrap().success());
    let emulator = std::process::Command::new("docker")
        .args(["inspect", &namespace])
        .output()
        .unwrap();
    assert!(!emulator.status.success(), "child emulator must be removed");
    for args in [
        vec!["ps", "-aq", "--filter"],
        vec!["volume", "ls", "-q", "--filter"],
    ] {
        let output = std::process::Command::new("docker")
            .args(args)
            .arg(format!("label=floci_namespace={namespace}"))
            .output()
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
