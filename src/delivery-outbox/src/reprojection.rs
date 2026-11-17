//! `reproject-all`: enqueues one projector job per existing listing, in keyset pages. It is how
//! OpenSearch is rebuilt from PostgreSQL after the index was lost, and how a separate index
//! generation is filled.

use aura_historia_cdc_routing::product_listing_projection_job;
use aura_historia_jobs::wire;
use platform_postgres_queue::{DedupKey, EnqueueOutcome, JobQueueError, NewJob, enqueue};
use platform_worker_queue_contract::job_queue::ProjectionTarget;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::ports::{
    ProductListingProjectionTrigger, ProductListingProjectionTriggerReadError,
    ProductListingProjectionTriggerReader,
};
use sqlx::PgPool;
use std::num::NonZeroU32;
use tracing::info;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReprojectionReport {
    pub pages: u64,
    /// Projector jobs inserted.
    pub enqueued: u64,
    /// Listings whose projector job from an earlier run was still unclaimed.
    pub deduplicated: u64,
    /// The last listing enqueued; pass it as `start_after` to resume an interrupted run.
    pub last: Option<ProductListingId>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReprojectionError {
    #[error("reprojection could not read a page of listings")]
    Read(#[source] ProductListingProjectionTriggerReadError),
    #[error("reprojection could not build a projector job")]
    InvalidJob(#[source] application::error::BoxError),
    #[error("reprojection could not enqueue a page of projector jobs")]
    Database(#[from] sqlx::Error),
    #[error("reprojection could not enqueue a projector job")]
    Enqueue(#[from] JobQueueError),
}

/// Enqueues a projector job for every listing after `start_after`, active or withdrawn, into
/// the target's scope, one committed page at a time.
///
/// The projection fences hold for these jobs as for routed ones: each job names the listing's
/// current event, so the projector writes current state only (a listing that changes meanwhile
/// completes as stale and its new event's own job projects it), and a withdrawn listing is
/// written as its versioned tombstone rather than skipped. Each job is deduplicated against an
/// unclaimed job of the same event in the target scope, so a rerun, the catch-up after old
/// writers are fenced, skips listings whose earlier job is still waiting.
pub async fn reproject_all(
    reader: &dyn ProductListingProjectionTriggerReader,
    pool: &PgPool,
    target: &ProjectionTarget,
    page_size: NonZeroU32,
    start_after: Option<ProductListingId>,
) -> Result<ReprojectionReport, ReprojectionError> {
    let scope = target.job_scope();
    let mut report = ReprojectionReport {
        last: start_after,
        ..ReprojectionReport::default()
    };
    loop {
        let page = reader
            .page_after(report.last, page_size.get())
            .await
            .map_err(ReprojectionError::Read)?;
        let Some(last) = page.last() else {
            break;
        };
        report.last = Some(last.product_listing_id);
        let jobs = page
            .iter()
            .map(|trigger| projector_job(&scope, trigger))
            .collect::<Result<Vec<_>, _>>()?;
        let mut transaction = pool.begin().await?;
        for job in &jobs {
            match enqueue(&mut *transaction, job).await? {
                EnqueueOutcome::Enqueued(_) => report.enqueued += 1,
                EnqueueOutcome::Deduplicated => report.deduplicated += 1,
            }
        }
        transaction.commit().await?;
        report.pages += 1;
        info!(
            scope = %scope.name(),
            pages = report.pages,
            enqueued = report.enqueued,
            deduplicated = report.deduplicated,
            last = %last.product_listing_id,
            "Reprojection page enqueued"
        );
        if page.len() < page_size.get() as usize {
            break;
        }
    }
    Ok(report)
}

/// The routed projector job's body, in the target's scope, deduplicated by its idempotency key.
fn projector_job(
    scope: &platform_postgres_queue::JobScope,
    trigger: &ProductListingProjectionTrigger,
) -> Result<NewJob, ReprojectionError> {
    let job = product_listing_projection_job(trigger.current_event_id, trigger.product_listing_id);
    let body = wire::encode(&job).map_err(invalid_job)?;
    let dedup_key = DedupKey::new(job.idempotency_key.as_str()).map_err(invalid_job)?;
    Ok(NewJob::new(scope, None, body)
        .map_err(invalid_job)?
        .with_dedup_key(dedup_key))
}

fn invalid_job(source: impl std::error::Error + Send + Sync + 'static) -> ReprojectionError {
    ReprojectionError::InvalidJob(application::error::box_error(source))
}
