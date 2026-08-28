use listing_source_core::ListingSourceId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::RemovedPageSchema;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListingSourceRemovedPageSchema {
    pub listing_source_id: ListingSourceId,
    pub removed_page_schemas: Vec<RemovedPageSchema>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}
