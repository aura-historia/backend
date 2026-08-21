use crate::core::{sort_user_field::SortUserField, user_search::UserSearch};
use crate::opensearch::{
    role_document::UserRoleDocument,
    tier_document::UserTierDocument,
    user_document::{UserDocument, UserDocumentSerdeField},
};
use common::opensearch::delete_response::DeleteResponse;
use common::opensearch::index_response::IndexResponse;
use common::opensearch::search_response::SearchResponse;
use common::pagination::cursor::Cursor;
use common::sort::{Sort, SortOrder};
use common::user_id::UserId;
use geo::data::continent_data::ContinentData;
use geo::opensearch::distance_to_opensearch_value;
use opensearch::{DeleteParts, IndexParts, SearchParts};
use serde::ser::Error;
use serde_json::json;
use time::format_description::well_known;

#[async_trait::async_trait]
#[mockall::automock]
pub trait UserOpenSearchRepository {
    async fn index_user_document(
        &self,
        document: UserDocument,
    ) -> Result<IndexResponse, opensearch::Error>;

    async fn delete_user_document(&self, id: &UserId) -> Result<DeleteResponse, opensearch::Error>;

    async fn search_user_documents(
        &self,
        search: &UserSearch,
        sort: &Sort<SortUserField>,
        cursor: &Option<Cursor<serde_json::Value>>,
    ) -> Result<SearchResponse<UserDocument>, opensearch::Error>;
}

#[derive(Debug, Clone)]
pub struct UserOpenSearchRepositoryImpl<'a> {
    client: &'a opensearch::OpenSearch,
}

impl<'a> UserOpenSearchRepositoryImpl<'a> {
    pub fn new(client: &'a opensearch::OpenSearch) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl<'a> UserOpenSearchRepository for UserOpenSearchRepositoryImpl<'a> {
    async fn index_user_document(
        &self,
        document: UserDocument,
    ) -> Result<IndexResponse, opensearch::Error> {
        let response = self
            .client
            .index(IndexParts::IndexId("users", &document._id().to_string()))
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

    async fn delete_user_document(&self, id: &UserId) -> Result<DeleteResponse, opensearch::Error> {
        let response = self
            .client
            .delete(DeleteParts::IndexId("users", &id.to_string()))
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

    async fn search_user_documents(
        &self,
        search: &UserSearch,
        sort: &Sort<SortUserField>,
        cursor: &Option<Cursor<serde_json::Value>>,
    ) -> Result<SearchResponse<UserDocument>, opensearch::Error> {
        let mut must = Vec::with_capacity(4);
        let mut filter = Vec::with_capacity(6);

        if let Some(query) = search.query.as_ref() {
            must.push(json!({
                "multi_match": {
                    "query": query,
                    "fields": ["email^3", "firstName^2", "lastName^2", "stripeCustomerId"],
                    "fuzziness": "AUTO",
                    "minimum_should_match": "70%"
                }
            }));
        }
        if let Some(query) = search.email_query.as_ref() {
            must.push(json!({ "match": { "email": { "query": query, "fuzziness": "AUTO", "minimum_should_match": "70%" } } }));
        }
        if let Some(query) = search.first_name_query.as_ref() {
            must.push(json!({ "match": { "firstName": { "query": query, "fuzziness": "AUTO", "minimum_should_match": "70%" } } }));
        }
        if let Some(query) = search.last_name_query.as_ref() {
            must.push(json!({ "match": { "lastName": { "query": query, "fuzziness": "AUTO", "minimum_should_match": "70%" } } }));
        }

        if !search.tier_query.is_empty() {
            let tiers: Vec<UserTierDocument> = search
                .tier_query
                .iter()
                .copied()
                .map(UserTierDocument::from)
                .collect();
            filter.push(json!({ "terms": { UserDocumentSerdeField::Tier.as_str(): tiers } }));
        }
        if !search.role_query.is_empty() {
            let roles: Vec<UserRoleDocument> = search
                .role_query
                .iter()
                .copied()
                .map(UserRoleDocument::from)
                .collect();
            filter.push(json!({ "terms": { UserDocumentSerdeField::Role.as_str(): roles } }));
        }
        if !search.country_query.is_empty() {
            filter.push(json!({
                "terms": {
                    UserDocumentSerdeField::StructuredAddressCountry.as_str(): search.country_query.iter().map(|c| c.alpha2()).collect::<Vec<_>>()
                }
            }));
        }
        if !search.continent_query.is_empty() {
            let continents: Vec<ContinentData> = search
                .continent_query
                .iter()
                .copied()
                .map(ContinentData::from)
                .collect();
            filter.push(json!({
                "terms": {
                    UserDocumentSerdeField::StructuredAddressContinent.as_str(): continents
                }
            }));
        }
        if let Some(query) = search.geo_address_distance_query {
            filter.push(json!({
                "geo_distance": {
                    "distance": distance_to_opensearch_value(query.distance),
                    UserDocumentSerdeField::GeoAddress.as_str(): {
                        "lat": query.lat,
                        "lon": query.lon
                    }
                }
            }));
        }

        if let Some(min) = search.created.and_then(|created| created.min) {
            let formatted_min = min
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            filter.push(json!({ "range": { UserDocumentSerdeField::Created.as_str(): { "gte": formatted_min } } }));
        }
        if let Some(max) = search.created.and_then(|created| created.max) {
            let formatted_max = max
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            filter.push(json!({ "range": { UserDocumentSerdeField::Created.as_str(): { "lte": formatted_max } } }));
        }
        if let Some(min) = search.updated.and_then(|updated| updated.min) {
            let formatted_min = min
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            filter.push(json!({ "range": { UserDocumentSerdeField::Updated.as_str(): { "gte": formatted_min } } }));
        }
        if let Some(max) = search.updated.and_then(|updated| updated.max) {
            let formatted_max = max
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            filter.push(json!({ "range": { UserDocumentSerdeField::Updated.as_str(): { "lte": formatted_max } } }));
        }

        let mut body = json!({ "query": { "bool": { "must": must, "filter": filter } } });
        if let Some(c) = cursor {
            let body_obj = body.as_object_mut().unwrap();
            body_obj.insert("size".to_string(), json!(c.size));
            if let Some(search_after) = &c.search_after {
                body_obj.insert("search_after".to_string(), json!(search_after));
            }
        }

        let sort_field = match sort.sort {
            SortUserField::Score => "_score",
            SortUserField::Email => "email.keyword",
            SortUserField::FirstName => "firstName.keyword",
            SortUserField::LastName => "lastName.keyword",
            SortUserField::Tier => UserDocumentSerdeField::Tier.as_str(),
            SortUserField::Role => UserDocumentSerdeField::Role.as_str(),
            SortUserField::Created => UserDocumentSerdeField::Created.as_str(),
            SortUserField::Updated => UserDocumentSerdeField::Updated.as_str(),
        };
        let order = match sort.order {
            SortOrder::Asc => "asc",
            SortOrder::Desc => "desc",
        };
        let primary_sort = if matches!(sort.sort, SortUserField::Score) {
            json!({ sort_field: { "order": order } })
        } else {
            json!({ sort_field: { "order": order, "missing": "_last" } })
        };
        body.as_object_mut().unwrap().insert(
            "sort".to_string(),
            json!([primary_sort, { UserDocumentSerdeField::UserId.as_str(): { "order": "asc" } }]),
        );

        let response = self
            .client
            .search(SearchParts::Index(&["users"]))
            .body(body)
            .send()
            .await?
            .error_for_status_code()?;
        let payload = response.text().await?;
        let search_response = serde_json::from_str::<SearchResponse<UserDocument>>(&payload).map_err(|err| {
            serde_json::Error::custom(format!(
                "Failed deserializing 'SearchResponse<UserDocument>' with error '{err}'. Received payload: {payload}"
            ))
        })?;

        Ok(search_response)
    }
}
