//! Reusable ingestion publication and source configuration composition.
use listing_source_postgres::SqlxListingSourceRepositoryFactory;
use partnership_postgres::SqlxListingSourceAuthorization;
use partnership_service::use_cases::commands::put_listing_source_ingestion_configuration::{
    PutListingSourceIngestionConfigurationHandler, PutListingSourceIngestionConfigurationUseCase,
};
use platform_postgres::SqlxUnitOfWork;
use product_listing_ingestion_sqs::{
    ScopedSqsProductListingIngestionPublisher, SqsProductListingIngestionPublisher,
};
use product_listing_service::use_cases::{
    SubmitPartnerProductListingIngestionHandler, SubmitPartnerProductListingIngestionUseCase,
};
use sqlx::PgPool;
use std::sync::Arc;

pub use product_listing_ingestion_sqs::with_publication_deadline;

#[derive(Clone)]
pub struct PartnerProductListingIngestion {
    pub submit: Arc<dyn SubmitPartnerProductListingIngestionUseCase>,
    pub put_configuration: Arc<dyn PutListingSourceIngestionConfigurationUseCase>,
}

pub struct ProductListingIngestionDependencies {
    submit: Arc<dyn SubmitPartnerProductListingIngestionUseCase>,
}
impl ProductListingIngestionDependencies {
    /// Compose against the application publisher port, including controlled local publishers.
    pub fn new(
        publisher: impl product_listing_service::ports::ProductListingIngestionPublisher + 'static,
    ) -> Self {
        Self {
            submit: Arc::new(SubmitPartnerProductListingIngestionHandler::new(publisher)),
        }
    }

    /// Reuse the application's AWS client configuration; requests install their own deadline.
    pub fn from_sqs(client: aws_sdk_sqs::Client, queue_url: String) -> Self {
        Self::new(ScopedSqsProductListingIngestionPublisher::new(
            SqsProductListingIngestionPublisher::new(client, queue_url),
        ))
    }
    /// Called only when an ingestion family is selected. Client construction performs no queue IO.
    pub async fn from_env() -> Result<Self, ProductListingIngestionConfigError> {
        let queue_url = queue_url(std::env::var("PRODUCT_LISTING_INGESTION_QUEUE_URL").ok())?;
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .load()
            .await;
        Ok(Self::from_sqs(aws_sdk_sqs::Client::new(&config), queue_url))
    }
}
impl PartnerProductListingIngestion {
    pub(super) fn compose(
        pool: &PgPool,
        dependencies: ProductListingIngestionDependencies,
    ) -> Self {
        Self {
            submit: dependencies.submit,
            put_configuration: Arc::new(PutListingSourceIngestionConfigurationHandler::new(
                SqlxUnitOfWork::new(pool.clone()),
                SqlxListingSourceRepositoryFactory::new(),
                SqlxListingSourceAuthorization::new(pool.clone()),
            )),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingIngestionConfigError {
    #[error("missing PRODUCT_LISTING_INGESTION_QUEUE_URL")]
    MissingQueueUrl,
    #[error("invalid product listing ingestion FIFO queue URL")]
    InvalidQueueUrl,
}
fn queue_url(value: Option<String>) -> Result<String, ProductListingIngestionConfigError> {
    let value = value.ok_or(ProductListingIngestionConfigError::MissingQueueUrl)?;
    if !url::Url::parse(&value).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.query().is_none()
            && url.fragment().is_none()
            && url
                .path_segments()
                .and_then(|mut parts| parts.next_back())
                .is_some_and(|name| !name.is_empty() && name.ends_with(".fifo"))
    }) {
        return Err(ProductListingIngestionConfigError::InvalidQueueUrl);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_validate_queue_configuration_without_disclosing_its_value() {
        assert!(matches!(
            queue_url(None),
            Err(ProductListingIngestionConfigError::MissingQueueUrl)
        ));
        for value in [
            "",
            "not-a-url",
            "https://sqs.example.test/queue",
            "https://sqs.example.test/queue.fifo?secret=value",
            "https://sqs.example.test/queue.fifo#secret",
        ] {
            let error = queue_url(Some(value.into())).unwrap_err();
            assert_eq!(
                "invalid product listing ingestion FIFO queue URL",
                error.to_string()
            );
        }
        assert_eq!(
            "http://127.0.0.1:9/fixture.fifo",
            queue_url(Some("http://127.0.0.1:9/fixture.fifo".into())).unwrap()
        );
    }
    #[tokio::test]
    async fn should_reuse_warm_ingestion_handles_without_queue_or_database_io() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://fixture:fixture@127.0.0.1:9/fixture")
            .unwrap();
        let runtime = crate::Runtime::from_pool(pool);
        let dependencies = || {
            let config = aws_sdk_sqs::config::Builder::new()
                .behavior_version_latest()
                .region(aws_sdk_sqs::config::Region::new("eu-central-1"))
                .credentials_provider(aws_sdk_sqs::config::Credentials::new(
                    "fixture", "fixture", None, None, "fixture",
                ))
                .build();
            ProductListingIngestionDependencies::from_sqs(
                aws_sdk_sqs::Client::from_conf(config),
                "http://127.0.0.1:9/fixture.fifo".into(),
            )
        };
        let first = runtime.partner_product_listing_ingestion(dependencies());
        let second = runtime.partner_product_listing_ingestion(dependencies());
        assert!(Arc::ptr_eq(&first.submit, &second.submit));
        assert!(Arc::ptr_eq(
            &first.put_configuration,
            &second.put_configuration
        ));
    }
}

#[cfg(test)]
mod environment_tests {
    #[test]
    fn should_construct_environment_dependencies_offline_with_isolated_fake_credentials() {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "partner_product_listing_ingestion::environment_tests::environment_probe",
                "--ignored",
                "--nocapture",
            ])
            .env_clear()
            .env(
                "PRODUCT_LISTING_INGESTION_QUEUE_URL",
                "http://127.0.0.1:9/fixture.fifo",
            )
            .env("AWS_REGION", "eu-central-1")
            .env("AWS_ACCESS_KEY_ID", "fixture")
            .env("AWS_SECRET_ACCESS_KEY", "fixture")
            .env("AWS_EC2_METADATA_DISABLED", "true");
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed"));
    }
    #[tokio::test]
    #[ignore = "invoked in the isolated fake AWS environment"]
    async fn environment_probe() {
        super::ProductListingIngestionDependencies::from_env()
            .await
            .unwrap();
    }
}
