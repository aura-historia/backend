use crate::period::document::{PeriodDocument, PeriodDocumentSerdeField};
use crate::period::period_search::PeriodSearch;
use crate::period::sort_period_field::SortPeriodField;
use common::language::domain::Language;
use common::opensearch::index_response::IndexResponse;
use common::opensearch::search_response::SearchResponse;
use common::sort::{Sort, SortOrder};
use opensearch::{IndexParts, SearchParts};
use product::core::title::Title;
use serde::ser::Error;
use serde_json::json;

#[async_trait::async_trait]
#[mockall::automock]
pub trait PeriodOpenSearchRepository {
    async fn index_period_document(
        &self,
        document: PeriodDocument,
    ) -> Result<IndexResponse, opensearch::Error>;

    async fn exact_k_nn(
        &self,
        embedding: &[f32],
        k: u16,
    ) -> Result<SearchResponse<PeriodDocument>, opensearch::Error>;

    async fn hybrid_search(
        &self,
        product_title: &Title,
        product_embedding: &[f32],
        k: u16,
    ) -> Result<SearchResponse<PeriodDocument>, opensearch::Error>;

    async fn search_period_documents(
        &self,
        search: &PeriodSearch,
        sort: &Sort<SortPeriodField>,
    ) -> Result<SearchResponse<PeriodDocument>, opensearch::Error>;
}

pub struct PeriodOpenSearchRepositoryImpl<'a> {
    client: &'a opensearch::OpenSearch,
}

impl<'a> PeriodOpenSearchRepositoryImpl<'a> {
    pub fn new(client: &'a opensearch::OpenSearch) -> Self {
        PeriodOpenSearchRepositoryImpl { client }
    }
}

#[async_trait::async_trait]
impl<'a> PeriodOpenSearchRepository for PeriodOpenSearchRepositoryImpl<'a> {
    async fn index_period_document(
        &self,
        document: PeriodDocument,
    ) -> Result<IndexResponse, opensearch::Error> {
        let response = self
            .client
            .index(IndexParts::IndexId(
                "periods",
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
    ) -> Result<SearchResponse<PeriodDocument>, opensearch::Error> {
        let response = self
            .client
            .search(SearchParts::Index(&["periods"]))
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
                         "field": PeriodDocumentSerdeField::Embedding.as_str(),
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

        let response_body = response.json::<SearchResponse<PeriodDocument>>().await?;
        Ok(response_body)
    }

    async fn hybrid_search(
        &self,
        product_title: &Title,
        product_embedding: &[f32],
        k: u16,
    ) -> Result<SearchResponse<PeriodDocument>, opensearch::Error> {
        let response = self
            .client
            .search(SearchParts::Index(&["periods"]))
            .body(json!({
                "size": k,
                "query": {
                    "bool": {
                        "should": [
                            {
                                "script_score": {
                                    "query": { "match_all": {} },
                                    "script": {
                                        "source": "knn_score",
                                        "lang": "knn",
                                        "params": {
                                            "field": PeriodDocumentSerdeField::Embedding.as_str(),
                                            "query_value": product_embedding,
                                            "space_type": "cosinesimil"
                                        }
                                    },
                                    "boost": 2.0
                                }
                            },
                            {
                                "multi_match": {
                                    "query": product_title.as_ref(),
                                    "fields": [
                                        format!("{}^8", PeriodDocumentSerdeField::MetaName.as_str()),
                                        format!("{}^5", PeriodDocumentSerdeField::MetaKeywords.as_str()),
                                        format!("{}^3", PeriodDocumentSerdeField::MetaDescription.as_str()),
                                    ],
                                    "type": "best_fields",
                                    "fuzziness": "AUTO",
                                    "minimum_should_match": "60%",
                                    "boost": 4.0
                                }
                            }
                        ],
                        "minimum_should_match": 1
                    }
                }
            }))
            .send()
            .await?
            .error_for_status_code()?;

        let payload = response.text().await?;
        let search_response =
            serde_json::from_str::<SearchResponse<PeriodDocument>>(&payload).map_err(|err| {
                serde_json::Error::custom(format!(
                    "Failed deserializing 'SearchResponse<PeriodDocument>' with error '{err}'. Received payload: {payload}"
                ))
            })?;

        Ok(search_response)
    }

    async fn search_period_documents(
        &self,
        search: &PeriodSearch,
        sort: &Sort<SortPeriodField>,
    ) -> Result<SearchResponse<PeriodDocument>, opensearch::Error> {
        let mut must = Vec::with_capacity(1);

        if let Some(query) = search.name_query.as_ref() {
            let name_field = match search.language {
                Language::De => PeriodDocumentSerdeField::DisplayNameDe.as_str(),
                Language::En => PeriodDocumentSerdeField::DisplayNameEn.as_str(),
                Language::Fr => PeriodDocumentSerdeField::DisplayNameFr.as_str(),
                Language::Es => PeriodDocumentSerdeField::DisplayNameEs.as_str(),
                Language::It => PeriodDocumentSerdeField::DisplayNameIt.as_str(),
                _ => PeriodDocumentSerdeField::DisplayNameEn.as_str(),
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
            SortPeriodField::Score => "_score",
            SortPeriodField::Name => match search.language {
                Language::De => "displayNameDe.keyword",
                Language::En => "displayNameEn.keyword",
                Language::Fr => "displayNameFr.keyword",
                Language::Es => "displayNameEs.keyword",
                Language::It => "displayNameIt.keyword",
                _ => "displayNameEn.keyword",
            },
            SortPeriodField::Created => PeriodDocumentSerdeField::Created.as_str(),
            SortPeriodField::Updated => PeriodDocumentSerdeField::Updated.as_str(),
        };
        let order = match sort.order {
            SortOrder::Asc => "asc",
            SortOrder::Desc => "desc",
        };
        let primary_sort = if matches!(sort.sort, SortPeriodField::Score) {
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
                { PeriodDocumentSerdeField::PeriodId.as_str(): { "order": "asc" } }
            ]
        });

        let response = self
            .client
            .search(SearchParts::Index(&["periods"]))
            .body(body)
            .send()
            .await?
            .error_for_status_code()?;
        let payload = response.text().await?;
        let search_response =
            serde_json::from_str::<SearchResponse<PeriodDocument>>(&payload).map_err(|err| {
                serde_json::Error::custom(format!(
                    "Failed deserializing 'SearchResponse<PeriodDocument>' with error '{err}'. Received payload: {payload}"
                ))
            })?;

        Ok(search_response)
    }
}
