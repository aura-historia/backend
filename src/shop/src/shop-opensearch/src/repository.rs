use crate::{shop_document::ShopDocument, shop_search::ShopSearch};
use common::{
    opensearch::{index_response::IndexResponse, search_response::SearchResponse},
    pagination::page::Page,
    sort::{Sort, SortOrder},
};
use opensearch::{IndexParts, SearchParts};
use serde::ser::Error;
use serde_json::json;
use shop_core::sort_shop_field::SortShopField;
use time::format_description::well_known;

#[async_trait::async_trait]
#[mockall::automock]
pub trait ShopOpenSearchRepository {
    async fn create_shop_document(
        &self,
        document: ShopDocument,
    ) -> Result<IndexResponse, opensearch::Error>;

    async fn search_shop_documents(
        &self,
        search: &ShopSearch,
        sort: &Sort<SortShopField>,
        page: &Option<Page>,
    ) -> Result<SearchResponse<ShopDocument>, opensearch::Error>;
}

#[derive(Debug, Clone)]
pub struct ShopOpenSearchRepositoryImpl<'a> {
    client: &'a opensearch::OpenSearch,
}

impl<'a> ShopOpenSearchRepositoryImpl<'a> {
    pub fn new(client: &'a opensearch::OpenSearch) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl<'a> ShopOpenSearchRepository for ShopOpenSearchRepositoryImpl<'a> {
    async fn create_shop_document(
        &self,
        document: ShopDocument,
    ) -> Result<IndexResponse, opensearch::Error> {
        let response = self
            .client
            .index(IndexParts::IndexId("shops", &document._id().to_string()))
            .body(document)
            .send()
            .await?;

        let payload = response.text().await?;
        let index_response = serde_json::from_str::<IndexResponse>(&payload)
            .map_err(|err| {
                serde_json::Error::custom(format!(
                    "Failed deserializing 'IndexResponse' with error '{err}'. Received payload: {payload}"
                ))
            })?;

        Ok(index_response)
    }

    async fn search_shop_documents(
        &self,
        search: &ShopSearch,
        sort: &Sort<SortShopField>,
        page: &Option<Page>,
    ) -> Result<SearchResponse<ShopDocument>, opensearch::Error> {
        let mut must = Vec::with_capacity(2);
        let mut filter = Vec::with_capacity(6);

        if let Some(query) = search.shop_name_query.as_ref() {
            must.push(json!({
                "match": {
                    "name": {
                        "query": query,
                        "fuzziness": "AUTO",
                        "minimum_should_match": "70%"
                    }
                }
            }));
        }

        if let Some(min) = search.created.and_then(|created| created.min) {
            let formatted_min = min
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            filter.push(json!({
                "range": { "created": { "gte": formatted_min } }
            }));
        }
        if let Some(max) = search.created.and_then(|created| created.max) {
            let formatted_max = max
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            filter.push(json!({
                "range": { "created": { "lte": formatted_max } }
            }));
        }

        if let Some(min) = search.updated.and_then(|updated| updated.min) {
            let formatted_min = min
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            filter.push(json!({
                "range": { "updated": { "gte": formatted_min } }
            }));
        }
        if let Some(max) = search.updated.and_then(|updated| updated.max) {
            let formatted_max = max
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            filter.push(json!({
                "range": { "updated": { "lte": formatted_max } }
            }));
        }

        let mut body = json!({
            "query": {
                "bool": {
                    "must": must,
                    "filter": filter
                },
            }
        });

        if let Some(p) = page {
            body.as_object_mut()
                .unwrap()
                .insert("from".to_string(), json!(p.from));
            body.as_object_mut()
                .unwrap()
                .insert("size".to_string(), json!(p.size));
        }

        let sort_field = match sort.sort {
            SortShopField::Score => "_score",
            SortShopField::Name => "name.keyword",
            SortShopField::Created => "created",
            SortShopField::Updated => "updated",
        };
        let order = match sort.order {
            SortOrder::Asc => "asc",
            SortOrder::Desc => "desc",
        };
        let primary_sort = if matches!(sort.sort, SortShopField::Score) {
            json!({ sort_field: { "order": order } })
        } else {
            json!({ sort_field: { "order": order, "missing": "_last" } })
        };
        body.as_object_mut().unwrap().insert(
            "sort".to_string(),
            json!([
                primary_sort,
                { "shopId": { "order": order} } // tie-breaker
            ]),
        );

        let response = self
            .client
            .search(SearchParts::Index(&["shops"]))
            .body(body)
            .send()
            .await?;
        let payload = response.text().await?;
        let search_response = serde_json::from_str::<SearchResponse<ShopDocument>>(&payload)
            .map_err(|err| {
                serde_json::Error::custom(format!(
                    "Failed deserializing 'SearchResponse<ShopDocument>' with error '{err}'. Received payload: {payload}"
                ))
            })?;

        Ok(search_response)
    }
}
