use crate::category::category_search::CategorySearch;
use crate::category::document::{CategoryDocument, CategoryDocumentSerdeField};
use crate::category::sort_category_field::SortCategoryField;
use common::language::domain::Language;
use common::opensearch::index_response::IndexResponse;
use common::opensearch::search_response::SearchResponse;
use common::sort::{Sort, SortOrder};
use opensearch::{IndexParts, SearchParts};
use serde::ser::Error;
use serde_json::json;

#[async_trait::async_trait]
#[mockall::automock]
pub trait CategoryOpenSearchRepository {
    async fn index_category_document(
        &self,
        document: CategoryDocument,
    ) -> Result<IndexResponse, opensearch::Error>;

    async fn exact_k_nn(
        &self,
        embedding: &[f32],
        k: u16,
    ) -> Result<SearchResponse<CategoryDocument>, opensearch::Error>;

    async fn search_category_documents(
        &self,
        search: &CategorySearch,
        sort: &Sort<SortCategoryField>,
    ) -> Result<SearchResponse<CategoryDocument>, opensearch::Error>;
}

pub struct CategoryOpenSearchRepositoryImpl<'a> {
    client: &'a opensearch::OpenSearch,
}

impl<'a> CategoryOpenSearchRepositoryImpl<'a> {
    pub fn new(client: &'a opensearch::OpenSearch) -> Self {
        CategoryOpenSearchRepositoryImpl { client }
    }
}

#[async_trait::async_trait]
impl<'a> CategoryOpenSearchRepository for CategoryOpenSearchRepositoryImpl<'a> {
    async fn index_category_document(
        &self,
        document: CategoryDocument,
    ) -> Result<IndexResponse, opensearch::Error> {
        let response = self
            .client
            .index(IndexParts::IndexId(
                "categories",
                #[allow(clippy::unnecessary_to_owned)]
                &document._id().to_string(),
            ))
            .body(document)
            .send()
            .await?
            .error_for_status_code()?;

        let payload = response.text().await?;
        let index_response = serde_json::from_str::<IndexResponse>(&payload)
            .map_err(|err| {
                serde_json::Error::custom(format!(
                    "Failed deserializing 'IndexResponse' with error '{err}'. Received payload: {payload}"
                ))
            })?;

        Ok(index_response)
    }

    async fn exact_k_nn(
        &self,
        embedding: &[f32],
        k: u16,
    ) -> Result<SearchResponse<CategoryDocument>, opensearch::Error> {
        let response = self
            .client
            .search(SearchParts::Index(&["categories"]))
            .body(json!({
                "size": k,
                "query": {
                   "script_score": {
                     "query": {
                       "match_all": {}
                     },
                     "script": {
                       "source": "knn_score",
                       "lang": "knn",
                       "params": {
                         "field": CategoryDocumentSerdeField::Embedding.as_str(),
                         "query_value": embedding,
                         "space_type": "cosinesimil"
                       }
                     }
                   }
                  }
            }))
            .send()
            .await?
            .error_for_status_code()?;

        let response_body = response.json::<SearchResponse<CategoryDocument>>().await?;
        Ok(response_body)
    }

    async fn search_category_documents(
        &self,
        search: &CategorySearch,
        sort: &Sort<SortCategoryField>,
    ) -> Result<SearchResponse<CategoryDocument>, opensearch::Error> {
        let mut must = Vec::with_capacity(1);

        if let Some(query) = search.name_query.as_ref() {
            let name_field = match search.language {
                Language::De => CategoryDocumentSerdeField::DisplayNameDe.as_str(),
                Language::En => CategoryDocumentSerdeField::DisplayNameEn.as_str(),
                Language::Fr => CategoryDocumentSerdeField::DisplayNameFr.as_str(),
                Language::Es => CategoryDocumentSerdeField::DisplayNameEs.as_str(),
                Language::It => CategoryDocumentSerdeField::DisplayNameIt.as_str(),
            };
            must.push(json!({
                "multi_match": {
                    "query": query,
                    "fields": [
                        format!("{name_field}^5"),
                    ],
                    "fuzziness": "AUTO",
                    "minimum_should_match": "70%"
                }
            }));
        }

        let sort_field = match sort.sort {
            SortCategoryField::Score => "_score",
            SortCategoryField::Name => match search.language {
                Language::De => "displayNameDe.keyword",
                Language::En => "displayNameEn.keyword",
                Language::Fr => "displayNameFr.keyword",
                Language::Es => "displayNameEs.keyword",
                Language::It => "displayNameIt.keyword",
            },
            SortCategoryField::Created => CategoryDocumentSerdeField::Created.as_str(),
            SortCategoryField::Updated => CategoryDocumentSerdeField::Updated.as_str(),
        };
        let order = match sort.order {
            SortOrder::Asc => "asc",
            SortOrder::Desc => "desc",
        };
        let primary_sort = if matches!(sort.sort, SortCategoryField::Score) {
            json!({ sort_field: { "order": order } })
        } else {
            json!({ sort_field: { "order": order, "missing": "_last" } })
        };

        let body = json!({
            "size": 1000, // big-enough catch-all
            "query": {
                "bool": {
                    "must": must,
                }
            },
            "sort": [
                primary_sort,
                { CategoryDocumentSerdeField::CategoryId.as_str(): { "order": "asc" } }
            ]
        });

        let response = self
            .client
            .search(SearchParts::Index(&["categories"]))
            .body(body)
            .send()
            .await?
            .error_for_status_code()?;
        let payload = response.text().await?;
        let search_response =
            serde_json::from_str::<SearchResponse<CategoryDocument>>(&payload).map_err(|err| {
                serde_json::Error::custom(format!(
                    "Failed deserializing 'SearchResponse<CategoryDocument>' with error '{err}'. Received payload: {payload}"
                ))
            })?;

        Ok(search_response)
    }
}
