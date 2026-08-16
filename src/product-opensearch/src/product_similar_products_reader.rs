use crate::product_document::{ProductDocument, ProductDocumentSerdeField};
use crate::product_search_reader::map_search_response_without_price;
use common::error::boxed::box_error;
use common::opensearch::search_response::SearchResponse;
use opensearch::{OpenSearch, SearchParts};
use product_core::product_search::ProductSearch;
use product_service::ports::{
    ProductSimilarProductsReadError, ProductSimilarProductsReader, ProductSimilarProductsRequest,
};
use product_service::use_cases::ProductSummary;
use serde_json::json;

const DEFAULT_INDEX: &str = "products";
const DEFAULT_RESULT_COUNT: u64 = 20;

#[derive(Clone)]
pub struct OpenSearchProductSimilarProductsReader {
    client: OpenSearch,
    index: String,
}

impl OpenSearchProductSimilarProductsReader {
    pub fn new(client: OpenSearch) -> Self {
        Self {
            client,
            index: DEFAULT_INDEX.to_owned(),
        }
    }

    pub fn with_index(client: OpenSearch, index: impl Into<String>) -> Self {
        Self {
            client,
            index: index.into(),
        }
    }
}

#[async_trait::async_trait]
impl ProductSimilarProductsReader for OpenSearchProductSimilarProductsReader {
    #[tracing::instrument(name = "opensearch_product_similar_products", skip_all)]
    async fn find_similar_products(
        &self,
        request: &ProductSimilarProductsRequest,
    ) -> Result<Vec<ProductSummary>, ProductSimilarProductsReadError> {
        let response = self
            .client
            .search(SearchParts::Index(&[self.index.as_str()]))
            .body(build_similar_products_request(request))
            .send()
            .await
            .map_err(
                |error| ProductSimilarProductsReadError::SimilarProductsQueryFailed {
                    source: box_error(error),
                },
            )?
            .error_for_status_code()
            .map_err(
                |error| ProductSimilarProductsReadError::SimilarProductsQueryFailed {
                    source: box_error(error),
                },
            )?;
        let payload = response.text().await.map_err(|error| {
            ProductSimilarProductsReadError::SimilarProductsQueryFailed {
                source: box_error(error),
            }
        })?;
        let search_response = serde_json::from_str::<SearchResponse<ProductDocument>>(&payload)
            .map_err(
                |error| ProductSimilarProductsReadError::SimilarProductsQueryFailed {
                    source: box_error(error),
                },
            )?
            .into_non_timed_out("similar products")
            .map_err(
                |error| ProductSimilarProductsReadError::SimilarProductsQueryFailed {
                    source: box_error(error),
                },
            )?;
        let search = ProductSearch::new(request.language, request.currency);

        map_search_response_without_price(&search, search_response)
            .map(|result| result.items)
            .map_err(
                |error| ProductSimilarProductsReadError::SimilarProductsQueryFailed {
                    source: box_error(error),
                },
            )
    }
}

pub(crate) fn build_similar_products_request(
    request: &ProductSimilarProductsRequest,
) -> serde_json::Value {
    json!({
        "_source": { "excludes": [ProductDocumentSerdeField::Embedding] },
        "size": DEFAULT_RESULT_COUNT,
        "query": {
            "knn": {
                ProductDocumentSerdeField::Embedding.as_str(): {
                    "vector": request.embedding,
                    "k": DEFAULT_RESULT_COUNT,
                    "filter": {
                        "bool": {
                            "must_not": [{
                                "term": {
                                    ProductDocumentSerdeField::ProductId.as_str(): request.product_id.to_string()
                                }
                            }]
                        }
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::language::domain::Language;
    use common::product_id::ProductId;
    use serde_json::json;

    #[test]
    fn should_build_knn_request_that_excludes_source_product_and_embedding() {
        let product_id = ProductId::new();
        let request = ProductSimilarProductsRequest::new(product_id, vec![0.1, 0.2], Language::De);

        let actual = build_similar_products_request(&request);

        assert_eq!(
            actual.pointer("/_source/excludes/0"),
            Some(&json!("embedding"))
        );
        assert_eq!(actual.pointer("/size"), Some(&json!(20)));
        assert_eq!(
            actual.pointer("/query/knn/embedding/vector"),
            Some(&json!(request.embedding))
        );
        assert_eq!(actual.pointer("/query/knn/embedding/k"), Some(&json!(20)));
        assert_eq!(
            actual.pointer("/query/knn/embedding/filter/bool/must_not/0/term/productId"),
            Some(&json!(product_id.to_string()))
        );
        assert!(actual.pointer("/query/bool").is_none());
    }
}
