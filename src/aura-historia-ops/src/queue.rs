//! Queue and outbox inspection and redrive.

use crate::{OpsError, command::OUTBOX, command::Selection};
use platform_postgres_queue::{JobQueue, JobScope};
use platform_worker_queue_contract::job_queue::all_job_scopes;
use serde_json::{Value, json};
use sqlx::PgPool;

/// Depth, leases, oldest waiting age and custody of every catalogued scope and of every other
/// scope that holds jobs or custody, such as a projection generation, and the outbox's pending
/// and custody counts: the figures behind the host's queue metrics, which count every scope.
pub(crate) async fn inspect(pool: &PgPool) -> Result<Value, OpsError> {
    let present: Vec<String> = sqlx::query_scalar(
        "SELECT scope FROM job_queue_jobs UNION SELECT scope FROM job_queue_failures ORDER BY 1",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| OpsError::failed("inspect", error))?;
    let mut job_scopes = all_job_scopes();
    let mut unknown = Vec::new();
    for name in present {
        if job_scopes.iter().any(|scope| scope.name().as_str() == name) {
            continue;
        }
        match find_scope(&name) {
            Ok(scope) => job_scopes.push(scope),
            Err(_) => unknown.push(name),
        }
    }
    let mut scopes = serde_json::Map::new();
    for scope in job_scopes {
        let name = scope.name().as_str().to_owned();
        let stats = JobQueue::new(pool.clone(), scope)
            .inspect()
            .await
            .map_err(|error| OpsError::failed("inspect", error))?;
        scopes.insert(
            name,
            json!({
                "depth": stats.depth,
                "leased": stats.leased,
                "oldestWaitingAgeSeconds": stats.oldest_waiting_age.map(|age| age.as_secs()),
                "inCustody": stats.in_custody,
            }),
        );
    }
    let mut connection = pool
        .acquire()
        .await
        .map_err(|error| OpsError::failed("inspect", error))?;
    let outbox = delivery_outbox::inspect(&mut connection)
        .await
        .map_err(|error| OpsError::failed("inspect", error))?;
    Ok(json!({
        "scopes": scopes,
        // Scopes with jobs or custody that no catalogue or generation names; inspect them in SQL.
        "unknownScopes": unknown,
        "outbox": {
            "pending": outbox.pending,
            "oldestPendingAgeSeconds": outbox.oldest_pending_age.map(|age| age.as_secs()),
            "inCustody": outbox.custody,
        },
    }))
}

/// Moves custody entries of a scope, or of the outbox, back with their identity and order.
/// Custody is never deleted any other way.
pub(crate) async fn redrive(
    pool: &PgPool,
    scope: &str,
    selection: &Selection,
) -> Result<Value, OpsError> {
    if scope == OUTBOX {
        let selection = selection.outbox_rows().map_err(OpsError::Usage)?;
        let report = delivery_outbox::redrive(pool, &selection)
            .await
            .map_err(|error| OpsError::failed("redrive", error))?;
        return Ok(json!({
            "scope": OUTBOX,
            "redriven": report.redriven,
            "notInCustody": report.not_in_custody,
        }));
    }
    let job_scope = find_scope(scope)?;
    let selection = selection.jobs().map_err(OpsError::Usage)?;
    let report = JobQueue::new(pool.clone(), job_scope)
        .redrive(&selection)
        .await
        .map_err(|error| OpsError::failed("redrive", error))?;
    let ids = |ids: &[platform_postgres_queue::JobId]| {
        ids.iter().map(ToString::to_string).collect::<Vec<_>>()
    };
    Ok(json!({
        "scope": scope,
        "redriven": ids(&report.redriven),
        "conflicting": ids(&report.conflicting),
        "notInCustody": ids(&report.not_in_custody),
    }))
}

/// A catalogued scope, or a projection generation scope such as `product-listing-opensearch-g7`.
fn find_scope(name: &str) -> Result<JobScope, OpsError> {
    if let Some(scope) = all_job_scopes()
        .into_iter()
        .find(|scope| scope.name().as_str() == name)
    {
        return Ok(scope);
    }
    let live = platform_worker_queue_contract::job_queue::ProjectionTarget::Live.job_scope();
    name.strip_prefix(live.name().as_str())
        .and_then(|suffix| suffix.strip_prefix('-'))
        .and_then(|generation| {
            platform_worker_queue_contract::job_queue::ProjectionGeneration::new(generation).ok()
        })
        .map(|generation| {
            platform_worker_queue_contract::job_queue::ProjectionTarget::Generation(generation)
                .job_scope()
        })
        .ok_or_else(|| OpsError::Usage(format!("unknown scope {name}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_find_catalogued_and_generation_scopes_only() {
        assert_eq!(
            "geo-resolution",
            find_scope("geo-resolution").unwrap().name().as_str()
        );
        assert_eq!(
            "product-listing-opensearch-g7",
            find_scope("product-listing-opensearch-g7")
                .unwrap()
                .name()
                .as_str()
        );
        for unknown in [
            "unknown",
            "product-listing-opensearch-",
            "geo-resolution-g7",
        ] {
            assert!(find_scope(unknown).is_err(), "{unknown}");
        }
    }
}
