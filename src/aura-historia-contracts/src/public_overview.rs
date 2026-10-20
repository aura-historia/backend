use product_listing_service::use_cases::PublicCatalogueOverview;
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
            live_objects: value.live_objects,
            countries: value.countries,
            languages: value.languages,
            new_objects_last_7_days: value.new_objects_last_7_days,
        }
    }
}
