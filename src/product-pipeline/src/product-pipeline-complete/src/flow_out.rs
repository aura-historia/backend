use common::{batch::Batch, product_id::ProductId};
use product::{
    dynamodb::repository::ProductDynamoDbRepository,
    opensearch::{
        product_update_document::ProductUpdateDocument, repository::ProductOpenSearchRepository,
    },
};
use product_pipeline_common::{
    flow_out::{FlowOutResult, PipeFlowOut},
    types::CompletedPipeProduct,
};
use std::collections::{HashMap, HashSet};
use tracing::{error, warn};

pub struct PersistDynamoDbOpenSearchPipeFlowOutImpl<'a> {
    dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Send + Sync),
    opensearch_repository: &'a (dyn ProductOpenSearchRepository + Send + Sync),
}

impl<'a> PersistDynamoDbOpenSearchPipeFlowOutImpl<'a> {
    pub fn new(
        dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Send + Sync),
        opensearch_repository: &'a (dyn ProductOpenSearchRepository + Send + Sync),
    ) -> Self {
        Self {
            dynamodb_repository,
            opensearch_repository,
        }
    }
}

#[async_trait::async_trait]
impl<'a> PipeFlowOut<'a, CompletedPipeProduct> for PersistDynamoDbOpenSearchPipeFlowOutImpl<'a> {
    async fn flow_out(
        &'a self,
        completed_pipe_products: Vec<CompletedPipeProduct>,
    ) -> FlowOutResult {
        let mut total_successes = HashSet::with_capacity(completed_pipe_products.len());
        let mut total_failures = HashSet::new();

        for batch in Batch::chunked_from(completed_pipe_products.into_iter()) {
            let batch: Batch<CompletedPipeProduct, 200> = batch;
            let batch_product_ids = batch
                .iter()
                .map(|product| product.product_id)
                .collect::<HashSet<_>>();
            let mut dynamodb_succeses = HashSet::with_capacity(200);
            let mut dynamodb_failures = HashSet::new();

            // Single DynamoDB-Updates
            for product in batch.iter() {
                let update_res = self
                    .dynamodb_repository
                    .update_product_record(
                        &product.shop_id,
                        &product.shops_product_id,
                        product.clone().into(),
                    )
                    .await;
                match update_res {
                    Ok(_) => {
                        dynamodb_succeses.insert(product.product_id);
                    }
                    Err(err) => {
                        warn!(error = ?err, productId = %product.product_id, "Failed updating DynamoDB ProductRecord.");
                        dynamodb_failures.insert(product.product_id);
                    }
                }
            }

            // Batched OpenSearch-Updates
            let eligible_os_updates = batch
                .into_iter()
                .filter(|product| !dynamodb_failures.contains(&product.product_id))
                .map(|product| (product.product_id, ProductUpdateDocument::from(product)))
                .collect::<HashMap<_, _>>();

            if eligible_os_updates.is_empty() {
                total_successes.extend(dynamodb_succeses);
                total_failures.extend(dynamodb_failures);
                continue;
            }

            let os_res = self
                .opensearch_repository
                .update_product_documents(eligible_os_updates)
                .await;
            match os_res {
                Ok(bulk_response) => {
                    if bulk_response.errors {
                        for reponse_item in bulk_response.items {
                            let update_res = reponse_item.unwrap_update();
                            match ProductId::try_from(update_res.id) {
                                Ok(product_id) => match update_res.error {
                                    None => {
                                        total_successes.insert(product_id);
                                    }
                                    Some(err) => {
                                        warn!(error = ?err, productId = %product_id, "Partially failed updating OpenSearch ProductDocument in batch.");
                                        total_failures.insert(product_id);
                                    }
                                },
                                Err(err) => {
                                    error!(error = %err, "Failed parsing String as ProductId for response in batch-update of products. Skipping.");
                                }
                            }
                        }
                    } else {
                        total_successes.extend(batch_product_ids);
                    }
                }
                Err(err) => {
                    error!(err = %err, "Failed writing entire OpenSearch Update-Batch. Failing entire PipeProduct-Completion-Step.");
                    total_failures.extend(batch_product_ids);
                }
            }
        }

        FlowOutResult {
            successes: total_successes,
            failures: total_failures,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::flow_out::PersistDynamoDbOpenSearchPipeFlowOutImpl;
    use aws_sdk_dynamodb::error::SdkError;
    use aws_sdk_dynamodb::operation::update_item::UpdateItemOutput;
    use common::opensearch::bulk_response::{
        BulkError, BulkItemResult, BulkOpResult, BulkResponse,
    };
    use fake::{Fake, Faker};
    use product::{
        dynamodb::repository::MockProductDynamoDbRepository,
        opensearch::repository::MockProductOpenSearchRepository,
    };
    use product_pipeline_common::flow_out::PipeFlowOut;
    use product_pipeline_common::types::CompletedPipeProduct;

    #[rstest::rstest]
    #[case(0)]
    #[case(1)]
    #[case(2)]
    #[case(5)]
    #[case(10)]
    #[case(100)]
    #[case(150)]
    #[case(198)]
    #[case(199)]
    #[case(200)]
    #[case(201)]
    #[case(397)]
    #[case(398)]
    #[case(399)]
    #[case(400)]
    #[case(401)]
    #[case(402)]
    #[case(404)]
    #[case(4040)]
    #[case(4231)]
    #[trace]
    #[tokio::test]
    async fn should_partially_fail_when_dynamodb_partially_fails(#[case] count: usize) {
        let mut dynamodb_repository = MockProductDynamoDbRepository::default();
        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        dynamodb_repository
            .expect_update_product_record()
            .returning(|_, _, _| Box::pin(async { Err(SdkError::construction_failure("Foo")) }));
        opensearch_repository
            .expect_update_product_documents()
            .never();

        let flow_out_impl = PersistDynamoDbOpenSearchPipeFlowOutImpl::new(
            &dynamodb_repository,
            &opensearch_repository,
        );
        let actual = flow_out_impl
            .flow_out(fake::vec![CompletedPipeProduct; count])
            .await;

        assert!(actual.successes.is_empty());
        assert_eq!(count, actual.failures.len());
    }

    #[rstest::rstest]
    #[case(0, 0)]
    #[case(1, 0)]
    #[case(0, 1)]
    #[case(1, 1)]
    #[case(2, 1)]
    #[case(1, 2)]
    #[case(5, 1)]
    #[case(5, 2)]
    #[case(10, 1)]
    #[case(10, 3)]
    #[case(10, 5)]
    #[case(10, 9)]
    #[case(199, 0)]
    #[case(198, 1)]
    #[case(150, 49)]
    #[case(150, 50)]
    #[case(100, 99)]
    #[case(1, 198)]
    #[case(200, 0)]
    #[case(199, 1)]
    #[case(150, 50)]
    #[case(100, 100)]
    #[case(1, 199)]
    #[case(0, 200)]
    #[case(0, 201)]
    #[case(0, 2345)]
    #[trace]
    #[tokio::test]
    async fn should_partially_fail_when_dynamodb_succeeds_but_opensearch_update_partially_fails(
        #[case] successes: usize,
        #[case] failures: usize,
    ) {
        let mut dynamodb_repository = MockProductDynamoDbRepository::default();
        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        dynamodb_repository
            .expect_update_product_record()
            .returning(|_, _, _| Box::pin(async { Ok(UpdateItemOutput::builder().build()) }));
        opensearch_repository
            .expect_update_product_documents()
            .returning(move |updates| {
                let product_ids = updates.into_keys().collect::<Vec<_>>();
                let (succ, fail) = product_ids.split_at(successes);
                let succ_res = succ
                    .iter()
                    .map(|product_id| BulkItemResult::Update {
                        update: BulkOpResult {
                            index: "products".to_owned(),
                            id: product_id.to_string(),
                            version: None,
                            status: 200,
                            error: None,
                        },
                    })
                    .collect::<Vec<_>>();
                let fail_res = fail
                    .iter()
                    .map(|product_id| BulkItemResult::Update {
                        update: BulkOpResult {
                            index: "products".to_owned(),
                            id: product_id.to_string(),
                            version: None,
                            status: if Faker.fake() { 400 } else { 500 },
                            error: Some(BulkError {
                                error_type: "foo".to_owned(),
                                reason: "unknown".to_owned(),
                                index_uuid: None,
                                shard: None,
                                index: None,
                                extra: None,
                            }),
                        },
                    })
                    .collect::<Vec<_>>();
                Box::pin(async move {
                    Ok(BulkResponse {
                        took: 420,
                        errors: true,
                        items: [succ_res, fail_res].concat(),
                    })
                })
            });

        let flow_out_impl = PersistDynamoDbOpenSearchPipeFlowOutImpl::new(
            &dynamodb_repository,
            &opensearch_repository,
        );
        let actual = flow_out_impl
            .flow_out(fake::vec![CompletedPipeProduct; (successes + failures)])
            .await;

        assert_eq!(successes, actual.successes.len());
        assert_eq!(failures, actual.failures.len());
    }
}
