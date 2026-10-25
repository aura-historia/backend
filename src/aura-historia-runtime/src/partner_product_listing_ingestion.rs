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

#[cfg(test)]
mod tests {
    use super::*;
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
