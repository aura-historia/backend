//! Public source search and detail contracts. Management fields have a separate owner.
pub mod cursor;
pub mod location_default;

use listing_source_core::{ListingSourceId, ListingSourceSlugId};
use listing_source_service::use_cases::queries::{
    get_public_listing_source_by_slug::GetPublicListingSourceBySlugRequest,
    public_listing_source::PublicListingSourceSummary,
    search_public_listing_sources::{
        DEFAULT_PUBLIC_LISTING_SOURCE_SEARCH_PAGE_SIZE, PublicListingSourceSearchQuery,
        PublicListingSourceSearchQueryError, SearchPublicListingSourcesRequest,
        SearchPublicListingSourcesRequestError, SearchPublicListingSourcesResult,
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchPublicListingSourcesData {
    /// Optional source-name text; omission or whitespace browses, insufficient text returns no matches.
    #[serde(default)]
    pub query: Option<String>,
    /// Page size from 1 through 50. Continuations are bound to the query and page size.
    #[serde(default = "default_size")]
    pub size: u8,
    #[serde(default)]
    pub search_after: Option<String>,
}

fn default_size() -> u8 {
    DEFAULT_PUBLIC_LISTING_SOURCE_SEARCH_PAGE_SIZE
}

#[derive(Debug, thiserror::Error)]
pub enum SourceSearchInputError {
    #[error("Query parameter is invalid.")]
    Query(#[source] PublicListingSourceSearchQueryError),
    #[error(transparent)]
    Cursor(#[from] cursor::SourceCursorError),
    #[error("Search continuation is invalid for this request.")]
    Continuation(#[source] SearchPublicListingSourcesRequestError),
    #[error("Size must be an integer from 1 through 50.")]
    Size(#[source] SearchPublicListingSourcesRequestError),
}

impl SourceSearchInputError {
    pub fn field(&self) -> &'static str {
        match self {
            Self::Query(_) => "query",
            Self::Size(_) => "size",
            Self::Cursor(_) | Self::Continuation(_) => "searchAfter",
        }
    }
}

impl TryFrom<SearchPublicListingSourcesData> for SearchPublicListingSourcesRequest {
    type Error = SourceSearchInputError;
    fn try_from(data: SearchPublicListingSourcesData) -> Result<Self, Self::Error> {
        let query = PublicListingSourceSearchQuery::new(data.query)
            .map_err(SourceSearchInputError::Query)?;
        let continuation = data
            .search_after
            .as_deref()
            .map(cursor::parse_continuation)
            .transpose()?;
        Self::new(query, data.size, continuation).map_err(|source| match source {
            source @ SearchPublicListingSourcesRequestError::PageSizeOutOfRange { .. } => {
                SourceSearchInputError::Size(source)
            }
            source => SourceSearchInputError::Continuation(source),
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GetPublicListingSourceBySlugData {
    pub listing_source_slug_id: String,
}

impl TryFrom<GetPublicListingSourceBySlugData> for GetPublicListingSourceBySlugRequest {
    type Error = listing_source_core::listing_source_slug_id::InvalidListingSourceSlug;
    fn try_from(data: GetPublicListingSourceBySlugData) -> Result<Self, Self::Error> {
        Ok(Self {
            slug_id: ListingSourceSlugId::raw(data.listing_source_slug_id)?,
        })
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicListingSourceData {
    #[schemars(schema_with = "crate::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    listing_source_slug_id: String,
    name: String,
    operator: PublicListingSourceOperatorData,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "crate::wire::schema::optional_url")]
    url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "crate::wire::schema::optional_url")]
    image: Option<Url>,
}

impl From<PublicListingSourceSummary> for PublicListingSourceData {
    fn from(value: PublicListingSourceSummary) -> Self {
        Self {
            listing_source_id: value.listing_source_id,
            listing_source_slug_id: value.listing_source_slug_id.to_string(),
            name: value.name.to_string(),
            operator: PublicListingSourceOperatorData {
                name: value.operator.name.to_string(),
            },
            url: value.url,
            image: value.image,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct PublicListingSourceOperatorData {
    name: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicListingSourceSearchCollectionData {
    items: Vec<PublicListingSourceData>,
    #[schemars(range(max = u64::MAX))]
    size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_after: Option<String>,
}

impl PublicListingSourceSearchCollectionData {
    pub fn new(result: SearchPublicListingSourcesResult, search_after: Option<String>) -> Self {
        let items = result.items.into_iter().map(Into::into).collect::<Vec<_>>();
        Self {
            size: items.len() as u64,
            items,
            search_after,
        }
    }
}

pub mod administered;
