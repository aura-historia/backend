use crate::opensearch::user_search_filter_document::UserSearchFilterDocument;
use common::opensearch::delete_response::DeleteResponse;
use common::opensearch::index_response::IndexResponse;
use common::user_search_filter_id::UserSearchFilterId;
use opensearch::{DeleteParts, IndexParts, SearchParts};
use product::opensearch::product_document::ProductDocument;
use serde::ser::Error;
use serde_json::json;

const INDEX_NAME: &str = "user_search_filters";
const PERCOLATE_MIN_SCORE: f64 = 3.1;

#[async_trait::async_trait]
#[mockall::automock]
pub trait UserSearchFilterOpenSearchRepository {
    async fn index_document(
        &self,
        document: UserSearchFilterDocument,
    ) -> Result<IndexResponse, opensearch::Error>;

    async fn delete_document(
        &self,
        id: &UserSearchFilterId,
    ) -> Result<DeleteResponse, opensearch::Error>;

    async fn percolate(
        &self,
        product_document: &ProductDocument,
    ) -> Result<Vec<UserSearchFilterDocument>, opensearch::Error>;
}

#[derive(Debug, Clone)]
pub struct UserSearchFilterOpenSearchRepositoryImpl<'a> {
    client: &'a opensearch::OpenSearch,
}

impl<'a> UserSearchFilterOpenSearchRepositoryImpl<'a> {
    pub fn new(client: &'a opensearch::OpenSearch) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl<'a> UserSearchFilterOpenSearchRepository for UserSearchFilterOpenSearchRepositoryImpl<'a> {
    async fn index_document(
        &self,
        document: UserSearchFilterDocument,
    ) -> Result<IndexResponse, opensearch::Error> {
        let response = self
            .client
            .index(IndexParts::IndexId(INDEX_NAME, &document._id().to_string()))
            .body(document)
            .send()
            .await?
            .error_for_status_code()?;

        let payload = response.text().await?;
        let index_response = serde_json::from_str::<IndexResponse>(&payload).map_err(|err| {
            serde_json::Error::custom(format!(
                "Failed deserializing 'IndexResponse' with error '{err}'. Received payload: {payload}"
            ))
        })?;

        Ok(index_response)
    }

    async fn delete_document(
        &self,
        id: &UserSearchFilterId,
    ) -> Result<DeleteResponse, opensearch::Error> {
        let response = self
            .client
            .delete(DeleteParts::IndexId(INDEX_NAME, &id.to_string()))
            .send()
            .await?
            .error_for_status_code()?;

        let payload = response.text().await?;
        let delete_response =
            serde_json::from_str::<DeleteResponse>(&payload).map_err(|err| {
                serde_json::Error::custom(format!(
                    "Failed deserializing 'DeleteResponse' with error '{err}'. Received payload: {payload}"
                ))
            })?;

        Ok(delete_response)
    }

    async fn percolate(
        &self,
        product_document: &ProductDocument,
    ) -> Result<Vec<UserSearchFilterDocument>, opensearch::Error> {
        let document_value = serde_json::to_value(product_document).map_err(|err| {
            serde_json::Error::custom(format!(
                "Failed serializing ProductDocument with error '{err}'."
            ))
        })?;
        let body = json!({
            "min_score": PERCOLATE_MIN_SCORE,
            "query": {
                "percolate": {
                    "field": "query",
                    "document": document_value
                }
            }
        });

        let response = self
            .client
            .search(SearchParts::Index(&[INDEX_NAME]))
            .body(body)
            .send()
            .await?
            .error_for_status_code()?;

        let payload = response.text().await?;
        let search_response = serde_json::from_str::<
            common::opensearch::search_response::SearchResponse<UserSearchFilterDocument>,
        >(&payload)
        .map_err(|err| {
            serde_json::Error::custom(format!(
                "Failed deserializing 'SearchResponse<UserSearchFilterDocument>' with error '{err}'. Received payload: {payload}"
            ))
        })?;

        Ok(search_response
            .hits
            .hits
            .into_iter()
            .map(|hit| hit.source)
            .collect())
    }
}
