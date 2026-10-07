use crate::IntegrationTestService;
use async_trait::async_trait;

/// Starts S3 in LocalStack for integration tests. Tests create their own fixtures.
pub struct S3();

#[async_trait]
impl IntegrationTestService for S3 {
    fn service_names(&self) -> &'static [&'static str] {
        &["s3"]
    }

    async fn set_up(&self) {}
}
