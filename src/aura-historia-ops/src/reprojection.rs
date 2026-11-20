//! `reproject-all`: rebuilds OpenSearch, or fills an isolated index generation, from PostgreSQL.

use crate::OpsError;
use platform_worker_queue_contract::job_queue::ProjectionTarget;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_postgres::SqlxProductListingProjectionTriggerReader;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::num::NonZeroU32;

pub(crate) async fn reproject_all(
    pool: &PgPool,
    target: &ProjectionTarget,
    page_size: NonZeroU32,
    start_after: Option<ProductListingId>,
) -> Result<Value, OpsError> {
    let reader = SqlxProductListingProjectionTriggerReader::new(pool.clone());
    let report = delivery_outbox::reproject_all(&reader, pool, target, page_size, start_after)
        .await
        .map_err(|error| OpsError::failed("reproject-all", error))?;
    Ok(json!({
        "scope": target.job_scope().name().as_str(),
        "pages": report.pages,
        "enqueued": report.enqueued,
        "deduplicated": report.deduplicated,
        // Pass as --start-after to resume an interrupted run.
        "last": report.last.map(|id| id.to_string()),
    }))
}
