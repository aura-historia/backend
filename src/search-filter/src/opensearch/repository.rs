use crate::core::user_search_filter_search::UserSearchFilterSearch;
use crate::opensearch::user_search_filter_document::UserSearchFilterDocument;
use common::opensearch::delete_response::DeleteResponse;
use common::opensearch::index_response::IndexResponse;
use common::opensearch::search_response::SearchResponse;
use common::pagination::cursor::Cursor;
use common::resource_state::document::ResourceStateDocument;
use common::user_search_filter_id::UserSearchFilterId;
use opensearch::{DeleteParts, IndexParts, SearchParts};
use product::opensearch::product_document::ProductDocument;
use serde::ser::Error;
use serde_json::json;

const INDEX_NAME: &str = "user_search_filters";

fn build_percolate_request_body(
    product_document: &ProductDocument,
) -> Result<serde_json::Value, serde_json::Error> {
    let document_value = serde_json::to_value(product_document).map_err(|err| {
        serde_json::Error::custom(format!(
            "Failed serializing ProductDocument with error '{err}'."
        ))
    })?;

    Ok(json!({
        "query": {
            "percolate": {
                "field": "query",
                "document": document_value
            }
        }
    }))
}

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

    async fn query_documents(
        &self,
        search: &UserSearchFilterSearch,
        cursor: &Option<Cursor<serde_json::Value>>,
    ) -> Result<SearchResponse<UserSearchFilterDocument>, opensearch::Error>;
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
        let body = build_percolate_request_body(product_document)?;

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

    async fn query_documents(
        &self,
        search: &UserSearchFilterSearch,
        cursor: &Option<Cursor<serde_json::Value>>,
    ) -> Result<SearchResponse<UserSearchFilterDocument>, opensearch::Error> {
        let mut filter = Vec::new();
        if let Some(state) = search.state {
            filter.push(json!({
                "term": {
                    "state": ResourceStateDocument::from(state)
                }
            }));
        }
        if let Some(has_enhanced_search_description) = search.has_enhanced_search_description {
            let exists_query = json!({
                "bool": {
                    "should": [
                        { "exists": { "field": "enhancedSearchDescription" } },
                        { "exists": { "field": "search.enhancedSearchDescription" } }
                    ],
                    "minimum_should_match": 1
                }
            });
            if has_enhanced_search_description {
                filter.push(exists_query);
            } else {
                filter.push(json!({
                    "bool": {
                        "must_not": [exists_query]
                    }
                }));
            }
        }

        let mut body = json!({
            "query": {
                "bool": {
                    "filter": filter
                }
            },
            "sort": [
                { "lastHybridSearchMatched": { "order": "asc", "missing": "_first" } },
                { "userSearchFilterId": { "order": "asc" } }
            ]
        });

        if let Some(c) = cursor {
            body["size"] = json!(c.size);
            if let Some(search_after) = &c.search_after {
                body["search_after"] = json!(search_after);
            }
        }

        let response = self
            .client
            .search(SearchParts::Index(&[INDEX_NAME]))
            .body(body)
            .send()
            .await?
            .error_for_status_code()?;

        let payload = response.text().await?;
        serde_json::from_str::<SearchResponse<UserSearchFilterDocument>>(&payload).map_err(|err| {
            serde_json::Error::custom(format!(
                "Failed deserializing 'SearchResponse<UserSearchFilterDocument>' with error '{err}'. Received payload: {payload}"
            ))
            .into()
        })
    }
}

#[cfg(all(test, feature = "test-data"))]
mod tests {
    use super::*;
    use fake::{Fake, Faker};

    #[test]
    fn should_build_percolate_request_without_min_score() {
        let product_document: ProductDocument = Faker.fake();

        let actual = build_percolate_request_body(&product_document).unwrap();

        assert!(actual.get("min_score").is_none());
        assert_eq!(
            actual.pointer("/query/percolate/field"),
            Some(&json!("query"))
        );
        assert!(actual.pointer("/query/percolate/document").is_some());
    }
}
