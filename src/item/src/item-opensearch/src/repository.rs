use crate::item_document::ItemDocument;
use crate::item_state_document::ItemStateDocument;
use crate::item_update_document::ItemUpdateDocument;
use async_trait::async_trait;
use common::currency::domain::Currency;
use common::item_id::ItemId;
use common::item_state::domain::ItemState;
use common::language::domain::Language;
use common::opensearch::{bulk_response::BulkResponse, search_response::SearchResponse};
use common::pagination::cursor::Cursor;
use common::sort::{Sort, SortOrder};
use item_core::sort_item_field::SortItemField;
use opensearch::{BulkOperation, BulkOperations, BulkParts, SearchParts};
use search_filter_core::search_filter::SearchFilter;
use serde::ser::Error;
use serde_json::json;
use std::collections::HashMap;
use std::ops::Deref;
use time::format_description::well_known;

#[async_trait]
#[mockall::automock]
pub trait ItemOpenSearchRepository {
    async fn create_item_documents(
        &self,
        documents: Vec<ItemDocument>,
    ) -> Result<BulkResponse, opensearch::Error>;

    async fn update_item_documents(
        &self,
        updates: HashMap<ItemId, ItemUpdateDocument>,
    ) -> Result<BulkResponse, opensearch::Error>;

    async fn search_item_documents(
        &self,
        search_filter: &SearchFilter,
        sort: &Sort<SortItemField>,
        page: &Option<Cursor<serde_json::Value>>,
    ) -> Result<SearchResponse<ItemDocument>, opensearch::Error>;
}

pub struct ItemOpenSearchRepositoryImpl<'a> {
    client: &'a opensearch::OpenSearch,
}

impl<'a> ItemOpenSearchRepositoryImpl<'a> {
    pub fn new(client: &'a opensearch::OpenSearch) -> Self {
        ItemOpenSearchRepositoryImpl { client }
    }
}

#[async_trait]
impl<'a> ItemOpenSearchRepository for ItemOpenSearchRepositoryImpl<'a> {
    async fn create_item_documents(
        &self,
        documents: Vec<ItemDocument>,
    ) -> Result<BulkResponse, opensearch::Error> {
        let mut ops = BulkOperations::new();

        for doc in documents {
            ops.push(BulkOperation::create(doc._id(), &doc))?;
        }

        let response = self
            .client
            .bulk(BulkParts::Index("items"))
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

    async fn update_item_documents(
        &self,
        updates: HashMap<ItemId, ItemUpdateDocument>,
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
            .bulk(BulkParts::Index("items"))
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

    async fn search_item_documents(
        &self,
        search_filter: &SearchFilter,
        sort: &Sort<SortItemField>,
        cursor: &Option<Cursor<serde_json::Value>>,
    ) -> Result<SearchResponse<ItemDocument>, opensearch::Error> {
        let mut must = Vec::with_capacity(3);
        let mut filter = Vec::with_capacity(10);

        let (title_field, description_field) = match search_filter.language {
            Language::De => ("titleDe", "descriptionDe"),
            Language::En => ("titleEn", "descriptionEn"),
            _ => ("titleDe", "descriptionDe"),
        };
        must.push(json!({
            "multi_match": {
                "query": search_filter.item_query.as_ref(),
                "fields": [
                    format!("{title_field}^3"),
                    format!("{description_field}^1"),
                ],
                "fuzziness": "AUTO",
                "minimum_should_match": "70%"
            }
        }));

        if let Some(shop_name_query) = &search_filter.shop_name_query {
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

        match search_filter
            .state_query
            .iter()
            .collect::<Vec<&ItemState>>()
            .as_slice()
        {
            [] => {}
            [ItemState::Available] => {
                filter.push(json!({
                    "term": { "isAvailable": true }
                }));
            }
            states => {
                let state_values: Vec<&str> = states
                    .iter()
                    .map(|state| ItemStateDocument::from(**state))
                    .map(|s| s.as_str())
                    .collect();

                filter.push(json!({
                    "terms": { "state": state_values }
                }));
            }
        }

        let price_field = match search_filter.currency {
            Currency::Eur => "priceEur",
            Currency::Gbp => "priceGbp",
            Currency::Usd => "priceUsd",
            Currency::Aud => "priceAud",
            Currency::Cad => "priceCad",
            Currency::Nzd => "priceNzd",
        };
        if let Some(min) = search_filter
            .price_query
            .and_then(|price_query| price_query.min)
        {
            filter.push(json!({
                "range": { price_field: { "gte": min.deref() } }
            }));
        }
        if let Some(max) = search_filter
            .price_query
            .and_then(|price_query| price_query.max)
        {
            filter.push(json!({
                "range": { price_field: { "lte": max.deref() } }
            }));
        }

        if let Some(min) = search_filter
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
        if let Some(max) = search_filter
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

        if let Some(min) = search_filter
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
        if let Some(max) = search_filter
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
            SortItemField::Score => "_score",
            SortItemField::Price => price_field,
            SortItemField::Created => "created",
            SortItemField::Updated => "updated",
        };
        let order = match sort.order {
            SortOrder::Asc => "asc",
            SortOrder::Desc => "desc",
        };
        let primary_sort = if matches!(sort.sort, SortItemField::Score) {
            json!({ sort_field: { "order": order } })
        } else {
            json!({ sort_field: { "order": order, "missing": "_last" } })
        };

        body.as_object_mut().unwrap().insert(
            "sort".to_string(),
            json!([
                primary_sort,
                { "itemId": { "order": order } } // tie-breaker
            ]),
        );

        let response = self
            .client
            .search(SearchParts::Index(&["items"]))
            .body(body)
            .send()
            .await?;
        let payload = response.text().await?;

        let search_response = serde_json::from_str::<SearchResponse<ItemDocument>>(&payload)
            .map_err(|err| {
                serde_json::Error::custom(format!(
                    "Failed deserializing 'SearchResponse<ItemDocument>' with error '{err}'. Received payload: {payload}"
                ))
            })?;

        Ok(search_response)
    }
}
