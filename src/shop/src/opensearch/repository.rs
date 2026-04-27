use crate::core::shop_search::ShopSearch;
use crate::core::sort_shop_field::SortShopField;
use crate::opensearch::continent_document::ContinentDocument;
use crate::opensearch::partner_status_document::ShopPartnerStatusDocument;
use crate::opensearch::shop_document::ShopDocument;
use crate::opensearch::shop_document::ShopDocumentSerdeField;
use crate::opensearch::shop_document_update::ShopDocumentUpdate;
use crate::opensearch::shop_type_document::ShopTypeDocument;
use common::opensearch::update_response::UpdateResponse;
use common::shop_id::ShopId;
use common::{
    opensearch::{index_response::IndexResponse, search_response::SearchResponse},
    pagination::cursor::Cursor,
    sort::{Sort, SortOrder},
};
use opensearch::{IndexParts, SearchParts, UpdateParts};
use serde::ser::Error;
use serde_json::json;
use time::format_description::well_known;

#[async_trait::async_trait]
#[mockall::automock]
pub trait ShopOpenSearchRepository {
    async fn index_shop_document(
        &self,
        document: ShopDocument,
    ) -> Result<IndexResponse, opensearch::Error>;

    async fn update_shop_document(
        &self,
        shop_id: &ShopId,
        update: ShopDocumentUpdate,
    ) -> Result<UpdateResponse, opensearch::Error>;

    async fn search_shop_documents(
        &self,
        search: &ShopSearch,
        sort: &Sort<SortShopField>,
        cursor: &Option<Cursor<serde_json::Value>>,
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
    async fn index_shop_document(
        &self,
        document: ShopDocument,
    ) -> Result<IndexResponse, opensearch::Error> {
        let response = self
            .client
            .index(IndexParts::IndexId("shops", &document._id().to_string()))
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

    async fn update_shop_document(
        &self,
        shop_id: &ShopId,
        update: ShopDocumentUpdate,
    ) -> Result<UpdateResponse, opensearch::Error> {
        let response = self
            .client
            .update(UpdateParts::IndexId("shops", &shop_id.to_string()))
            .body(json!({
                "doc": update
            }))
            .send()
            .await?
            .error_for_status_code()?;

        let payload = response.text().await?;
        let update_response = serde_json::from_str::<UpdateResponse>(&payload)
            .map_err(|err| {
                serde_json::Error::custom(format!(
                    "Failed deserializing 'UpdateResponse' with error '{err}'. Received payload: {payload}"
                ))
            })?;

        Ok(update_response)
    }

    async fn search_shop_documents(
        &self,
        search: &ShopSearch,
        sort: &Sort<SortShopField>,
        cursor: &Option<Cursor<serde_json::Value>>,
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

        // Add shop_type filter
        if !search.shop_type_query.is_empty() {
            let shop_types: Vec<&str> = search
                .shop_type_query
                .iter()
                .map(|v| ShopTypeDocument::from(*v).as_str())
                .collect();
            filter.push(json!({
                "terms": {
                    ShopDocumentSerdeField::ShopType.as_str(): shop_types
                }
            }));
        }

        // Add partner_status filter
        if !search.partner_status_query.is_empty() {
            let partner_statuses: Vec<&str> = search
                .partner_status_query
                .iter()
                .map(|v| ShopPartnerStatusDocument::from(*v).as_str())
                .collect();
            filter.push(json!({
                "terms": {
                    ShopDocumentSerdeField::PartnerStatus.as_str(): partner_statuses
                }
            }));
        }

        // Add specialities_categories filter
        if !search.specialities_categories.is_empty() {
            let categories: Vec<String> = search
                .specialities_categories
                .iter()
                .map(|c| c.to_string())
                .collect();
            filter.push(json!({
                "terms": {
                    ShopDocumentSerdeField::SpecialitiesCategories.as_str(): categories
                }
            }));
        }

        // Add specialities_periods filter
        if !search.specialities_periods.is_empty() {
            let periods: Vec<String> = search
                .specialities_periods
                .iter()
                .map(|p| p.to_string())
                .collect();
            filter.push(json!({
                "terms": {
                    ShopDocumentSerdeField::SpecialitiesPeriods.as_str(): periods
                }
            }));
        }

        // Add country filter
        if !search.countries.is_empty() {
            let countries: Vec<&str> = search.countries.iter().map(|c| c.alpha2()).collect();
            filter.push(json!({
                "terms": {
                    ShopDocumentSerdeField::StructuredAddressCountry.as_str(): countries
                }
            }));
        }

        // Add continent filter
        if !search.continents.is_empty() {
            let continents: Vec<&str> = search
                .continents
                .iter()
                .map(|c| ContinentDocument::from(*c).as_str())
                .collect();
            filter.push(json!({
                "terms": {
                    ShopDocumentSerdeField::StructuredAddressContinent.as_str(): continents
                }
            }));
        }

        if let Some(min) = search.created.and_then(|created| created.min) {
            let formatted_min = min
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            filter.push(json!({
                "range": { ShopDocumentSerdeField::Created.as_str() : { "gte": formatted_min } }
            }));
        }
        if let Some(max) = search.created.and_then(|created| created.max) {
            let formatted_max = max
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            filter.push(json!({
                "range": { ShopDocumentSerdeField::Created.as_str() : { "lte": formatted_max } }
            }));
        }

        if let Some(min) = search.updated.and_then(|updated| updated.min) {
            let formatted_min = min
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            filter.push(json!({
                "range": { ShopDocumentSerdeField::Updated.as_str() : { "gte": formatted_min } }
            }));
        }
        if let Some(max) = search.updated.and_then(|updated| updated.max) {
            let formatted_max = max
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            filter.push(json!({
                "range": { ShopDocumentSerdeField::Updated.as_str() : { "lte": formatted_max } }
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
            SortShopField::Score => "_score",
            SortShopField::Name => "name.keyword",
            SortShopField::Created => ShopDocumentSerdeField::Created.as_str(),
            SortShopField::Updated => ShopDocumentSerdeField::Updated.as_str(),
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
                { ShopDocumentSerdeField::ShopId.as_str() : { "order": "asc" } } // tie-breaker
            ]),
        );

        let response = self
            .client
            .search(SearchParts::Index(&["shops"]))
            .body(body)
            .send()
            .await?
            .error_for_status_code()?;
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
