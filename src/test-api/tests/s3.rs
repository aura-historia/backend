use test_api::*;

#[aura_integration_test(services = [S3()], isolated = true)]
#[ignore = "fixture self-test; run explicitly with --ignored"]
async fn should_run_without_errors() {}
