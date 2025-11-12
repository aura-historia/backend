use common::product_id::{ProductId, ProductKey};
use product::dynamodb::{
    product_update_record::ProductRecordUpdate, repository::ProductDynamoDbRepository,
};
use product::opensearch::{
    product_update_document::ProductUpdateDocument, repository::ProductOpenSearchRepository,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tracing::{error, info, warn};

#[async_trait::async_trait]
#[mockall::automock]
pub trait EnrichmentPipeSink {
    async fn drain_documents(
        &self,
        documents: HashMap<ProductId, ProductUpdateDocument>,
    ) -> HashSet<ProductId>;

    async fn drain_records(
        &self,
        records: HashMap<ProductId, (ProductKey, ProductRecordUpdate)>,
    ) -> HashSet<ProductId>;
}

pub struct EnrichmentPipeSinkImpl {
    item_dynamodb_repository: Arc<dyn ProductDynamoDbRepository + Send + Sync>,
    item_opensearch_repository: Arc<dyn ProductOpenSearchRepository + Send + Sync>,
}

impl EnrichmentPipeSinkImpl {
    pub fn new(
        item_dynamodb_repository: Arc<dyn ProductDynamoDbRepository + Send + Sync>,
        item_opensearch_repository: Arc<dyn ProductOpenSearchRepository + Send + Sync>,
    ) -> Self {
        Self {
            item_dynamodb_repository,
            item_opensearch_repository,
        }
    }
}

#[async_trait::async_trait]
impl EnrichmentPipeSink for EnrichmentPipeSinkImpl {
    async fn drain_documents(
        &self,
        documents: HashMap<ProductId, ProductUpdateDocument>,
    ) -> HashSet<ProductId> {
        if documents.is_empty() {
            return HashSet::new();
        }

        let count = documents.len();
        let item_ids = documents.keys().copied().collect::<HashSet<_>>();
        let res = self
            .item_opensearch_repository
            .update_item_documents(documents)
            .await;

        let mut skipped = 0usize;
        let failures = match res {
            Err(err) => {
                error!(error = %err, "Failed draining all ProductDocument-Updates for this sink.");
                item_ids
            }
            Ok(response) => {
                if response.errors {
                    response.items.into_iter().map(|res| res.unwrap_update()).filter_map(|failure | {
                        warn!(error = ?failure.error, status = failure.status, itemId = failure.id, "Failed updating ProductDocument.");
                        match ProductId::try_from(failure.id.as_str()) {
                            Err(err) => {
                                error!(error = %err, itemId = failure.id, "Failed parsing returned '_id' from OpenSearch for 'ProductDocument' as 'ProductId'.
                                    This is highly likely to be a bug. Cannot retry.");
                                skipped += 1;
                                None
                            }
                            Ok(product_id) => Some(product_id)
                        }
                    }).collect()
                } else {
                    HashSet::new()
                }
            }
        };

        info!(
            count = count,
            successes = count - skipped - failures.len(),
            skipped = skipped,
            failures = failures.len(),
            "Drained documents."
        );

        failures
    }

    async fn drain_records(
        &self,
        records: HashMap<ProductId, (ProductKey, ProductRecordUpdate)>,
    ) -> HashSet<ProductId> {
        let count = records.len();
        let mut failures = HashSet::new();
        for (product_id, (product_key, record)) in records {
            let res = self
                .item_dynamodb_repository
                .update_item_record(&product_key.shop_id, &product_key.shops_product_id, record)
                .await;
            if let Err(err) = res {
                error!(error = ?err, itemId = %product_id, "Failed updating ProductRecord.");
                failures.insert(product_id);
            }
        }

        info!(
            count = count,
            successes = count - failures.len(),
            failures = failures.len(),
            "Drained records."
        );

        failures
    }
}

#[cfg(test)]
mod tests {
    mod drain_documents {
        use crate::pipeline::sink::{EnrichmentPipeSink, EnrichmentPipeSinkImpl};
        use common::{
            opensearch::bulk_response::{BulkItemResult, BulkOpResult, BulkResponse},
            product_id::ProductId,
        };
        use fake::{Fake, Faker};
        use product::dynamodb::repository::MockProductDynamoDbRepository;
        use product::opensearch::{
            product_update_document::ProductUpdateDocument,
            repository::MockItemOpenSearchRepository,
        };
        use std::collections::HashSet;
        use std::{collections::HashMap, sync::Arc};

        #[tokio::test]
        async fn should_drain_documents_when_successful() {
            let item_dynamodb_repository = MockProductDynamoDbRepository::default();
            let mut item_opensearch_repository = MockItemOpenSearchRepository::default();
            item_opensearch_repository
                .expect_update_item_documents()
                .return_once(|_| {
                    Box::pin(async {
                        Ok(BulkResponse {
                            took: Faker.fake(),
                            errors: false,
                            items: vec![],
                        })
                    })
                });

            let sink = EnrichmentPipeSinkImpl::new(
                Arc::new(item_dynamodb_repository),
                Arc::new(item_opensearch_repository),
            );
            let actual = sink.drain_documents(Faker.fake()).await;

            assert!(actual.is_empty());
        }

        #[rstest::rstest]
        #[case(1)]
        #[case(5)]
        #[case(10)]
        #[case(17)]
        #[case(37)]
        #[case(42)]
        #[case(69)]
        #[case(100)]
        #[case(250)]
        #[case(555)]
        #[case(999)]
        #[case(1000)]
        #[tokio::test]
        async fn should_return_partial_failures(#[case] failure_count: usize) {
            let item_dynamodb_repository = MockProductDynamoDbRepository::default();
            let mut item_opensearch_repository = MockItemOpenSearchRepository::default();
            item_opensearch_repository
                .expect_update_item_documents()
                .return_once(move |documents| {
                    Box::pin(async move {
                        Ok(BulkResponse {
                            took: Faker.fake(),
                            errors: true,
                            items: documents
                                .into_keys()
                                .take(failure_count)
                                .map(|product_id| BulkItemResult::Update {
                                    update: BulkOpResult {
                                        index: "items".to_string(),
                                        id: product_id.to_string(),
                                        version: None,
                                        status: 409,
                                        error: None,
                                    },
                                })
                                .collect(),
                        })
                    })
                });

            let sink = EnrichmentPipeSinkImpl::new(
                Arc::new(item_dynamodb_repository),
                Arc::new(item_opensearch_repository),
            );
            let input = fake::vec![(ProductId, ProductUpdateDocument); 1000]
                .into_iter()
                .collect::<HashMap<_, _>>();
            let actual = sink.drain_documents(input.clone()).await;

            assert_eq!(failure_count, actual.len());
            assert_eq!(
                input
                    .into_keys()
                    .take(failure_count)
                    .collect::<HashSet<_>>(),
                actual
            );
        }
    }

    mod drain_records {
        use crate::pipeline::sink::{EnrichmentPipeSink, EnrichmentPipeSinkImpl};
        use aws_sdk_dynamodb::{error::SdkError, operation::update_item::UpdateItemOutput};
        use common::product_id::{ProductId, ProductKey};
        use fake::{Fake, Faker};
        use product::dynamodb::{
            product_update_record::ProductRecordUpdate, repository::MockProductDynamoDbRepository,
        };
        use product::opensearch::repository::MockItemOpenSearchRepository;
        use std::collections::HashSet;
        use std::{collections::HashMap, sync::Arc};

        #[tokio::test]
        async fn should_drain_records_when_successful() {
            let item_opensearch_repository = MockItemOpenSearchRepository::default();
            let mut item_dynamodb_repository = MockProductDynamoDbRepository::default();
            item_dynamodb_repository
                .expect_update_item_record()
                .returning(|_, _, _| Box::pin(async { Ok(UpdateItemOutput::builder().build()) }));

            let sink = EnrichmentPipeSinkImpl::new(
                Arc::new(item_dynamodb_repository),
                Arc::new(item_opensearch_repository),
            );
            let actual = sink.drain_records(Faker.fake()).await;

            assert!(actual.is_empty());
        }

        #[rstest::rstest]
        #[case(1)]
        #[case(5)]
        #[case(10)]
        #[case(17)]
        #[case(37)]
        #[case(42)]
        #[case(69)]
        #[case(100)]
        #[case(250)]
        #[case(555)]
        #[case(999)]
        #[case(1000)]
        #[tokio::test]
        async fn should_return_partial_failures(#[case] failure_count: usize) {
            let item_opensearch_repository = MockItemOpenSearchRepository::default();
            let mut item_dynamodb_repository = MockProductDynamoDbRepository::default();

            item_dynamodb_repository
                .expect_update_item_record()
                .returning(|_, _, _| {
                    Box::pin(async { Err(SdkError::construction_failure("Something went wrong")) })
                });

            let sink = EnrichmentPipeSinkImpl::new(
                Arc::new(item_dynamodb_repository),
                Arc::new(item_opensearch_repository),
            );
            let input = fake::vec![(ProductId, (ProductKey, ProductRecordUpdate)); failure_count]
                .into_iter()
                .collect::<HashMap<_, _>>();
            let actual = sink.drain_records(input.clone()).await;

            assert_eq!(input.into_keys().collect::<HashSet<_>>(), actual);
        }
    }
}
