use crate::document::SearchFilterDocument;
use common::opensearch::search_response::SearchResponse;
use common::pagination::cursor::{Cursor, CursoredResult};
use common::resource_state::document::ResourceStateDocument;
use common::user_search_filter_id::UserSearchFilterId;
use opensearch::{DeleteParts, IndexParts, OpenSearch, SearchParts};
use search_filter_service::ports::{
    SearchFilterIndex, SearchFilterIndexError, SearchFilterIndexQuery, SearchFilterView,
};
use serde_json::json;

const DEFAULT_INDEX: &str = "user_search_filters";

#[derive(Clone)]
pub struct OpenSearchSearchFilterIndex {
    client: OpenSearch,
    index: String,
}

impl OpenSearchSearchFilterIndex {
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
impl SearchFilterIndex for OpenSearchSearchFilterIndex {
    async fn index(&self, filter: &SearchFilterView) -> Result<(), SearchFilterIndexError> {
        let document = SearchFilterDocument::from(filter);
        self.client
            .index(IndexParts::IndexId(
                &self.index,
                &document.user_search_filter_id.to_string(),
            ))
            .body(document)
            .send()
            .await
            .map_err(|_| SearchFilterIndexError::WriteFailed)?
            .error_for_status_code()
            .map_err(|_| SearchFilterIndexError::WriteFailed)?;
        Ok(())
    }

    async fn delete(&self, id: UserSearchFilterId) -> Result<(), SearchFilterIndexError> {
        self.client
            .delete(DeleteParts::IndexId(&self.index, &id.to_string()))
            .send()
            .await
            .map_err(|_| SearchFilterIndexError::DeleteFailed)?
            .error_for_status_code()
            .map_err(|_| SearchFilterIndexError::DeleteFailed)?;
        Ok(())
    }

    async fn percolate(
        &self,
        product_document: serde_json::Value,
    ) -> Result<Vec<SearchFilterView>, SearchFilterIndexError> {
        let body = json!({"query":{"percolate":{"field":"query","document": product_document}}});
        let response = self
            .client
            .search(SearchParts::Index(&[&self.index]))
            .body(body)
            .send()
            .await
            .map_err(|_| SearchFilterIndexError::PercolateFailed)?
            .error_for_status_code()
            .map_err(|_| SearchFilterIndexError::PercolateFailed)?;
        let payload = response
            .text()
            .await
            .map_err(|_| SearchFilterIndexError::PercolateFailed)?;
        let response = serde_json::from_str::<SearchResponse<SearchFilterDocument>>(&payload)
            .map_err(|_| SearchFilterIndexError::InvalidDocument)?;
        Ok(response
            .hits
            .hits
            .into_iter()
            .map(|hit| hit.source.into())
            .collect())
    }

    async fn query(
        &self,
        query: &SearchFilterIndexQuery,
    ) -> Result<CursoredResult<SearchFilterView, serde_json::Value>, SearchFilterIndexError> {
        let body = build_query_body(query);
        let response = self
            .client
            .search(SearchParts::Index(&[&self.index]))
            .body(body)
            .send()
            .await
            .map_err(|_| SearchFilterIndexError::QueryFailed)?
            .error_for_status_code()
            .map_err(|_| SearchFilterIndexError::QueryFailed)?;
        let payload = response
            .text()
            .await
            .map_err(|_| SearchFilterIndexError::QueryFailed)?;
        let response = serde_json::from_str::<SearchResponse<SearchFilterDocument>>(&payload)
            .map_err(|_| SearchFilterIndexError::InvalidDocument)?;
        let items: Vec<_> = response
            .hits
            .hits
            .into_iter()
            .map(|hit| hit.source.into())
            .collect();
        Ok(CursoredResult {
            cursor: Cursor {
                size: items.len() as u64,
                search_after: None,
            },
            items,
            total: Some(response.hits.total.value),
        })
    }
}

fn build_query_body(query: &SearchFilterIndexQuery) -> serde_json::Value {
    let mut filter = Vec::new();
    if let Some(state) = query.state {
        filter.push(json!({"term": {"state": ResourceStateDocument::from(state)}}));
    }
    if let Some(has) = query.has_enhanced_search_description {
        filter.push(if has {
            json!({"exists":{"field":"search.enhancedSearchDescription"}})
        } else {
            json!({"bool":{"must_not":[{"exists":{"field":"search.enhancedSearchDescription"}}]}})
        });
    }
    let mut body = json!({
        "query":{"bool":{"filter":filter}},
        "sort":[
            {"lastHybridSearchMatched":{"order":"asc","missing":"_first"}},
            {"userSearchFilterId":{"order":"asc"}}
        ]
    });
    if let Some(Cursor { size, search_after }) = &query.cursor {
        body["size"] = json!(size);
        if let Some(search_after) = search_after {
            body["search_after"] = search_after.clone();
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::resource_state::domain::ResourceState;

    #[test]
    fn should_build_query_body_with_filters_and_cursor() {
        let body = build_query_body(&SearchFilterIndexQuery {
            state: Some(ResourceState::Active),
            has_enhanced_search_description: Some(true),
            cursor: Some(Cursor {
                size: 25,
                search_after: Some(json!(["a", "b"])),
            }),
        });

        assert_eq!(25, body["size"]);
        assert_eq!(json!(["a", "b"]), body["search_after"]);
        assert!(body["query"]["bool"]["filter"].is_array());
    }
}
