use test_api::*;

#[aura_integration_test(services = [S3()])]
async fn should_run_without_errors() {}
