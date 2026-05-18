use crate::core::product::{LocalizedProductView, Product};
use crate::core::product_event::ProductEvent;
use crate::dynamodb::repository::ProductDynamoDbRepository;
use crate::opensearch::repository::ProductOpenSearchRepository;
use async_trait::async_trait;
use aws_sdk_dynamodb::error::SdkError;
use common::aggregate::Aggregate;
use common::currency::domain::Currency;
use common::language::domain::Language;
use common::opensearch::search_response::OpenSearchTimedOutError;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use time::OffsetDateTime;
use tracing::warn;

#[derive(thiserror::Error, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum SemanticSearchProductsError {
    #[error("Product with ShopId '{0}' and ShopsProductId '{1}' not found.")]
    ProductNotFound(ShopId, ShopsProductId),

    #[error("OpenSearchError: {0}")]
    OpenSearchError(#[from] opensearch::Error),

    #[error("OpenSearchTimedOut: {0}")]
    OpenSearchTimedOut(#[from] OpenSearchTimedOutError),

    #[error("Encountered DynamoDB SdkError for GetItem: {0:?}")]
    SdkGetItemError(#[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError>),

    #[error("Encountered DynamoDB SdkError for Query: {0:?}")]
    SdkQueryError(#[from] SdkError<aws_sdk_dynamodb::operation::query::QueryError>),

    #[error("Failed replaying product events: {0}")]
    ProductReplayError(#[from] crate::core::product::ProductReplayError),
}

#[cfg(feature = "data")]
pub mod api {
    use crate::service::semantic_service::SemanticSearchProductsError;
    use common::api::error::ApiError;
    use common::api::error_code::PRODUCT_NOT_FOUND;

    impl From<SemanticSearchProductsError> for ApiError {
        fn from(err: SemanticSearchProductsError) -> Self {
            match err {
                SemanticSearchProductsError::ProductNotFound(_, _) => {
                    ApiError::not_found(PRODUCT_NOT_FOUND, Box::new(err))
                }
                SemanticSearchProductsError::OpenSearchError(opensearch_err) => {
                    opensearch_err.into()
                }
                SemanticSearchProductsError::OpenSearchTimedOut(timeout_err) => timeout_err.into(),
                SemanticSearchProductsError::SdkGetItemError(sdk_error) => sdk_error.into(),
                SemanticSearchProductsError::SdkQueryError(sdk_error) => sdk_error.into(),
                SemanticSearchProductsError::ProductReplayError(_) => {
                    ApiError::not_found(PRODUCT_NOT_FOUND, Box::new(err))
                }
            }
        }
    }
}

#[async_trait]
#[mockall::automock]
pub trait SemanticSearchService {
    async fn similar_products(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<Option<Vec<LocalizedProductView>>, SemanticSearchProductsError>;
}

pub struct SemanticSearchServiceImpl<'a> {
    dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Sync),
    opensearch_repository: &'a (dyn ProductOpenSearchRepository + Sync),
}

impl<'a> SemanticSearchServiceImpl<'a> {
    pub fn new(
        dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Sync),
        opensearch_repository: &'a (dyn ProductOpenSearchRepository + Sync),
    ) -> Self {
        Self {
            dynamodb_repository,
            opensearch_repository,
        }
    }
}

#[async_trait]
impl<'a> SemanticSearchService for SemanticSearchServiceImpl<'a> {
    async fn similar_products(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<Option<Vec<LocalizedProductView>>, SemanticSearchProductsError> {
        let event_records = self
            .dynamodb_repository
            .query_product_event_records(shop_id, shops_product_id)
            .await?;
        if event_records.is_empty() {
            return Err(SemanticSearchProductsError::ProductNotFound(
                *shop_id,
                shops_product_id.clone(),
            ));
        }
        let product = Product::replay(event_records.into_iter().filter_map(|record| {
            ProductEvent::try_from(record)
                .map_err(|err| warn!(error = %err, "Failed mapping ProductEventRecord."))
                .ok()
        }))?;
        match product.embedding {
            None => {
                if OffsetDateTime::now_utc().date() > product.created.date() {
                    warn!(
                        shopId = %shop_id,
                        shopProductId = %shops_product_id,
                         productId = %product.product_id,
                        "When trying to find similar products for given ProductKey,
                         ProductRecord for ProductKey did not have an embedding
                         although it was created at least one day prior -
                         hence why the nightly product-enrichment SHOULD have run and embedded the text."
                    );
                }
                Ok(None)
            }
            Some(embedding) => {
                let localized_documents = self
                    .opensearch_repository
                    .k_nn_text(&embedding, 20)
                    .await?
                    .into_non_timed_out("product semantic similarity search")?
                    .hits
                    .hits
                    .into_iter()
                    .filter(|hit| hit.source.product_id != product.product_id)
                    .map(|hit| hit.source)
                    .map(Product::from)
                    .map(|product| product.localized(currency, languages))
                    .collect();
                Ok(Some(localized_documents))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::product_event::ProductEventPayload;
    use crate::dynamodb::product_event_record::ProductEventRecord;
    use crate::dynamodb::repository::MockProductDynamoDbRepository;
    use crate::opensearch::product_document::ProductDocument;
    use crate::opensearch::repository::MockProductOpenSearchRepository;
    use common::has_key::HasKey;
    use common::opensearch::search_response::{
        HitsMetadata, SearchHit, SearchResponse, ShardStats, TotalHits,
    };
    use fake::{Fake, Faker};

    fn created_event_record() -> (ShopId, ShopsProductId, ProductEventRecord) {
        let event = Product::create(
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
        );
        let key = event.payload.key();
        (
            key.shop_id,
            key.shops_product_id,
            ProductEventRecord::Domain(event.into()),
        )
    }

    #[tokio::test]
    async fn should_return_product_not_found_when_event_stream_is_empty() {
        let mut dynamodb_repository = MockProductDynamoDbRepository::default();
        dynamodb_repository
            .expect_query_product_event_records()
            .return_once(|_, _| Box::pin(async { Ok(vec![]) }));
        let opensearch_repository = MockProductOpenSearchRepository::default();
        let service = SemanticSearchServiceImpl::new(&dynamodb_repository, &opensearch_repository);

        let actual = service
            .similar_products(&ShopId::new(), &ShopsProductId::new(), &[], &Currency::Eur)
            .await;

        assert!(matches!(
            actual,
            Err(SemanticSearchProductsError::ProductNotFound(_, _))
        ));
    }

    #[tokio::test]
    async fn should_return_none_when_product_has_no_embedding() {
        let (shop_id, shops_product_id, record) = created_event_record();
        let mut dynamodb_repository = MockProductDynamoDbRepository::default();
        dynamodb_repository
            .expect_query_product_event_records()
            .return_once(move |_, _| Box::pin(async move { Ok(vec![record]) }));
        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository.expect_k_nn_text().never();
        let service = SemanticSearchServiceImpl::new(&dynamodb_repository, &opensearch_repository);

        let actual = service
            .similar_products(&shop_id, &shops_product_id, &[], &Currency::Eur)
            .await
            .unwrap();

        assert!(actual.is_none());
    }

    fn search_response(documents: Vec<ProductDocument>) -> SearchResponse<ProductDocument> {
        SearchResponse {
            took: 1,
            timed_out: false,
            shards: ShardStats {
                total: 1,
                successful: 1,
                skipped: 0,
                failed: 0,
            },
            hits: HitsMetadata {
                total: TotalHits {
                    value: documents.len() as u64,
                    relation: "eq".to_owned(),
                },
                max_score: None,
                hits: documents
                    .into_iter()
                    .map(|source| SearchHit {
                        index: "products".to_owned(),
                        id: source.product_id.to_string(),
                        score: None,
                        sort: None,
                        matched_queries: vec![],
                        source,
                    })
                    .collect(),
            },
        }
    }

    fn embedded_event_records() -> (
        ShopId,
        ShopsProductId,
        common::product_id::ProductId,
        Vec<ProductEventRecord>,
    ) {
        let (shop_id, shops_product_id, created_record) = created_event_record();
        let mut product = Product::replay(
            vec![created_record.clone()]
                .into_iter()
                .filter_map(|record| ProductEvent::try_from(record).ok()),
        )
        .unwrap();
        let product_id = product.product_id;
        let embedded = product.embed(vec![0.1, 0.2, 0.3]).unwrap();
        (
            shop_id,
            shops_product_id,
            product_id,
            vec![
                created_record,
                ProductEventRecord::from(
                    embedded.map_payload(ProductEventPayload::ProductEnrichmentEvent),
                ),
            ],
        )
    }

    #[tokio::test]
    async fn should_return_similar_products_when_embedding_exists() {
        let (shop_id, shops_product_id, _, records) = embedded_event_records();
        let document_product: Product = Faker.fake();
        let mut dynamodb_repository = MockProductDynamoDbRepository::default();
        dynamodb_repository
            .expect_query_product_event_records()
            .return_once(move |_, _| Box::pin(async move { Ok(records) }));
        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository
            .expect_k_nn_text()
            .return_once(move |embedding, k| {
                assert_eq!(&[0.1, 0.2, 0.3], embedding);
                assert_eq!(20, k);
                Box::pin(async move { Ok(search_response(vec![document_product.into()])) })
            });
        let service = SemanticSearchServiceImpl::new(&dynamodb_repository, &opensearch_repository);

        let actual = service
            .similar_products(&shop_id, &shops_product_id, &[Language::En], &Currency::Eur)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(1, actual.len());
    }

    #[tokio::test]
    async fn should_filter_out_requested_product_from_similar_products() {
        let (shop_id, shops_product_id, product_id, records) = embedded_event_records();
        let mut self_product: Product = Faker.fake();
        self_product.product_id = product_id;
        let other_product: Product = Faker.fake();
        let mut dynamodb_repository = MockProductDynamoDbRepository::default();
        dynamodb_repository
            .expect_query_product_event_records()
            .return_once(move |_, _| Box::pin(async move { Ok(records) }));
        let mut opensearch_repository = MockProductOpenSearchRepository::default();
        opensearch_repository
            .expect_k_nn_text()
            .return_once(move |_, _| {
                Box::pin(async move {
                    Ok(search_response(vec![
                        self_product.into(),
                        other_product.into(),
                    ]))
                })
            });
        let service = SemanticSearchServiceImpl::new(&dynamodb_repository, &opensearch_repository);

        let actual = service
            .similar_products(&shop_id, &shops_product_id, &[Language::En], &Currency::Eur)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(1, actual.len());
        assert_ne!(product_id, actual[0].product_id);
    }
}
