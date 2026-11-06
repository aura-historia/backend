use product_listing_service::use_cases::{PublicCatalogueOverview, SourcesIndexedByType};
use schemars::JsonSchema;
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicOverviewData {
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::datetime")]
    as_of: OffsetDateTime,
    #[schemars(range(max = u64::MAX))]
    sources_indexed: u64,
    sources_indexed_by_type: SourcesIndexedByTypeData,
    #[schemars(range(max = u64::MAX))]
    live_objects: u64,
    #[schemars(range(max = u64::MAX))]
    countries: u64,
    #[schemars(range(max = u64::MAX))]
    languages: u64,
    #[schemars(range(max = u64::MAX))]
    new_objects_last_7_days: u64,
}

impl From<PublicCatalogueOverview> for PublicOverviewData {
    fn from(value: PublicCatalogueOverview) -> Self {
        Self {
            as_of: value.as_of,
            sources_indexed: value.sources_indexed,
            sources_indexed_by_type: value.sources_indexed_by_type.into(),
            live_objects: value.live_objects,
            countries: value.countries,
            languages: value.languages,
            new_objects_last_7_days: value.new_objects_last_7_days,
        }
    }
}

/// `sourcesIndexed` split by ListingSource type; the values sum to `sourcesIndexed`.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SourcesIndexedByTypeData {
    #[schemars(range(max = u64::MAX))]
    dealers: u64,
    #[schemars(range(max = u64::MAX))]
    auction_houses: u64,
    #[schemars(range(max = u64::MAX))]
    marketplaces: u64,
    #[schemars(range(max = u64::MAX))]
    auction_platforms: u64,
}

impl From<SourcesIndexedByType> for SourcesIndexedByTypeData {
    fn from(value: SourcesIndexedByType) -> Self {
        Self {
            dealers: value.dealers,
            auction_houses: value.auction_houses,
            marketplaces: value.marketplaces,
            auction_platforms: value.auction_platforms,
        }
    }
}
