use super::{SavedSearchInputError, SavedSearchInputErrorKind};
use crate::product_listings::product_data::{
    PersonalizedProductListingDetailsData, personalized_product_details_data,
};
use application::pagination::Cursor;
use domain_primitives::sort::SortOrder;
use localization::Language;
use money::Currency;
use schemars::JsonSchema;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::{
    ports::SearchFilterMatchCursor,
    use_cases::{ListSearchFilterMatchesRequest, ListSearchFilterMatchesResult},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use user_core::user_id::UserId;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchFilterMatchesQuery {
    #[serde(default, with = "crate::wire::language")]
    #[schemars(schema_with = "crate::wire::language::schema")]
    pub language: Language,
    #[serde(default, with = "crate::wire::currency")]
    #[schemars(schema_with = "crate::wire::currency::schema")]
    pub currency: Currency,
    /// Defaults to 21 and clamps to 1..=100.
    #[serde(default)]
    #[schemars(range(max = u64::MAX))]
    pub size: Option<u64>,
}
impl SearchFilterMatchesQuery {
    pub fn into_request(
        self,
        user_id: UserId,
        search_filter_id: UserSearchFilterId,
        search_after: Option<Value>,
    ) -> Result<ListSearchFilterMatchesRequest, SavedSearchInputError> {
        Ok(ListSearchFilterMatchesRequest {
            user_id,
            search_filter_id,
            language: self.language,
            currency: self.currency,
            cursor: Some(self.cursor(search_after)?),
            order: SortOrder::Asc,
        })
    }
    pub fn cursor(
        &self,
        search_after: Option<Value>,
    ) -> Result<Cursor<SearchFilterMatchCursor>, SavedSearchInputError> {
        Ok(Cursor {
            size: self.size.unwrap_or(21).clamp(1, 100),
            search_after: search_after.map(parse_matches_cursor).transpose()?,
        })
    }
}

/// Exact two-part JSON continuation returned by REST and MCP; REST query-encodes it.
pub fn parse_matches_cursor(
    value: Value,
) -> Result<SearchFilterMatchCursor, SavedSearchInputError> {
    let invalid = |detail: &str| {
        SavedSearchInputError::new(SavedSearchInputErrorKind::QueryValue)
            .with_query_field("searchAfter")
            .with_detail(detail)
    };
    let Value::Array(values) = value else {
        return Err(invalid(
            "searchAfter must be a JSON array containing timestamp and product ID.",
        ));
    };
    let [Value::String(created), Value::String(product_listing_id)] = values.as_slice() else {
        return Err(invalid(
            "searchAfter must contain an RFC3339 timestamp and ProductListing ID.",
        ));
    };
    let created =
        OffsetDateTime::parse(created, &Rfc3339).map_err(|error| invalid(&error.to_string()))?;
    let product_listing_id = product_listing_id.parse().map_err(|_| {
        SavedSearchInputError::new(SavedSearchInputErrorKind::Identifier)
            .with_query_field("searchAfter")
            .with_detail("must be a valid ProductListing ID")
    })?;
    Ok(SearchFilterMatchCursor {
        created,
        product_listing_id,
    })
}

pub fn match_cursor_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"anyOf":[{"type":"array","prefixItems":[crate::wire::schema::datetime(generator),crate::wire::schema::product_listing_id(generator)],"minItems":2,"maxItems":2},{"type":"null"}]})
}

#[derive(Debug, thiserror::Error)]
#[error("search-filter match cursor failed internally")]
pub struct MatchCursorOutputError(#[source] time::error::Format);

pub fn matches_cursor_value(
    cursor: SearchFilterMatchCursor,
) -> Result<Value, MatchCursorOutputError> {
    cursor
        .created
        .format(&Rfc3339)
        .map(|created| json!([created, cursor.product_listing_id]))
        .map_err(MatchCursorOutputError)
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchFilterMatchesData {
    pub items: Vec<PersonalizedProductListingDetailsData>,
    #[schemars(range(max = u64::MAX))]
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "match_cursor_schema")]
    pub search_after: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(range(max = u64::MAX))]
    pub total: Option<u64>,
}
impl TryFrom<ListSearchFilterMatchesResult> for SearchFilterMatchesData {
    type Error = MatchCursorOutputError;
    fn try_from(result: ListSearchFilterMatchesResult) -> Result<Self, Self::Error> {
        let items = result
            .items
            .into_iter()
            .map(personalized_product_details_data)
            .collect::<Vec<_>>();
        Ok(Self {
            size: items.len() as u64,
            items,
            search_after: result
                .cursor
                .search_after
                .map(matches_cursor_value)
                .transpose()?,
            total: result.total,
        })
    }
}
