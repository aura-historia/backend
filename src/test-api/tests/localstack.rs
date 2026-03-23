use test_api::localstack::spin_up_localstack_with_services;

#[tokio::test]
#[serial_test::serial]
async fn should_expose_test_host_and_port() {
    let container = spin_up_localstack_with_services(&[], &[]).await;

    let host_ip = container.0.get_host().await.unwrap().to_string();
    let host_port = container.0.get_host_port_ipv4(4566).await.unwrap();

    assert_eq!(host_ip, "localhost");
    assert!(host_port > 0, "a random port should be assigned");

    drop(container);
}
