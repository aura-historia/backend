use crate::{
    WorkerScope,
    cdc::{CdcOperation, DomainJob, DomainJobPayload},
    queue::{JobOutcome, WorkerQueueReceiver},
};
use search_filter_service::use_cases::{
    ProjectSearchFilterChangeCommand, ProjectSearchFilterChangeError,
    ProjectSearchFilterChangeUseCase, SearchFilterProjectionOperation,
};
use std::sync::Arc;

/// Lambda- and polling-transport result for a fully handled saved-filter projection job.
///
/// Only `Complete` may be acknowledged. A missing upsert source is deliberately converted to
/// the service's external-versioned tombstone, so it is not an unconditional acknowledgment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFilterProjectionJobDisposition {
    Complete(&'static str),
    DependencyUnavailable(&'static str),
    Poison(&'static str),
}

impl SearchFilterProjectionJobDisposition {
    pub const fn category(self) -> &'static str {
        match self {
            Self::Complete(category)
            | Self::DependencyUnavailable(category)
            | Self::Poison(category) => category,
        }
    }
}

pub async fn consume_search_filter_projection_queue(
    receiver: impl Into<WorkerQueueReceiver>,
    handler: Arc<dyn ProjectSearchFilterChangeUseCase>,
) {
    receiver
        .into()
        .run(WorkerScope::SearchFilterProjection, move |job| {
            let handler = Arc::clone(&handler);
            async move { polling_outcome(execute_job(handler.as_ref(), job).await) }
        })
        .await;
}

/// Decode and execute one compact schema-2 saved-filter projection job.
///
/// This is transport-neutral so Lambda and the legacy polling worker preserve the same source
/// read, deletion-fence, validation, and retry rules.
pub async fn process_search_filter_projection_job(
    body: &str,
    handler: &(dyn ProjectSearchFilterChangeUseCase + Send + Sync),
) -> SearchFilterProjectionJobDisposition {
    match crate::wire::decode(body, WorkerScope::SearchFilterProjection) {
        Ok(job) => execute_job(handler, job).await,
        Err(_) => SearchFilterProjectionJobDisposition::Poison("invalid_wire_job"),
    }
}

async fn execute_job(
    handler: &(dyn ProjectSearchFilterChangeUseCase + Send + Sync),
    job: DomainJob,
) -> SearchFilterProjectionJobDisposition {
    let Ok(command) = command_from_job(job) else {
        return SearchFilterProjectionJobDisposition::Poison("projection_metadata_invalid");
    };
    match handler.execute(command).await {
        Ok(result) => {
            tracing::info!(outcome = ?result.outcome, "search filter projection write completed");
            SearchFilterProjectionJobDisposition::Complete("projection_written_or_stale")
        }
        Err(
            ProjectSearchFilterChangeError::InvalidSourceVersion
            | ProjectSearchFilterChangeError::DeleteVersionOverflow
            | ProjectSearchFilterChangeError::InvalidPersistedState { .. },
        ) => SearchFilterProjectionJobDisposition::Poison("projection_state_invalid"),
        Err(
            ProjectSearchFilterChangeError::ReadFailed { .. }
            | ProjectSearchFilterChangeError::WriteFailed { .. },
        ) => SearchFilterProjectionJobDisposition::DependencyUnavailable("projection_unavailable"),
    }
}

fn polling_outcome(disposition: SearchFilterProjectionJobDisposition) -> JobOutcome {
    match disposition {
        SearchFilterProjectionJobDisposition::Complete(category) => JobOutcome::Complete(category),
        SearchFilterProjectionJobDisposition::DependencyUnavailable(category) => {
            JobOutcome::DependencyUnavailable(category)
        }
        SearchFilterProjectionJobDisposition::Poison(category) => JobOutcome::Invalid(category),
    }
}
fn command_from_job(
    job: DomainJob,
) -> Result<ProjectSearchFilterChangeCommand, crate::jobs::InvalidJob> {
    let DomainJobPayload::SearchFilterChanged(change) = job.payload else {
        return Err(crate::jobs::InvalidJob);
    };
    if change.version <= 0 {
        return Err(crate::jobs::InvalidJob);
    }
    Ok(ProjectSearchFilterChangeCommand {
        search_filter_id: change.user_search_filter_id,
        source_version: change.version,
        operation: match change.operation {
            CdcOperation::Insert | CdcOperation::Update => SearchFilterProjectionOperation::Upsert,
            CdcOperation::Delete => SearchFilterProjectionOperation::Delete,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::{IdempotencyKey, OrderingKey, SearchFilterChangedJob, WorkerQueue};
    use search_filter_core::user_search_filter_id::UserSearchFilterId;
    use user_core::user_id::UserId;
    use uuid::Uuid;

    #[test]
    fn should_map_insert_update_and_delete_without_inventing_projection_completion()
    -> Result<(), Box<dyn std::error::Error>> {
        let user_id = UserId::try_from(Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0001))?;
        let id = UserSearchFilterId::try_from(Uuid::from_u128(
            0x0190_0000_0000_7000_8000_0000_0000_0006,
        ))?;
        assert_eq!("usr_01j0000000e008000000000001", user_id.to_string());
        assert_eq!("sf_01j0000000e008000000000006", id.to_string());
        for (operation, expected) in [
            (
                CdcOperation::Insert,
                SearchFilterProjectionOperation::Upsert,
            ),
            (
                CdcOperation::Update,
                SearchFilterProjectionOperation::Upsert,
            ),
            (
                CdcOperation::Delete,
                SearchFilterProjectionOperation::Delete,
            ),
        ] {
            let command = command_from_job(DomainJob {
                target_queue: WorkerQueue::SearchFilterOpenSearch,
                idempotency_key: IdempotencyKey::new(format!("search-filter:{id}:3:{operation}")),
                ordering_key: OrderingKey::new(format!("search-filter:{id}")),
                payload: DomainJobPayload::SearchFilterChanged(SearchFilterChangedJob {
                    user_id,
                    user_search_filter_id: id,
                    version: 3,
                    operation,
                }),
            });
            assert_eq!(
                Ok(ProjectSearchFilterChangeCommand {
                    search_filter_id: id,
                    source_version: 3,
                    operation: expected
                }),
                command
            );
        }
        Ok(())
    }
}
