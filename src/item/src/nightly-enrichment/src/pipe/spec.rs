use common::item_id::{ItemId, ItemKey};
use item_core::item_event::ItemCreatedEventPayload;
use item_dynamodb::{item_update_record::ItemRecordUpdate, repository::ItemDynamoDbRepository};
use item_opensearch::{
    item_update_document::ItemUpdateDocument, repository::ItemOpenSearchRepository,
};
use std::collections::HashMap;
use tracing::{error, warn};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone)]
pub struct PipeItem {
    pub source: PipeItemSource,
    pub update: PipeItemUpdate,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone)]
pub struct PipeItemSource {
    pub item_id: ItemId,
    pub payload: ItemCreatedEventPayload,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone)]
pub struct PipeItemUpdate {
    pub document: Option<ItemUpdateDocument>,
    pub record: Option<ItemRecordUpdate>,
}

pub trait EnrichmentPipe {
    type Error;

    fn enrich(&self, items: Vec<PipeItem>) -> Result<Vec<PipeItem>, Self::Error>;
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait EnrichmentPipeSink {
    async fn drain_documents(&self, documents: HashMap<ItemId, ItemUpdateDocument>) -> Vec<ItemId>;

    async fn drain_records(
        &self,
        records: HashMap<ItemId, (ItemKey, ItemRecordUpdate)>,
    ) -> Vec<ItemId>;
}

pub struct EnrichmentPipeSinkImpl<'a> {
    item_dynamodb_repository: &'a (dyn ItemDynamoDbRepository + Sync),
    item_opensearch_repository: &'a (dyn ItemOpenSearchRepository + Sync),
}

impl<'a> EnrichmentPipeSinkImpl<'a> {
    pub fn new(
        item_dynamodb_repository: &'a (dyn ItemDynamoDbRepository + Sync),
        item_opensearch_repository: &'a (dyn ItemOpenSearchRepository + Sync),
    ) -> Self {
        Self {
            item_dynamodb_repository,
            item_opensearch_repository,
        }
    }
}

#[async_trait::async_trait]
impl<'a> EnrichmentPipeSink for EnrichmentPipeSinkImpl<'a> {
    async fn drain_documents(&self, documents: HashMap<ItemId, ItemUpdateDocument>) -> Vec<ItemId> {
        let item_ids = documents.keys().copied().collect::<Vec<_>>();
        let res = self
            .item_opensearch_repository
            .update_item_documents(documents)
            .await;

        match res {
            Err(err) => {
                error!(error = %err, "Failed draining all ItemDocument-Updates for this sink.");
                item_ids
            }
            Ok(response) => {
                if response.errors {
                    response.items.into_iter().map(|res| res.unwrap_update()).filter_map(|failure | {
                        warn!(error = ?failure.error, status = failure.status, itemId = failure.id, "Failed updating ItemDocument.");
                        match ItemId::try_from(failure.id.as_str()) {
                            Err(err) => {
                                error!(error = %err, itemId = failure.id, "Failed parsing returned '_id' from OpenSearch for 'ItemDocument' as 'ItemId'.
                                    This is highly to be a bug. Cannot retry.");
                                None
                            }
                            Ok(item_id) => Some(item_id)
                        }
                    }).collect()
                } else {
                    vec![]
                }
            }
        }
    }

    async fn drain_records(
        &self,
        records: HashMap<ItemId, (ItemKey, ItemRecordUpdate)>,
    ) -> Vec<ItemId> {
        let mut failures = Vec::new();
        for (item_id, (item_key, record)) in records {
            let res = self
                .item_dynamodb_repository
                .update_item_record(&item_key.shop_id, &item_key.shops_item_id, record)
                .await;
            if let Err(err) = res {
                error!(error = ?err, itemId = %item_id, "Failed updating ItemRecord.");
                failures.push(item_id);
            }
        }

        failures
    }
}

#[cfg(test)]
mod tests {
    mod drain_documents {
        use crate::pipe::spec::{EnrichmentPipeSink, EnrichmentPipeSinkImpl};
        use common::{
            item_id::ItemId,
            opensearch::bulk_response::{BulkItemResult, BulkOpResult, BulkResponse},
        };
        use fake::{Fake, Faker};
        use item_dynamodb::repository::MockItemDynamoDbRepository;
        use item_opensearch::{
            item_update_document::ItemUpdateDocument, repository::MockItemOpenSearchRepository,
        };
        use std::collections::HashMap;

        #[tokio::test]
        async fn should_drain_documents_when_successful() {
            let item_dynamodb_repository = MockItemDynamoDbRepository::default();
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

            let sink =
                EnrichmentPipeSinkImpl::new(&item_dynamodb_repository, &item_opensearch_repository);
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
            let item_dynamodb_repository = MockItemDynamoDbRepository::default();
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
                                .map(|item_id| BulkItemResult::Update {
                                    update: BulkOpResult {
                                        index: "items".to_string(),
                                        id: item_id.to_string(),
                                        version: None,
                                        status: 409,
                                        error: None,
                                    },
                                })
                                .collect(),
                        })
                    })
                });

            let sink =
                EnrichmentPipeSinkImpl::new(&item_dynamodb_repository, &item_opensearch_repository);
            let input = fake::vec![(ItemId, ItemUpdateDocument); 1000]
                .into_iter()
                .collect::<HashMap<_, _>>();
            let actual = sink.drain_documents(input.clone()).await;

            assert_eq!(failure_count, actual.len());
            assert_eq!(
                input.into_keys().take(failure_count).collect::<Vec<_>>(),
                actual
            );
        }
    }
}
