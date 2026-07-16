use common::shop_id::ShopId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::RemovedPageSchema;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShopsRemovedPageSchema {
    pub shop_id: ShopId,
    pub removed_page_schemas: Vec<RemovedPageSchema>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}
