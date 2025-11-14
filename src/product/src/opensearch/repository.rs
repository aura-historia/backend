use crate::core::product_search::ProductSearch;
use crate::core::sort_product_field::SortProductField;
use crate::opensearch::product_document::ProductDocument;
use crate::opensearch::product_state_document::ProductStateDocument;
use crate::opensearch::product_update_document::ProductUpdateDocument;
use async_trait::async_trait;
use common::currency::domain::Currency;
use common::language::domain::Language;
use common::opensearch::{bulk_response::BulkResponse, search_response::SearchResponse};
use common::pagination::cursor::Cursor;
use common::product_id::ProductId;
use common::product_state::domain::ProductState;
use common::sort::{Sort, SortOrder};
use opensearch::{BulkOperation, BulkOperations, BulkParts, GetParts, SearchParts};
use serde::ser::Error;
use serde_json::json;
use std::collections::HashMap;
use std::ops::Deref;
use strum::EnumCount;
use time::format_description::well_known;

#[async_trait]
#[mockall::automock]
pub trait ProductOpenSearchRepository {
    async fn create_product_documents(
        &self,
        documents: Vec<ProductDocument>,
    ) -> Result<BulkResponse, opensearch::Error>;

    async fn update_product_documents(
        &self,
        updates: HashMap<ProductId, ProductUpdateDocument>,
    ) -> Result<BulkResponse, opensearch::Error>;

    async fn search_product_documents(
        &self,
        search: &ProductSearch,
        sort: &Sort<SortProductField>,
        page: &Option<Cursor<serde_json::Value>>,
    ) -> Result<SearchResponse<ProductDocument>, opensearch::Error>;

    async fn get_product_document_by_id(
        &self,
        product_id: &ProductId,
    ) -> Result<ProductDocument, opensearch::Error>;

    async fn k_nn_text(
        &self,
        text_embedding: &[f32],
        k: u16,
    ) -> Result<SearchResponse<ProductDocument>, opensearch::Error>;
}

pub struct ProductOpenSearchRepositoryImpl<'a> {
    client: &'a opensearch::OpenSearch,
}

impl<'a> ProductOpenSearchRepositoryImpl<'a> {
    pub fn new(client: &'a opensearch::OpenSearch) -> Self {
        ProductOpenSearchRepositoryImpl { client }
    }
}

#[async_trait]
impl<'a> ProductOpenSearchRepository for ProductOpenSearchRepositoryImpl<'a> {
    async fn create_product_documents(
        &self,
        documents: Vec<ProductDocument>,
    ) -> Result<BulkResponse, opensearch::Error> {
        let mut ops = BulkOperations::new();

        for doc in documents {
            ops.push(BulkOperation::create(doc._id(), &doc))?;
        }

        let response = self
            .client
            .bulk(BulkParts::Index("products"))
            .body(vec![ops])
            .send()
            .await?;

        let payload = response.text().await?;
        let bulk_response = serde_json::from_str::<BulkResponse>(&payload)
            .map_err(|err| {
                serde_json::Error::custom(format!(
                    "Failed deserializing 'BulkResponse' with error '{err}'. Received payload: {payload}"
                ))
            })?;

        Ok(bulk_response)
    }

    async fn update_product_documents(
        &self,
        updates: HashMap<ProductId, ProductUpdateDocument>,
    ) -> Result<BulkResponse, opensearch::Error> {
        let mut ops = BulkOperations::new();
        for (_id, doc) in updates {
            ops.push(BulkOperation::update(
                _id,
                json!({
                "doc": doc
                }),
            ))?;
        }

        let response = self
            .client
            .bulk(BulkParts::Index("products"))
            .body(vec![ops])
            .send()
            .await?;

        let payload = response.text().await?;
        let bulk_response = serde_json::from_str::<BulkResponse>(&payload)
            .map_err(|err| {
                serde_json::Error::custom(format!(
                    "Failed deserializing 'BulkResponse' with error '{err}'. Received payload: {payload}"
                ))
            })?;

        Ok(bulk_response)
    }

    async fn search_product_documents(
        &self,
        search: &ProductSearch,
        sort: &Sort<SortProductField>,
        cursor: &Option<Cursor<serde_json::Value>>,
    ) -> Result<SearchResponse<ProductDocument>, opensearch::Error> {
        let mut must = Vec::with_capacity(3);
        let mut filter = Vec::with_capacity(10);

        let (title_field, description_field) = match search.language {
            Language::De => ("titleDe", "descriptionDe"),
            Language::En => ("titleEn", "descriptionEn"),
            _ => ("titleDe", "descriptionDe"),
        };
        must.push(json!({
            "multi_match": {
                "query": search.product_query.as_ref(),
                "fields": [
                    format!("{title_field}^3"),
                    format!("{description_field}^1"),
                ],
                "fuzziness": "AUTO",
                "minimum_should_match": "70%"
            }
        }));

        if let Some(shop_name_query) = &search.shop_name_query {
            must.push(json!({
                "match": {
                    "shopName": {
                        "query": shop_name_query.deref(),
                        "fuzziness": "AUTO",
                        "operator": "and"
                    }
                }
            }));
        }

        match search
            .state_query
            .iter()
            .collect::<Vec<&ProductState>>()
            .as_slice()
        {
            [] => {}
            states if states.len() == ProductState::COUNT => {}
            states => {
                let state_values: Vec<&str> = states
                    .iter()
                    .map(|state| ProductStateDocument::from(**state))
                    .map(|s| s.as_str())
                    .collect();

                filter.push(json!({
                    "terms": { "state": state_values }
                }));
            }
        }

        let price_field = match search.currency {
            Currency::Eur => "priceEur",
            Currency::Gbp => "priceGbp",
            Currency::Usd => "priceUsd",
            Currency::Aud => "priceAud",
            Currency::Cad => "priceCad",
            Currency::Nzd => "priceNzd",
        };
        if let Some(min) = search.price_query.and_then(|price_query| price_query.min) {
            filter.push(json!({
                "range": { price_field: { "gte": min.deref() } }
            }));
        }
        if let Some(max) = search.price_query.and_then(|price_query| price_query.max) {
            filter.push(json!({
                "range": { price_field: { "lte": max.deref() } }
            }));
        }

        if let Some(min) = search
            .created_query
            .and_then(|created_query| created_query.min)
        {
            let formatted_min = min
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            filter.push(json!({
                "range": { "created": { "gte": formatted_min } }
            }));
        }
        if let Some(max) = search
            .created_query
            .and_then(|created_query| created_query.max)
        {
            let formatted_max = max
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            filter.push(json!({
                "range": { "created": { "lte": formatted_max } }
            }));
        }

        if let Some(min) = search
            .updated_query
            .and_then(|updated_query| updated_query.min)
        {
            let formatted_min = min
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            filter.push(json!({
                "range": { "updated": { "gte": formatted_min } }
            }));
        }
        if let Some(max) = search
            .updated_query
            .and_then(|updated_query| updated_query.max)
        {
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

        if let Some(c) = cursor {
            body.as_object_mut()
                .unwrap()
                .insert("size".to_string(), json!(c.size));

            if let Some(search_after) = &c.search_after {
                body.as_object_mut()
                    .unwrap()
                    .insert("search_after".to_string(), json!(search_after));
            }
        }

        let sort_field = match sort.sort {
            SortProductField::Score => "_score",
            SortProductField::Price => price_field,
            SortProductField::Created => "created",
            SortProductField::Updated => "updated",
        };
        let order = match sort.order {
            SortOrder::Asc => "asc",
            SortOrder::Desc => "desc",
        };
        let primary_sort = if matches!(sort.sort, SortProductField::Score) {
            json!({ sort_field: { "order": order } })
        } else {
            json!({ sort_field: { "order": order, "missing": "_last" } })
        };
        body.as_object_mut().unwrap().insert(
            "sort".to_string(),
            json!([
                primary_sort,
                { "productId": { "order": "asc" } } // tie-breaker
            ]),
        );

        let response = self
            .client
            .search(SearchParts::Index(&["products"]))
            .body(body)
            .send()
            .await?;
        let payload = response.text().await?;

        let search_response = serde_json::from_str::<SearchResponse<ProductDocument>>(&payload)
            .map_err(|err| {
                serde_json::Error::custom(format!(
                    "Failed deserializing 'SearchResponse<ProductDocument>' with error '{err}'. Received payload: {payload}"
                ))
            })?;

        Ok(search_response)
    }

    async fn get_product_document_by_id(
        &self,
        product_id: &ProductId,
    ) -> Result<ProductDocument, opensearch::Error> {
        let mut response: serde_json::Value = self
            .client
            .get(GetParts::IndexId("products", &product_id.to_string()))
            .send()
            .await?
            .error_for_status_code()?
            .json()
            .await?;

        serde_json::from_value(response["_source"].take()).map_err(opensearch::Error::from)
    }

    async fn k_nn_text(
        &self,
        text_embedding: &[f32],
        k: u16,
    ) -> Result<SearchResponse<ProductDocument>, opensearch::Error> {
        let body = json!({
            "size": k,
            "query": {
              "knn": {
                "textEmbedding": {
                  "vector": text_embedding,
                  "k": k,
                }
              }
            }
        });
        let response = self
            .client
            .search(SearchParts::Index(&["products"]))
            .body(body)
            .send()
            .await?;
        let payload = response.text().await?;
        let knn_response = serde_json::from_str::<SearchResponse<ProductDocument>>(&payload)
            .map_err(|err| {
                serde_json::Error::custom(format!(
                    "Failed deserializing 'SearchResponse<ProductDocument>' with error '{err}'. Received payload: {payload}"
                ))
            })?;
        Ok(knn_response)
    }
}
