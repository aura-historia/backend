use crate::{document::SearchFilterDocument, product_match_document::product_match_document};
use common::error::boxed::box_error;
use common::opensearch::search_response::SearchResponse;
use common::pagination::cursor::{Cursor, CursoredResult};
use common::resource_state::document::ResourceStateDocument;
use common::user_search_filter_id::UserSearchFilterId;
use opensearch::{
    DeleteParts, IndexParts, OpenSearch, SearchParts, http::StatusCode, params::VersionType,
};
use search_filter_service::ports::{
    SearchFilterIndex, SearchFilterIndexError, SearchFilterIndexQuery, SearchFilterProjection,
    SearchFilterProjectionWriteOutcome, SearchFilterView,
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
}

#[async_trait::async_trait]
impl SearchFilterIndex for OpenSearchSearchFilterIndex {
    async fn upsert(
        &self,
        projection: &SearchFilterProjection,
    ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
        let document = SearchFilterDocument::try_from(projection).map_err(|source| {
            SearchFilterIndexError::WriteFailed {
                source: box_error(source),
            }
        })?;
        let response = self
            .client
            .index(IndexParts::IndexId(
                &self.index,
                &document.user_search_filter_id.to_string(),
            ))
            .version(document.source_version)
            .version_type(VersionType::External)
            .body(document)
            .send()
            .await
            .map_err(|source| SearchFilterIndexError::WriteFailed {
                source: box_error(source),
            })?;
        projection_write_outcome(response, |source| SearchFilterIndexError::WriteFailed {
            source,
        })
    }

    async fn delete(
        &self,
        id: UserSearchFilterId,
        source_version: i64,
    ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
        let response = self
            .client
            .delete(DeleteParts::IndexId(&self.index, &id.to_string()))
            .version(source_version)
            .version_type(VersionType::External)
            .send()
            .await
            .map_err(|source| SearchFilterIndexError::DeleteFailed {
                source: box_error(source),
            })?;
        projection_write_outcome(response, |source| SearchFilterIndexError::DeleteFailed {
            source,
        })
    }

    async fn percolate(
        &self,
        product: &product_service::ports::ProductSearchFilterMatchSource,
    ) -> Result<Vec<SearchFilterView>, SearchFilterIndexError> {
        let product_document = product_match_document(product).map_err(|source| {
            SearchFilterIndexError::PercolateFailed {
                source: box_error(source),
            }
        })?;
        let body = json!({"query":{"percolate":{"field":"query","document": product_document}}});
        let response = self
            .client
            .search(SearchParts::Index(&[&self.index]))
            .body(body)
            .send()
            .await
            .map_err(|source| SearchFilterIndexError::PercolateFailed {
                source: box_error(source),
            })?
            .error_for_status_code()
            .map_err(|source| SearchFilterIndexError::PercolateFailed {
                source: box_error(source),
            })?;
        let payload =
            response
                .text()
                .await
                .map_err(|source| SearchFilterIndexError::PercolateFailed {
                    source: box_error(source),
                })?;
        let response = serde_json::from_str::<SearchResponse<SearchFilterDocument>>(&payload)
            .map_err(|source| SearchFilterIndexError::InvalidDocument {
                source: box_error(source),
            })?;
        response
            .hits
            .hits
            .into_iter()
            .map(|hit| SearchFilterView::try_from(hit.source))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| SearchFilterIndexError::InvalidDocument {
                source: box_error(source),
            })
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
            .map_err(|source| SearchFilterIndexError::QueryFailed {
                source: box_error(source),
            })?
            .error_for_status_code()
            .map_err(|source| SearchFilterIndexError::QueryFailed {
                source: box_error(source),
            })?;
        let payload =
            response
                .text()
                .await
                .map_err(|source| SearchFilterIndexError::QueryFailed {
                    source: box_error(source),
                })?;
        let response = serde_json::from_str::<SearchResponse<SearchFilterDocument>>(&payload)
            .map_err(|source| SearchFilterIndexError::InvalidDocument {
                source: box_error(source),
            })?;
        let total = response.hits.total.value;
        let search_after = response.hits.hits.last().and_then(|hit| hit.sort.clone());
        let items: Vec<_> = response
            .hits
            .hits
            .into_iter()
            .map(|hit| SearchFilterView::try_from(hit.source))
            .collect::<Result<_, _>>()
            .map_err(|source| SearchFilterIndexError::InvalidDocument {
                source: box_error(source),
            })?;
        Ok(CursoredResult {
            cursor: Cursor {
                size: items.len() as u64,
                search_after,
            },
            items,
            total: Some(total),
        })
    }
}

fn projection_write_outcome(
    response: opensearch::http::response::Response,
    error: impl FnOnce(common::error::boxed::BoxError) -> SearchFilterIndexError,
) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
    if response.status_code() == StatusCode::CONFLICT {
        return Ok(SearchFilterProjectionWriteOutcome::Stale);
    }
    response
        .error_for_status_code()
        .map(|_| SearchFilterProjectionWriteOutcome::Applied)
        .map_err(|source| error(box_error(source)))
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
