//! Compatibility adapter for worker-local CDC operation types.
use crate::{WorkerScope, jobs::DomainJob};
pub(crate) use aura_historia_jobs::wire::{MAX_JOB_BYTES, WireError};

pub(crate) fn encode(job: &DomainJob) -> Result<String, WireError> {
    aura_historia_jobs::wire::encode(job)
}

pub(crate) fn decode(body: &str, expected_scope: WorkerScope) -> Result<DomainJob, WireError> {
    aura_historia_jobs::wire::decode(body, expected_scope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{cdc::CdcOperation, jobs::DomainJobPayload};

    #[test]
    fn should_preserve_worker_cdc_operation_on_schema_v2_round_trip()
    -> Result<(), Box<dyn std::error::Error>> {
        for (wire, key, operation) in [
            ("INSERT", "insert", CdcOperation::Insert),
            ("UPDATE", "update", CdcOperation::Update),
            ("DELETE", "delete", CdcOperation::Delete),
        ] {
            let body = format!(
                "{{\"schema_version\":2,\"scope\":\"search-filter-projection\",\"idempotency_key\":\"search-filter:sf_01h455vb4pex5vy7enb1p677vn:3:{key}\",\"ordering_key\":\"search-filter:sf_01h455vb4pex5vy7enb1p677vn\",\"job_type\":\"SEARCH_FILTER_CHANGED\",\"payload\":{{\"user_id\":\"usr_01h455vb4pex5vy7enb1p677vn\",\"user_search_filter_id\":\"sf_01h455vb4pex5vy7enb1p677vn\",\"version\":3,\"operation\":\"{wire}\"}}}}"
            );
            let job = decode(&body, WorkerScope::SearchFilterProjection)?;
            assert!(
                matches!(job.payload, DomainJobPayload::SearchFilterChanged(ref change) if change.operation == operation)
            );
            assert_eq!(body, encode(&job)?);
        }
        Ok(())
    }
}
